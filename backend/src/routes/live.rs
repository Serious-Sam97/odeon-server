//! As rotas dos canais ao vivo.
//!
//! Duas perguntas diferentes, duas rotas: `channels` responde "o que está no ar
//! agora" (uma linha por canal) e `guide` responde "o que passa entre X e Y"
//! (muitas linhas por canal). Servir as duas com uma só obrigaria a tela do
//! "agora" a baixar a grade inteira.

use axum::extract::{Path, Query, State};
use axum::Json;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::{AdminUser, AuthUser};
use crate::error::{AppError, AppResult};
use crate::AppState;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CanalNoAr {
    pub id: Uuid,
    pub name: String,
    pub number: Option<String>,
    pub logo_url: Option<String>,
    pub grupo: Option<String>,
    /// O programa no ar. Nulo quando não há grade — o canal ainda toca.
    pub titulo: Option<String>,
    pub sub_titulo: Option<String>,
    pub comeca: Option<DateTime<Utc>>,
    pub termina: Option<DateTime<Utc>>,
    /// O próximo, só o título: é o que cabe num cartão.
    pub a_seguir: Option<String>,
    /// Id do programa no ar — é o que o lembrete referencia.
    pub programme_id: Option<i64>,
    /// Arte da obra ligada ao programa, quando o casamento foi seguro.
    /// Backdrop antes de pôster: o cartão é largo. Ver `live::ligar_obras`.
    pub arte: Option<String>,
    /// A obra da biblioteca que está passando, e o arquivo dela.
    ///
    /// É o que permite "ver desde o início": o stream está no meio do filme,
    /// mas o arquivo é seu. Nulo nos canais cujo programa não casou com obra
    /// nenhuma — e aí o botão simplesmente não aparece, em vez de aparecer e
    /// falhar. Medido: 11 dos 17 canais no ar têm casamento.
    pub work_id: Option<Uuid>,
    /// A **série** que está passando, quando o programa é de série (R68).
    ///
    /// O EPG anuncia o nome da série, não o do episódio, então não há obra pra
    /// apontar — há coleção. É por ela que a ficha da série abre, e é dela que
    /// sai a capa quando não há obra. Nunca vem junto com `work_id`: um é o
    /// alvo de quando o outro não existe.
    pub collection_id: Option<Uuid>,
    pub media_file_id: Option<Uuid>,
}

/// Os canais, com o que está no ar em cada um.
pub async fn channels(
    State(state): State<AppState>,
    _user: AuthUser,
) -> AppResult<Json<Vec<CanalNoAr>>> {
    let agora = Utc::now();
    let canais = sqlx::query_as::<_, CanalNoAr>(
        r#"
        SELECT c.id, c.name, c.number, c.logo_url, c.grupo,
               p.title AS titulo, p.sub_title AS sub_titulo,
               p.starts_at AS comeca, p.ends_at AS termina,
               s.title AS a_seguir,
               p.id AS programme_id,
               -- A ordem tem motivo. O `backdrop` da obra é deitado e veio do
               -- provedor: é o melhor fundo que existe. Depois vem a arte do
               -- PROGRAMA, que costuma ser um quadro daquele episódio — mais
               -- específica que o pôster da série, que serve pra temporada
               -- inteira. O pôster fica por último, cortado, porque é em pé.
               -- R68: a capa da série entra no fim da mesma escada, depois da
               -- arte do próprio programa. Ela é a menos específica de todas —
               -- vale pra série inteira —, e por isso é a última.
               COALESCE(w.artwork->>'backdrop', p.arte, w.artwork->>'poster',
                        col.artwork->>'backdrop', col.artwork->>'poster') AS arte,
               p.work_id,
               p.collection_id,
               (SELECT m.id FROM media_file m
                 WHERE m.work_id = p.work_id AND m.status = 'probed'
                 ORDER BY m.size_bytes DESC LIMIT 1) AS media_file_id
        FROM channel c
        LEFT JOIN LATERAL (
            SELECT id, title, sub_title, starts_at, ends_at, work_id, collection_id, arte
            FROM programme
            WHERE channel_id = c.id AND starts_at <= $1 AND ends_at > $1
            ORDER BY starts_at DESC LIMIT 1
        ) p ON true
        LEFT JOIN work w ON w.id = p.work_id
        LEFT JOIN collection col ON col.id = p.collection_id
        LEFT JOIN LATERAL (
            SELECT title FROM programme
            WHERE channel_id = c.id AND starts_at > $1
            ORDER BY starts_at LIMIT 1
        ) s ON true
        WHERE NOT c.hidden
        ORDER BY c.position NULLS LAST, c.name
        "#,
    )
    .bind(agora)
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(canais))
}

#[derive(Debug, Deserialize)]
pub struct GuiaQuery {
    /// Quantas horas à frente. O padrão é o que cabe na tela.
    #[serde(default = "tres")]
    pub hours: i64,
}
fn tres() -> i64 {
    3
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProgramaDoGuia {
    pub id: i64,
    pub channel_id: Uuid,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub title: String,
    pub sub_title: Option<String>,
    pub description: Option<String>,
    pub year: Option<i32>,
    pub categoria: Option<String>,
    pub arte: Option<String>,
    /// A obra da sua biblioteca, quando existe. É o que permite ao modal dizer
    /// "isto está na sua biblioteca".
    pub work_id: Option<Uuid>,
    /// A **série**, quando o programa é de série (R68) — o EPG anuncia o nome
    /// da série, e ele não casa com obra nenhuma. Nunca vem junto com
    /// `work_id`.
    pub collection_id: Option<Uuid>,
    /// O arquivo da obra, quando ela existe — é o que "ver desde o início" toca.
    pub media_file_id: Option<Uuid>,
    /// Já agendado por você.
    pub lembrete: bool,
}

/// A grade numa janela. Devolve o instante do servidor junto, porque a agulha
/// do "agora" tem que ser desenhada contra o mesmo relógio que produziu a
/// grade — o do navegador pode estar minutos fora.
pub async fn guide(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<GuiaQuery>,
) -> AppResult<Json<Value>> {
    let agora = Utc::now();
    let ate = agora + Duration::hours(params.hours.clamp(1, 24));

    let programas = sqlx::query_as::<_, ProgramaDoGuia>(
        "SELECT p.id, p.channel_id, p.starts_at, p.ends_at, p.title, p.sub_title,
                p.description, p.year, p.categoria, p.work_id, p.collection_id,
                COALESCE(w.artwork->>'backdrop', p.arte, w.artwork->>'poster',
                         col.artwork->>'backdrop', col.artwork->>'poster') AS arte,
                (SELECT m.id FROM media_file m
                  WHERE m.work_id = p.work_id AND m.status = 'probed'
                  ORDER BY m.size_bytes DESC LIMIT 1) AS media_file_id,
                (r.programme_id IS NOT NULL) AS lembrete
         FROM programme p
         JOIN channel c ON c.id = p.channel_id
         LEFT JOIN work w ON w.id = p.work_id
         LEFT JOIN collection col ON col.id = p.collection_id
         LEFT JOIN programme_reminder r ON r.programme_id = p.id AND r.user_id = $3
         WHERE NOT c.hidden AND p.ends_at > $1 AND p.starts_at < $2
         ORDER BY p.channel_id, p.starts_at",
    )
    .bind(agora)
    .bind(ate)
    .bind(user.id())
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(json!({
        "agora": agora,
        "ate": ate,
        "programas": programas,
    })))
}

// ------------------------------------------------------------------ fontes

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Fonte {
    pub id: Uuid,
    pub name: String,
    pub m3u_url: String,
    pub xmltv_url: Option<String>,
    pub enabled: bool,
    pub last_import_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub canais: i64,
}

pub async fn sources(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> AppResult<Json<Vec<Fonte>>> {
    let fontes = sqlx::query_as::<_, Fonte>(
        "SELECT s.*, (SELECT count(*) FROM channel c WHERE c.source_id = s.id) AS canais
         FROM channel_source s ORDER BY s.created_at",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(fontes))
}

#[derive(Debug, Deserialize)]
pub struct NovaFonte {
    pub name: String,
    pub m3u_url: String,
    pub xmltv_url: Option<String>,
}

pub async fn create_source(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Json(body): Json<NovaFonte>,
) -> AppResult<Json<Value>> {
    if body.m3u_url.trim().is_empty() {
        return Err(AppError::BadRequest("a URL da lista é obrigatória".into()));
    }

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO channel_source (name, m3u_url, xmltv_url) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(body.name.trim())
    .bind(body.m3u_url.trim())
    .bind(body.xmltv_url.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(json!({ "id": id })))
}

pub async fn delete_source(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let r = sqlx::query("DELETE FROM channel_source WHERE id = $1")
        .bind(id)
        .execute(&state.pool)
        .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(json!({ "ok": true })))
}

/// Importa todas as fontes ativas, em segundo plano.
pub async fn import(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> AppResult<Json<Value>> {
    let fontes: Vec<Uuid> =
        sqlx::query_scalar("SELECT id FROM channel_source WHERE enabled ORDER BY created_at")
            .fetch_all(&state.pool)
            .await?;

    if fontes.is_empty() {
        return Ok(Json(json!({
            "started": false,
            "reason": "nenhuma fonte cadastrada"
        })));
    }

    let total = fontes.len();
    let job = crate::jobs::Job::start(&state.pool, "live_import", json!({}), None).await;
    let job_id = job.as_ref().map(|j| j.id);
    let pool = state.pool.clone();
    let http = state.providers.http.clone();
    let artwork_dir = state.config.artwork_dir.clone();

    tokio::spawn(async move {
        let mut canais = 0usize;
        let mut programas = 0usize;
        let mut erros: Vec<String> = Vec::new();

        for (i, fonte) in fontes.iter().enumerate() {
            match crate::live::importar(&pool, &http, &artwork_dir, *fonte, None).await {
                Ok((c, p)) => {
                    canais += c;
                    programas += p;
                }
                Err(e) => {
                    // O erro fica NA FONTE, não só no log: a tela precisa poder
                    // dizer por que aquela lista não entrou.
                    crate::live::registrar_erro(&pool, *fonte, &e.to_string()).await;
                    erros.push(e.to_string());
                }
            }
            if let Some(j) = &job {
                j.tick(
                    &json!({ "canais": canais, "programas": programas }),
                    i as i64 + 1,
                    Some(fontes.len() as i64),
                )
                .await;
            }
        }

        if let Some(j) = job {
            let estado = if erros.is_empty() { "succeeded" } else { "failed" };
            j.finish(
                &json!({ "canais": canais, "programas": programas }),
                estado,
                erros.first().cloned(),
            )
            .await;
        }
        tracing::info!(canais, programas, "importação de canais concluída");
    });

    Ok(Json(json!({ "started": true, "job_id": job_id, "fontes": total })))
}

// --------------------------------------------------------------- assistir

/// Abre (ou entra em) a transmissão de um canal.
///
/// A sessão é **por canal, não por usuário**: ao vivo todo mundo vê a mesma
/// coisa, e um ffmpeg por espectador seria desperdício puro. É a diferença
/// central em relação ao transcode sob demanda, onde cada pessoa está num ponto
/// diferente do arquivo.
pub async fn watch(
    State(state): State<AppState>,
    _user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let canal: Option<(String, String)> =
        sqlx::query_as("SELECT name, stream_url FROM channel WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let (nome, stream_url) = canal.ok_or(AppError::NotFound)?;

    let sessao = state
        .transcode
        .live(id, &stream_url)
        .await
        .map_err(|e| AppError::BadRequest(format!("não consegui abrir o canal: {e}")))?;

    // **Só responde quando há o que tocar.**
    //
    // Sem isto, `watch` devolvia a URL na hora e o ffmpeg ainda estava
    // conectando no provedor — a primeira requisição de playlist ficava presa em
    // `wait_for_playlist` por até 25s, e o hls.js desiste bem antes. O sintoma
    // era cruel: nenhum erro em lugar nenhum, playlist e segmentos respondendo
    // 200 quando testados à mão, e o player parado em `MANIFEST_LOADING` para
    // sempre. Do lado de cá custa ~10s; do lado de lá, funcionava ou não.
    if state.transcode.wait_for_playlist(sessao.id).await.is_none() {
        state.transcode.stop(sessao.id).await;
        return Err(AppError::BadRequest(
            "o canal não começou a transmitir a tempo — a fonte pode estar fora do ar".into(),
        ));
    }

    Ok(Json(json!({
        "channel": { "id": id, "name": nome },
        "session_id": sessao.id,
        "playlist_url": format!("/api/hls/{}/index.m3u8", sessao.id),
        // O ErsatzTV e a maioria dos provedores servem MPEG-TS, que navegador
        // nenhum toca — então ao vivo é sempre no mínimo um remux, e o selo do
        // player diz isso em vez de fingir Direct Play.
        "mode": sessao.plan.mode,
        "reasons": sessao.plan.reasons,
    })))
}

// ------------------------------------------------------------- lembretes

/// Agenda "me avisa quando começar".
///
/// Idempotente: pedir duas vezes não cria duas linhas nem reabre um aviso já
/// dado — `ON CONFLICT DO NOTHING`, e não `DO UPDATE`, porque zerar o
/// `notified_at` faria o vigia avisar de novo.
pub async fn create_reminder(
    State(state): State<AppState>,
    user: AuthUser,
    Path(programme_id): Path<i64>,
) -> AppResult<Json<Value>> {
    let existe: Option<(DateTime<Utc>,)> =
        sqlx::query_as("SELECT starts_at FROM programme WHERE id = $1")
            .bind(programme_id)
            .fetch_optional(&state.pool)
            .await?;
    let (comeca,) = existe.ok_or(AppError::NotFound)?;

    if comeca < Utc::now() {
        return Err(AppError::BadRequest(
            "esse programa já começou — não dá pra agendar o passado".into(),
        ));
    }

    sqlx::query(
        "INSERT INTO programme_reminder (user_id, programme_id) VALUES ($1, $2)
         ON CONFLICT DO NOTHING",
    )
    .bind(user.id())
    .bind(programme_id)
    .execute(&state.pool)
    .await?;

    Ok(Json(json!({ "ok": true, "starts_at": comeca })))
}

pub async fn delete_reminder(
    State(state): State<AppState>,
    user: AuthUser,
    Path(programme_id): Path<i64>,
) -> AppResult<Json<Value>> {
    sqlx::query("DELETE FROM programme_reminder WHERE user_id = $1 AND programme_id = $2")
        .bind(user.id())
        .bind(programme_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Lembrete {
    pub programme_id: i64,
    pub title: String,
    pub starts_at: DateTime<Utc>,
    pub channel_id: Uuid,
    pub channel_name: String,
}

/// Os agendamentos futuros de quem está logado.
pub async fn reminders(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Vec<Lembrete>>> {
    let itens = sqlx::query_as::<_, Lembrete>(
        "SELECT r.programme_id, p.title, p.starts_at, c.id AS channel_id, c.name AS channel_name
         FROM programme_reminder r
         JOIN programme p ON p.id = r.programme_id
         JOIN channel c ON c.id = p.channel_id
         WHERE r.user_id = $1 AND p.ends_at > now()
         ORDER BY p.starts_at",
    )
    .bind(user.id())
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(itens))
}

// ----------------------------------------------------------- a emissora

/// A grade dos canais que o próprio Odeon programa.
///
/// Separado de `/api/live/guide` de propósito: aquele lê uma grade que alguém
/// publicou, este **calcula** uma. Misturar os dois numa rota só faria a
/// resposta ter dois formatos e obrigaria o cliente a adivinhar qual veio.
pub async fn odeon_guide(
    State(state): State<AppState>,
    _user: AuthUser,
    Query(params): Query<GuiaQuery>,
) -> AppResult<Json<Value>> {
    let agora = Utc::now();
    let ate = agora + Duration::hours(params.hours.clamp(1, 24));
    // `AppError::Other` cobre o `anyhow` que a emissora devolve.
    let programas = crate::live::emissora::grade_toda(&state.pool, agora, ate).await?;

    Ok(Json(json!({
        "agora": agora,
        "ate": ate,
        "canais": crate::live::emissora::CANAIS.iter().map(|c| json!({
            "slug": c.slug, "nome": c.nome, "numero": c.numero,
        })).collect::<Vec<_>>(),
        "programas": programas,
    })))
}
