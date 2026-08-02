//! Geração da folha de sprites para preview de seek.
//!
//! O Jellyfin trata isso como extra opcional e escondido. Aqui é parte do
//! player: arrastar a timeline mostra o quadro daquele instante, sem esperar
//! nada carregar.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use tokio::process::Command;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Quantos quadros amostrar por arquivo, independente da duração. 120 dá
/// granularidade boa num filme de 2h (um quadro a cada ~60s) sem estourar o
/// tamanho da folha.
const TARGET_FRAMES: i32 = 120;
/// Nunca amostrar mais denso que isto — em vídeo curto, 120 quadros seria
/// quase quadro-a-quadro e a folha ficaria enorme à toa.
const MIN_INTERVAL: f64 = 2.0;
const THUMB_WIDTH: i32 = 160;
const COLUMNS: i32 = 10;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SpriteInfo {
    pub media_file_id: Uuid,
    pub path: String,
    pub interval_seconds: f32,
    pub columns: i32,
    pub rows: i32,
    pub thumb_width: i32,
    pub thumb_height: i32,
    pub frame_count: i32,
}

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct ScrubStatus {
    pub running: bool,
    pub current: Option<String>,
    pub total: u64,
    pub done: u64,
    pub failed: u64,
    pub errors: Vec<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

pub type SharedScrubStatus = Arc<Mutex<ScrubStatus>>;

#[derive(Debug, sqlx::FromRow)]
struct Pending {
    id: Uuid,
    path: String,
    duration_seconds: Option<f64>,
    width: Option<i32>,
    height: Option<i32>,
}

/// Gera sprites pra todos os arquivos que ainda não têm.
///
/// Custo real: o ffmpeg decodifica o arquivo inteiro pra amostrar os quadros.
/// Num filme de 2h isso leva minutos. Por isso roda em background, uma vez por
/// arquivo, e o resultado fica em cache pra sempre.
pub async fn generate_all(
    pool: PgPool,
    scrub_dir: PathBuf,
    status: SharedScrubStatus,
    force: bool,
    job: Option<crate::jobs::Job>,
) -> bool {
    {
        let mut s = status.lock().await;
        if s.running {
            return false;
        }
        *s = ScrubStatus {
            running: true,
            started_at: Some(Utc::now()),
            ..Default::default()
        };
    }

    if let Err(e) = tokio::fs::create_dir_all(&scrub_dir).await {
        let mut s = status.lock().await;
        s.errors.push(format!("não consegui criar {}: {e}", scrub_dir.display()));
        s.running = false;
        s.finished_at = Some(Utc::now());
        return true;
    }

    let pending: Vec<Pending> = match sqlx::query_as(
        r#"
        SELECT m.id, m.path, m.duration_seconds, m.width, m.height
        FROM media_file m
        LEFT JOIN scrub_sprite s ON s.media_file_id = m.id
        WHERE m.status = 'probed'
          AND m.duration_seconds > 0
          AND ($1 OR s.media_file_id IS NULL)
        ORDER BY m.size_bytes
        "#,
    )
    .bind(force)
    .fetch_all(&pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            let mut s = status.lock().await;
            s.errors.push(format!("falha ao listar arquivos: {e}"));
            s.running = false;
            s.finished_at = Some(Utc::now());
            return true;
        }
    };

    status.lock().await.total = pending.len() as u64;
    let total = pending.len();
    let mut cancelado = false;
    tracing::info!(total, "geração de sprites iniciada");

    for file in pending {
        status.lock().await.current = Some(file.path.clone());

        match generate_one(&pool, &scrub_dir, &file).await {
            Ok(_) => status.lock().await.done += 1,
            Err(e) => {
                tracing::warn!(path = %file.path, error = %e, "sprite falhou");
                let mut s = status.lock().await;
                s.failed += 1;
                if s.errors.len() < 50 {
                    s.errors.push(format!("{}: {e}", file.path));
                }
            }
        }

        // Checa a cada arquivo, não a cada N: um sprite leva MINUTOS (o ffmpeg
        // decodifica o arquivo inteiro), então a consulta ao banco é ruído
        // perto disso — e esperar 50 arquivos pra parar seria uma hora.
        if let Some(j) = &job {
            let atual = status.lock().await.clone();
            let feitos = (atual.done + atual.failed) as i64;
            j.tick(&atual, feitos, Some(total as i64)).await;
            if j.cancelled().await {
                cancelado = true;
                tracing::info!(feitos, "geração de sprites cancelada a pedido");
                break;
            }
        }
    }

    let mut s = status.lock().await;
    s.running = false;
    s.current = None;
    s.finished_at = Some(Utc::now());
    tracing::info!(
        done = s.done,
        failed = s.failed,
        cancelado,
        "geração de sprites concluída"
    );
    if let Some(j) = job {
        j.finish(&*s, if cancelado { "cancelled" } else { "succeeded" }, None)
            .await;
    }
    true
}

async fn generate_one(pool: &PgPool, scrub_dir: &Path, file: &Pending) -> anyhow::Result<SpriteInfo> {
    let duration = file.duration_seconds.unwrap_or(0.0);
    anyhow::ensure!(duration > 0.0, "duração desconhecida");

    let interval = (duration / TARGET_FRAMES as f64).max(MIN_INTERVAL);
    let frame_count = ((duration / interval).ceil() as i32).max(1);
    // div_ceil em inteiro com sinal ainda é unstable no Rust estável.
    let rows = (frame_count + COLUMNS - 1) / COLUMNS;

    // Altura proporcional ao vídeo, arredondada pra par (exigência do encoder).
    let thumb_height = match (file.width, file.height) {
        (Some(w), Some(h)) if w > 0 => {
            let raw = (THUMB_WIDTH as f64 * h as f64 / w as f64).round() as i32;
            (raw + raw % 2).max(2)
        }
        _ => 90,
    };

    let filename = format!("{}.jpg", file.id);
    let destination = scrub_dir.join(&filename);

    let filter = format!(
        "fps=1/{interval:.4},scale={THUMB_WIDTH}:{thumb_height},tile={COLUMNS}x{rows}"
    );

    // `-skip_frame nokey`: decodifica só os quadros-chave.
    //
    // O custo desta operação era proporcional à DURAÇÃO do acervo, não à
    // quantidade de arquivos: o filtro `fps` obriga o decode do arquivo inteiro
    // pra amostrar 120 quadros. Medido nesta máquina, num 1080p de 20 minutos:
    //
    //     sem a flag: 126,0s        com a flag: 17,5s        7,2x
    //
    // A geometria não muda (verificado: 1600x1080 nos dois), o que importa
    // porque o player acha a célula por aritmética (§8d) e qualquer mudança ali
    // mostraria o quadro errado. O que muda é que o quadro exibido é o
    // quadro-chave mais próximo, deslocado em no máximo um GOP — o mesmo
    // compromisso que o §8g já aceita no seek do transcode.
    //
    // Medi também a alternativa de blocos com `-ss` que dava 22x na estimativa
    // de origem: aqui deu 28,1s, PIOR que esta. Fica registrado pra ninguém
    // refazer a medição achando que vai ganhar.
    let rodar = |pular_nao_chave: bool| {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-v", "error", "-y"]);
        if pular_nao_chave {
            cmd.args(["-skip_frame", "nokey"]);
        }
        cmd.arg("-i")
            .arg(&file.path)
            .args(["-vf", &filter, "-frames:v", "1", "-q:v", "6", "-an"])
            .arg(&destination);
        cmd.output()
    };

    let mut output = rodar(true).await.context("falha ao executar ffmpeg")?;

    // Codec ou container que não coopera com a flag: refaz do jeito lento.
    // Robustez num arquivo isolado vale mais que velocidade — o resultado
    // errado é invisível, e o preview mostraria o quadro de outro instante.
    if !output.status.success() {
        tracing::debug!(
            path = %file.path,
            "skip_frame falhou, refazendo com decode integral"
        );
        output = rodar(false).await.context("falha ao executar ffmpeg")?;
    }

    if !output.status.success() {
        anyhow::bail!(
            "ffmpeg saiu com {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let info = SpriteInfo {
        media_file_id: file.id,
        path: filename,
        interval_seconds: interval as f32,
        columns: COLUMNS,
        rows,
        thumb_width: THUMB_WIDTH,
        thumb_height,
        frame_count,
    };

    sqlx::query(
        "INSERT INTO scrub_sprite
            (media_file_id, path, interval_seconds, columns, rows,
             thumb_width, thumb_height, frame_count)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
         ON CONFLICT (media_file_id) DO UPDATE SET
            path = EXCLUDED.path,
            interval_seconds = EXCLUDED.interval_seconds,
            columns = EXCLUDED.columns,
            rows = EXCLUDED.rows,
            thumb_width = EXCLUDED.thumb_width,
            thumb_height = EXCLUDED.thumb_height,
            frame_count = EXCLUDED.frame_count,
            created_at = now()",
    )
    .bind(info.media_file_id)
    .bind(&info.path)
    .bind(info.interval_seconds)
    .bind(info.columns)
    .bind(info.rows)
    .bind(info.thumb_width)
    .bind(info.thumb_height)
    .bind(info.frame_count)
    .execute(pool)
    .await?;

    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A geometria da folha é o que o player usa pra achar a célula; se ela
    /// estiver errada o preview mostra o quadro errado.
    fn geometry(duration: f64) -> (f64, i32, i32) {
        let interval = (duration / TARGET_FRAMES as f64).max(MIN_INTERVAL);
        let frames = ((duration / interval).ceil() as i32).max(1);
        (interval, frames, (frames + COLUMNS - 1) / COLUMNS)
    }

    #[test]
    fn filme_de_duas_horas_cabe_na_grade() {
        let (interval, frames, rows) = geometry(7200.0);
        assert_eq!(frames, TARGET_FRAMES);
        assert_eq!(rows, 12);
        assert!((interval - 60.0).abs() < 0.01);
    }

    #[test]
    fn video_curto_nao_amostra_denso_demais() {
        // 30s: 30/120 = 0.25s seria quadro-a-quadro; o piso de 2s protege
        let (interval, frames, _) = geometry(30.0);
        assert_eq!(interval, MIN_INTERVAL);
        assert_eq!(frames, 15);
    }

    #[test]
    fn grade_sempre_comporta_todos_os_quadros() {
        for duration in [5.0, 61.0, 600.0, 3600.0, 10800.0] {
            let (_, frames, rows) = geometry(duration);
            assert!(rows * COLUMNS >= frames, "duração {duration}");
        }
    }
}
