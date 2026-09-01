pub mod guess;
pub mod probe;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::models::Library;

/// O que conta como vídeo — **R73**.
///
/// A lista original foi escrita no M0 e nunca revista. Comparada com o
/// Jellyfin da mesma casa, em 20/08/2026, sobre os mesmos três discos:
///
/// | | |
/// |---|---|
/// | arquivos que os dois enxergam | 16.938 |
/// | **só o Jellyfin enxergava** | **693** |
/// | só o Odeon enxerga | 1.065 (extras que o Jellyfin classifica à parte) |
///
/// E os 693 não eram mistério nenhum — 677 `.rmvb`, 7 `.divx`, 1 `.asf`, e 8
/// caminhos que **já não existem no disco** (o Jellyfin é que estava
/// desatualizado; o Odeon estava certo). Ou seja: a diferença inteira era
/// extensão faltando.
///
/// `.rmvb` é a maior fatia sozinha e é conteúdo de verdade — conferido no
/// ffprobe: `rv40` + `cook`, 1.300 s, uma temporada de *Uma Família da
/// Pesada*. Nenhum cliente decodifica RealMedia, mas isso é assunto do
/// `decide`, que já sabe recodificar o que não toca. **Não descobrir é a única
/// falha irreparável do pipeline** — o que não entra aqui não existe pro resto
/// do produto.
///
/// ⚠️ **O que ficou de fora, e de propósito**: `.mp3` (1.685) e `.flac`
/// (1.213) são música, e este servidor não é de música; `.mca` (65) e `.dat`
/// (42) são mundo de Minecraft. Conferido arquivo a arquivo no disco — não há
/// mais nenhuma extensão de vídeo acima de 1 MB fora desta lista.
const VIDEO_EXTS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "m4v", "webm", "ts", "m2ts", "mpg", "mpeg", "wmv", "flv", "ogv",
    // R73 — a leva que o Jellyfin via e nós não.
    "rmvb", "rm", "divx", "m2v", "asf",
];

/// Abaixo disso é sample, trailer ou thumbnail — não é obra.
const MIN_SIZE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct ScanStatus {
    pub running: bool,
    pub library: Option<String>,
    pub current_file: Option<String>,
    pub files_seen: u64,
    pub files_added: u64,
    pub files_updated: u64,
    pub files_missing: u64,
    pub errors: Vec<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

pub type SharedStatus = Arc<Mutex<ScanStatus>>;

enum Outcome {
    Added,
    Updated,
    Unchanged,
}

/// Varre todas as bibliotecas. Só roda uma por vez; chamada concorrente é no-op.
/// Devolve `false` se já havia um scan em andamento.
/// Varre as bibliotecas. `apenas` limita ao `default_kind` pedido — R73.
///
/// `None` varre tudo, que é o comportamento de sempre e o que o `/api/scan`
/// sem `tipo=` continua fazendo.
pub async fn scan_kind(
    pool: PgPool,
    status: SharedStatus,
    job: Option<crate::jobs::Job>,
    apenas: Option<&str>,
) -> bool {
    {
        let mut s = status.lock().await;
        if s.running {
            return false;
        }
        *s = ScanStatus {
            running: true,
            started_at: Some(Utc::now()),
            ..Default::default()
        };
    }

    let started_at = Utc::now();

    let libraries: Vec<Library> = match sqlx::query_as(
        "SELECT * FROM library
          WHERE $1::text IS NULL OR default_kind = $1
          ORDER BY created_at",
    )
    .bind(apenas)
    .fetch_all(&pool)
    .await
    {
        Ok(l) => l,
        Err(e) => {
            let mut s = status.lock().await;
            s.errors.push(format!("falha ao listar bibliotecas: {e}"));
            s.running = false;
            s.finished_at = Some(Utc::now());
            return true;
        }
    };

    let mut cancelado = false;
    for library in &libraries {
        scan_library(&pool, library, &status, started_at, job.as_ref(), &mut cancelado).await;
        if cancelado {
            break;
        }
    }

    let mut s = status.lock().await;
    s.running = false;
    s.current_file = None;
    s.finished_at = Some(Utc::now());
    tracing::info!(
        seen = s.files_seen,
        added = s.files_added,
        updated = s.files_updated,
        missing = s.files_missing,
        errors = s.errors.len(),
        cancelado,
        "scan concluído"
    );
    if let Some(j) = job {
        j.finish(&*s, if cancelado { "cancelled" } else { "succeeded" }, None)
            .await;
    }
    true
}

async fn scan_library(
    pool: &PgPool,
    library: &Library,
    status: &SharedStatus,
    started_at: DateTime<Utc>,
    job: Option<&crate::jobs::Job>,
    cancelado: &mut bool,
) {
    status.lock().await.library = Some(library.name.clone());
    tracing::info!(library = %library.name, root = %library.root_path, "varrendo");

    let root = library.root_path.clone();
    let files = match tokio::task::spawn_blocking(move || collect_files(&root)).await {
        Ok(f) => f,
        Err(e) => {
            status
                .lock()
                .await
                .errors
                .push(format!("walk falhou em {}: {e}", library.root_path));
            return;
        }
    };

    for path in files {
        {
            let mut s = status.lock().await;
            s.files_seen += 1;
            s.current_file = Some(path.to_string_lossy().to_string());
        }

        match process_file(pool, library, &path).await {
            Ok(Outcome::Added) => status.lock().await.files_added += 1,
            Ok(Outcome::Updated) => status.lock().await.files_updated += 1,
            Ok(Outcome::Unchanged) => {}
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "arquivo ignorado");
                let mut s = status.lock().await;
                if s.errors.len() < 100 {
                    s.errors.push(format!("{}: {e}", path.display()));
                }
            }
        }

        // O arquivo corrente terminou de ser gravado: é ponto seguro pra parar.
        // A cada 50 porque `ffprobe` é lento e o custo da consulta some no meio.
        if let Some(j) = job {
            let vistos = status.lock().await.files_seen;
            if vistos % 50 == 0 {
                let atual = status.lock().await.clone();
                j.tick(&atual, vistos as i64, None).await;
                if j.cancelled().await {
                    *cancelado = true;
                    tracing::info!(vistos, "varredura cancelada a pedido");
                    return;
                }
            }
        }
    }

    // Tudo que não foi tocado nesta passada sumiu do disco. Não apagamos —
    // marcamos, pra não perder histórico de play de um HD desconectado.
    match sqlx::query(
        "UPDATE media_file SET status = 'missing'
         WHERE library_id = $1 AND scanned_at < $2 AND status <> 'missing'",
    )
    .bind(library.id)
    .bind(started_at)
    .execute(pool)
    .await
    {
        Ok(r) => status.lock().await.files_missing += r.rows_affected(),
        Err(e) => status
            .lock()
            .await
            .errors
            .push(format!("falha ao marcar arquivos sumidos: {e}")),
    }
}

fn collect_files(root: &str) -> Vec<PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            // pula ocultos e as pastas de metadata do macOS/Synology
            let name = e.file_name().to_string_lossy();
            !(name.starts_with('.') || name == "@eaDir" || name == "lost+found")
        })
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| VIDEO_EXTS.contains(&x.to_ascii_lowercase().as_str()))
                .unwrap_or(false)
        })
        .filter(|e| e.metadata().map(|m| m.len() >= MIN_SIZE_BYTES).unwrap_or(false))
        .map(|e| e.into_path())
        .collect()
}

async fn process_file(pool: &PgPool, library: &Library, path: &Path) -> anyhow::Result<Outcome> {
    let meta = tokio::fs::metadata(path).await?;
    let size_bytes = meta.len() as i64;
    let mtime: DateTime<Utc> = meta.modified()?.into();
    let path_str = path.to_string_lossy().to_string();
    let filename = path
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_else(|| path_str.clone());

    let existing: Option<(Uuid, i64, DateTime<Utc>, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT id, size_bytes, mtime, status, work_id FROM media_file WHERE path = $1",
    )
    .bind(&path_str)
    .fetch_optional(pool)
    .await?;

    // Arquivo já conhecido e intacto: só carimba a data e sai.
    // (tolerância de 1s porque o filesystem tem resolução maior que o timestamptz)
    if let Some((id, prev_size, prev_mtime, prev_status, _)) = &existing {
        let unchanged = *prev_size == size_bytes
            && (*prev_mtime - mtime).num_milliseconds().abs() < 1000
            && prev_status == "probed";
        if unchanged {
            sqlx::query("UPDATE media_file SET scanned_at = now() WHERE id = $1")
                .bind(id)
                .execute(pool)
                .await?;
            return Ok(Outcome::Unchanged);
        }
    }

    let probed = probe::probe(path).await;

    let (probed, status, error_message) = match probed {
        Ok(p) => (Some(p), "probed", None),
        Err(e) => (None, "error", Some(e.to_string())),
    };

    // Um arquivo que o ffprobe não lê não vira obra — vira pendência.
    let work_id = match (&probed, existing.as_ref().and_then(|e| e.4)) {
        (None, previous) => previous,
        (Some(_), Some(previous)) => Some(previous),
        (Some(p), None) => {
            // `guess_from_path` e não `guess_from_filename`: é aqui que
            // `season_number` e `episode_number` da obra são gravados, e o
            // nome do arquivo sozinho frequentemente não os tem
            // (`Show/Season 1/01.mkv`). Sem o contexto do diretório a obra
            // nasce sem numeração e nenhum caminho posterior a corrige.
            let guess = guess::guess_from_path(
                path,
                Path::new(&library.root_path),
                library.default_kind == "episode",
            );
            Some(create_work(pool, library, &guess, p.duration_seconds).await?)
        }
    };

    sqlx::query(
        r#"
        INSERT INTO media_file (
            library_id, work_id, path, filename, size_bytes, mtime,
            container, duration_seconds, bitrate,
            video_codec, width, height, frame_rate,
            audio_codec, audio_channels, subtitle_langs,
            probe, status, error_message, scanned_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19, now())
        ON CONFLICT (path) DO UPDATE SET
            work_id          = EXCLUDED.work_id,
            size_bytes       = EXCLUDED.size_bytes,
            mtime            = EXCLUDED.mtime,
            container        = EXCLUDED.container,
            duration_seconds = EXCLUDED.duration_seconds,
            bitrate          = EXCLUDED.bitrate,
            video_codec      = EXCLUDED.video_codec,
            width            = EXCLUDED.width,
            height           = EXCLUDED.height,
            frame_rate       = EXCLUDED.frame_rate,
            audio_codec      = EXCLUDED.audio_codec,
            audio_channels   = EXCLUDED.audio_channels,
            subtitle_langs   = EXCLUDED.subtitle_langs,
            probe            = EXCLUDED.probe,
            status           = EXCLUDED.status,
            error_message    = EXCLUDED.error_message,
            scanned_at       = now()
        "#,
    )
    .bind(library.id)
    .bind(work_id)
    .bind(&path_str)
    .bind(&filename)
    .bind(size_bytes)
    .bind(mtime)
    .bind(probed.as_ref().and_then(|p| p.container.clone()))
    .bind(probed.as_ref().and_then(|p| p.duration_seconds))
    .bind(probed.as_ref().and_then(|p| p.bitrate))
    .bind(probed.as_ref().and_then(|p| p.video_codec.clone()))
    .bind(probed.as_ref().and_then(|p| p.width))
    .bind(probed.as_ref().and_then(|p| p.height))
    .bind(probed.as_ref().and_then(|p| p.frame_rate))
    .bind(probed.as_ref().and_then(|p| p.audio_codec.clone()))
    .bind(probed.as_ref().and_then(|p| p.audio_channels))
    .bind(
        probed
            .as_ref()
            .map(|p| p.subtitle_langs.clone())
            .unwrap_or_default(),
    )
    .bind(probed.as_ref().map(|p| p.raw.clone()))
    .bind(status)
    .bind(error_message.clone())
    .execute(pool)
    .await?;

    if let Some(msg) = error_message {
        anyhow::bail!(msg);
    }

    Ok(if existing.is_some() {
        Outcome::Updated
    } else {
        Outcome::Added
    })
}

async fn create_work(
    pool: &PgPool,
    library: &Library,
    guess: &guess::Guess,
    duration_seconds: Option<f64>,
) -> anyhow::Result<Uuid> {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO work (kind, title, year, season_number, episode_number, runtime_seconds)
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(guess.kind(&library.default_kind))
    .bind(&guess.title)
    .bind(guess.year)
    .bind(guess.season)
    .bind(guess.episode)
    .bind(duration_seconds.map(|d| d.round() as i32))
    .fetch_one(pool)
    .await?;

    // R64 — o formato nasce aqui, e não na identificação.
    //
    // Ele era escrito só no `apply_candidate`, então 7.376 entradas do acervo
    // não tinham nenhum e sumiam de toda prateleira. Mas "é um filme" e "é
    // *este* filme" são perguntas diferentes: a primeira o `kind` acima já
    // responde, de graça e sem rede. A identificação ainda refina — `anime` só
    // ela sabe — e por isso ela substitui em vez de acrescentar.
    let kind = guess.kind(&library.default_kind);
    crate::metadata::formato::gravar_do_kind(pool, id, &kind).await;

    Ok(id)
}
