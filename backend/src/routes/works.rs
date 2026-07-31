use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::models::{
    MediaFileSummary, ProgressReport, Work, WorkDetail, WorkListItem,
};
use crate::routes::graph;
use crate::AppState;

/// A obra é considerada assistida a partir daqui. 92% deixa os créditos de fora.
const FINISHED_RATIO: f64 = 0.92;

/// Projeção compartilhada por biblioteca, "continuar assistindo" e coleção.
/// Fica num const só pra as três não divergirem.
const WORK_COLUMNS: &str = r#"
    w.id, w.kind, w.title, w.year, w.season_number, w.episode_number,
    w.match_state, w.match_confidence, w.dominant_color,
    w.artwork->>'poster' AS poster,
    f.id AS media_file_id, f.duration_seconds, f.width, f.height,
    f.video_codec, f.audio_codec, f.container, f.size_bytes,
    ps.position_seconds, ps.finished,
    tg.tags
"#;

/// Melhor arquivo da obra, nome da série (subindo episódio→temporada→série) e
/// as tags achatadas em `namespace:valor`.
const WORK_JOINS: &str = r#"
LEFT JOIN LATERAL (
    SELECT m.* FROM media_file m
    WHERE m.work_id = w.id AND m.status = 'probed'
    ORDER BY m.size_bytes DESC LIMIT 1
) f ON true
-- Só temporada/série contam como "obra-mãe". Sem este filtro, uma playlist ou
-- uma ordem de exibição apareceria no card como se fosse o nome da série.
LEFT JOIN LATERAL (
    SELECT COALESCE(series.title, season.title) AS series_title
    FROM collection_item ci
    JOIN collection season ON season.id = ci.collection_id
    LEFT JOIN collection series ON series.id = season.parent_id
    WHERE ci.work_id = w.id
      AND season.kind IN ('season', 'series')
    LIMIT 1
) s ON true
LEFT JOIN LATERAL (
    SELECT array_agg(t.namespace || ':' || t.value ORDER BY t.namespace, t.value) AS tags
    FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
    WHERE wt.work_id = w.id
) tg ON true
LEFT JOIN playback_state ps ON ps.work_id = w.id AND ps.user_id = $1
"#;

#[derive(Debug, Deserialize)]
pub struct ListParams {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    /// Lista separada por vírgula em `namespace:valor` — `genre:Ação,format:anime`
    #[serde(default)]
    pub tags: Option<String>,
    /// `all` exige todas as tags; `any` aceita qualquer uma.
    #[serde(default = "default_tag_mode")]
    pub tag_mode: String,
    #[serde(default)]
    pub year_from: Option<i32>,
    #[serde(default)]
    pub year_to: Option<i32>,
    #[serde(default)]
    pub min_minutes: Option<i32>,
    #[serde(default)]
    pub max_minutes: Option<i32>,
    /// Inclui a subárvore inteira da coleção, não só os filhos diretos.
    #[serde(default)]
    pub collection: Option<Uuid>,
    #[serde(default)]
    pub state: Option<String>,
    /// "tudo do Villeneuve" — a consulta que justifica a tabela `credit`.
    #[serde(default)]
    pub person: Option<Uuid>,
    #[serde(default)]
    pub person_role: Option<String>,
    #[serde(default = "default_sort")]
    pub sort: String,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    120
}
fn default_sort() -> String {
    "title".to_string()
}
fn default_tag_mode() -> String {
    "all".to_string()
}

/// Whitelist. O `sort` vem da query string e é concatenado no SQL — só pode
/// virar SQL o que estiver aqui.
fn order_by(sort: &str) -> &'static str {
    match sort {
        "year" => "w.year DESC NULLS LAST, w.title",
        "added" => "w.created_at DESC",
        "recent" => "w.updated_at DESC",
        "duration" => "f.duration_seconds DESC NULLS LAST",
        "random" => "random()",
        // "title" e qualquer coisa não reconhecida
        _ => "COALESCE(s.series_title, w.title), w.season_number NULLS FIRST, \
              w.episode_number NULLS FIRST",
    }
}

pub async fn list(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ListParams>,
) -> AppResult<Json<Vec<WorkListItem>>> {
    let tags: Option<Vec<String>> = params.tags.as_ref().and_then(|raw| {
        let parsed: Vec<String> = raw
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        (!parsed.is_empty()).then_some(parsed)
    });

    let sql = format!(
        r#"
        SELECT {WORK_COLUMNS}, s.series_title
        FROM work w
        {WORK_JOINS}
        WHERE ($2::text IS NULL
               OR w.title ILIKE '%' || $2 || '%'
               OR s.series_title ILIKE '%' || $2 || '%'
               OR w.search_vector @@ websearch_to_tsquery('simple', $2))
          AND ($3::text IS NULL OR w.kind = $3)
          AND ($6::text[] IS NULL OR (
                SELECT count(DISTINCT t.namespace || ':' || t.value)
                FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
                WHERE wt.work_id = w.id
                  AND (t.namespace || ':' || t.value) = ANY($6)
              ) >= CASE WHEN $7 THEN cardinality($6) ELSE 1 END)
          AND ($8::int  IS NULL OR w.year >= $8)
          AND ($9::int  IS NULL OR w.year <= $9)
          AND ($10::int IS NULL OR f.duration_seconds >= $10 * 60)
          AND ($11::int IS NULL OR f.duration_seconds <= $11 * 60)
          AND ($12::uuid IS NULL OR EXISTS (
                SELECT 1 FROM collection_item ci
                WHERE ci.work_id = w.id
                  AND ci.collection_id IN (
                    WITH RECURSIVE subtree AS (
                        SELECT id FROM collection WHERE id = $12
                        UNION ALL
                        SELECT c.id FROM collection c JOIN subtree ON c.parent_id = subtree.id
                    ) SELECT id FROM subtree
                  )
              ))
          AND ($13::text IS NULL OR w.match_state = $13)
          AND ($14::uuid IS NULL OR EXISTS (
                SELECT 1 FROM credit c
                WHERE c.work_id = w.id AND c.person_id = $14
                  AND ($15::text IS NULL OR c.role = $15)
              ))
        ORDER BY {}
        LIMIT $4 OFFSET $5
        "#,
        order_by(&params.sort)
    );

    let items = sqlx::query_as::<_, WorkListItem>(&sql)
        .bind(user.id())
        .bind(params.q.filter(|s| !s.trim().is_empty()))
        .bind(params.kind)
        .bind(params.limit.clamp(1, 500))
        .bind(params.offset.max(0))
        .bind(tags)
        .bind(params.tag_mode != "any")
        .bind(params.year_from)
        .bind(params.year_to)
        .bind(params.min_minutes)
        .bind(params.max_minutes)
        .bind(params.collection)
        .bind(params.state)
        .bind(params.person)
        .bind(params.person_role)
        .fetch_all(&state.pool)
        .await?;

    Ok(Json(items))
}

pub async fn continue_watching(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Vec<WorkListItem>>> {
    let sql = format!(
        r#"
        SELECT {WORK_COLUMNS}, s.series_title
        FROM work w
        {WORK_JOINS}
        WHERE ps.user_id = $1 AND ps.position_seconds > 30 AND NOT ps.finished
        ORDER BY ps.updated_at DESC
        LIMIT 30
        "#
    );

    let items = sqlx::query_as::<_, WorkListItem>(&sql)
        .bind(user.id())
        .fetch_all(&state.pool)
        .await?;
    Ok(Json(items))
}

pub async fn detail(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<WorkDetail>> {
    let work = sqlx::query_as::<_, Work>("SELECT * FROM work WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let files = sqlx::query_as::<_, MediaFileSummary>(
        "SELECT id, path, filename, size_bytes, container, duration_seconds, bitrate,
                video_codec, width, height, frame_rate, audio_codec, audio_channels,
                subtitle_langs, status
         FROM media_file WHERE work_id = $1 ORDER BY size_bytes DESC",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;

    let state_row: Option<(f64, bool)> = sqlx::query_as(
        "SELECT position_seconds, finished FROM playback_state WHERE user_id = $1 AND work_id = $2",
    )
    .bind(user.id())
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;

    let (position_seconds, finished) = state_row.unwrap_or((0.0, false));

    Ok(Json(WorkDetail {
        work,
        files,
        position_seconds,
        finished,
        tags: graph::tags_of(&state, id).await?,
        credits: crate::routes::people::credits_of(&state, id).await?,
        collections: graph::collections_of(&state, id).await?,
        relations: graph::relations_of(&state, id).await?,
    }))
}

pub async fn progress(
    State(state): State<AppState>,
    user: AuthUser,
    Path(id): Path<Uuid>,
    Json(body): Json<ProgressReport>,
) -> AppResult<Json<Value>> {
    if body.position_seconds < 0.0 || !body.position_seconds.is_finite() {
        return Err(AppError::BadRequest("position_seconds inválido".into()));
    }

    let mut tx = state.pool.begin().await?;

    // 1. O log cru. Fonte da verdade, nunca sobrescrito.
    sqlx::query(
        "INSERT INTO play_event
            (user_id, work_id, media_file_id, event_type, position_seconds, duration_seconds, client)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(user.id())
    .bind(id)
    .bind(body.media_file_id)
    .bind(&body.event_type)
    .bind(body.position_seconds)
    .bind(body.duration_seconds)
    .bind(body.client.as_deref().unwrap_or("unknown"))
    .execute(&mut *tx)
    .await?;

    // 2. O cache derivado, pro "continuar assistindo" ser um SELECT barato.
    let finished = match body.duration_seconds {
        Some(d) if d > 0.0 => body.position_seconds / d >= FINISHED_RATIO,
        _ => false,
    };

    sqlx::query(
        "INSERT INTO playback_state
            (user_id, work_id, position_seconds, duration_seconds, finished, play_count, updated_at)
         VALUES ($1, $2, $3, $4, $5, CASE WHEN $5 THEN 1 ELSE 0 END, now())
         ON CONFLICT (user_id, work_id) DO UPDATE SET
            position_seconds = EXCLUDED.position_seconds,
            duration_seconds = COALESCE(EXCLUDED.duration_seconds, playback_state.duration_seconds),
            finished         = EXCLUDED.finished,
            play_count       = playback_state.play_count
                               + CASE WHEN EXCLUDED.finished AND NOT playback_state.finished
                                      THEN 1 ELSE 0 END,
            updated_at       = now()",
    )
    .bind(user.id())
    .bind(id)
    .bind(body.position_seconds)
    .bind(body.duration_seconds)
    .bind(finished)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    // 3. Avisa os outros aparelhos. Quem emitiu ignora o próprio eco pelo
    //    device_id — sem isso o player brigaria com a própria atualização.
    crate::events::publish(
        &state.events,
        crate::events::AppEvent::Progress {
            work_id: id,
            position_seconds: body.position_seconds,
            duration_seconds: body.duration_seconds,
            finished,
            device_id: body.device_id.unwrap_or_else(|| "desconhecido".to_string()),
        },
    );

    Ok(Json(json!({ "ok": true, "finished": finished })))
}
