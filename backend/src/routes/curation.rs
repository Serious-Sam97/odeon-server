use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::curation::{self, taste::TasteProfile, Recommendation};
use crate::auth::{AdminUser, AuthUser};
use crate::error::{AppError, AppResult};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ForYouParams {
    /// Tempo disponível, em minutos.
    #[serde(default)]
    pub minutes: Option<i32>,
    /// Valor da tag `mood:` — sem o prefixo.
    #[serde(default)]
    pub mood: Option<String>,
    #[serde(default)]
    pub include_finished: bool,
    #[serde(default = "default_limit")]
    pub limit: i64,
}

fn default_limit() -> i64 {
    24
}

#[derive(Debug, Serialize)]
pub struct ForYouResponse {
    pub profile: TasteProfile,
    pub items: Vec<Recommendation>,
    /// Verdadeiro quando não há histórico suficiente — a UI precisa dizer isso
    /// em vez de fingir que a lista é personalizada.
    pub cold_start: bool,
}

pub async fn for_you(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<ForYouParams>,
) -> AppResult<Json<ForYouResponse>> {
    let context = curation::Context {
        minutes: params.minutes.filter(|m| *m > 0),
        mood: params.mood.filter(|m| !m.trim().is_empty()),
        include_finished: params.include_finished,
        limit: params.limit,
    };

    let (profile, items) = curation::recommend(&state.pool, user.id(), &context).await?;

    Ok(Json(ForYouResponse {
        cold_start: profile.is_cold_start(),
        profile,
        items,
    }))
}

/// O perfil sozinho — pra você poder olhar no que o sistema acha que você gosta.
/// Recomendação que não se deixa inspecionar é adivinhação.
/// O perfil, mais a contagem de votos explícitos.
///
/// O `TasteProfile` mede o que veio do comportamento; o ♥/✕ é a outra metade,
/// e é a única que existe antes de alguém terminar alguma coisa. A tela do
/// "para você" decide entre frio, morno e quente com as duas — e sem o voto
/// aqui ela não teria como saber que a calibração já rendeu.
pub async fn taste(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Value>> {
    let perfil = curation::taste::build(&state.pool, user.id()).await?;

    let (curtidas, bloqueadas): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE verdict = 'love'),
                count(*) FILTER (WHERE verdict = 'block')
         FROM work_feedback WHERE user_id = $1",
    )
    .bind(user.id())
    .fetch_one(&state.pool)
    .await?;

    // `AppError` não converte de `serde_json::Error` — e serializar uma struct
    // própria não falha, então o `expect` aqui é honesto.
    let mut v = serde_json::to_value(perfil).expect("TasteProfile serializa");
    if let Some(o) = v.as_object_mut() {
        o.insert("curtidas".into(), json!(curtidas));
        o.insert("bloqueadas".into(), json!(bloqueadas));
    }
    Ok(Json(v))
}

pub async fn similar(
    State(state): State<AppState>,
    user: AuthUser,
    Path(work_id): Path<Uuid>,
) -> AppResult<Json<Vec<Recommendation>>> {
    Ok(Json(
        curation::similar(&state.pool, user.id(), work_id, 12).await?,
    ))
}

pub async fn rebuild(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
) -> AppResult<Json<Value>> {
    if state.embedding.lock().await.running {
        return Ok(Json(
            json!({ "started": false, "reason": "já em andamento" }),
        ));
    }

    let pool = state.pool.clone();
    let status = state.embedding.clone();
    tokio::spawn(async move {
        curation::rebuild(pool, status).await;
    });

    Ok(Json(json!({ "started": true })))
}

pub async fn rebuild_status(State(state): State<AppState>) -> Json<curation::EmbedStatus> {
    Json(state.embedding.lock().await.clone())
}

#[derive(Debug, Deserialize)]
pub struct FeedbackBody {
    /// love | block | later
    pub verdict: String,
}

/// Feedback explícito. Largar no meio pode ser interrupção; "nunca mais me
/// ofereça isso" não tem como ser inferido do comportamento.
pub async fn feedback(
    State(state): State<AppState>,
    user: AuthUser,
    Path(work_id): Path<Uuid>,
    Json(body): Json<FeedbackBody>,
) -> AppResult<Json<Value>> {
    if !matches!(body.verdict.as_str(), "love" | "block" | "later") {
        return Err(AppError::BadRequest(
            "verdict deve ser love, block ou later".into(),
        ));
    }

    sqlx::query(
        "INSERT INTO work_feedback (user_id, work_id, verdict) VALUES ($1, $2, $3)
         ON CONFLICT (user_id, work_id) DO UPDATE SET verdict = EXCLUDED.verdict,
                                                     created_at = now()",
    )
    .bind(user.id())
    .bind(work_id)
    .bind(&body.verdict)
    .execute(&state.pool)
    .await?;

    Ok(Json(json!({ "ok": true, "verdict": body.verdict })))
}

pub async fn clear_feedback(
    State(state): State<AppState>,
    user: AuthUser,
    Path(work_id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    sqlx::query("DELETE FROM work_feedback WHERE user_id = $1 AND work_id = $2")
        .bind(user.id())
        .bind(work_id)
        .execute(&state.pool)
        .await?;
    Ok(Json(json!({ "ok": true })))
}

/// Seis capas pra calibrar o gosto: uma por gênero, nunca votadas.
///
/// Existe porque o ♥/✕ é o **único sinal legível antes de alguém terminar
/// alguma coisa** — e o acervo tinha 0 votos e 0 obras terminadas. Sem isso a
/// tela de recomendação abre pedindo desculpa e não oferece saída.
///
/// A escolha é determinística no dia (`md5(dia || id)`), então a fileira não
/// se rearranja a cada recarga — votar em algo que sumiu da tela é a forma
/// mais rápida de a pessoa parar de votar.
pub async fn calibrar(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<Vec<crate::models::WorkListItem>>> {
    // `WorkListItem` e não `Recommendation`: a fileira só precisa da capa, e
    // `Recommendation` embrulha um score que aqui não existe.
    let itens = sqlx::query_as::<_, crate::models::WorkListItem>(
        r#"
        WITH um_por_genero AS (
            SELECT DISTINCT ON (t.value) t.value AS genero, w.id
            FROM work w
            JOIN work_tag wt ON wt.work_id = w.id
            JOIN tag t ON t.id = wt.tag_id
            WHERE w.kind = 'movie'
              AND t.namespace = 'genre'
              AND w.artwork ? 'poster'
              AND w.match_state IN ('auto', 'confirmed')
              AND EXISTS (SELECT 1 FROM media_file m
                           WHERE m.work_id = w.id AND m.status = 'probed')
              -- já votada não volta: a fileira é pra ganhar sinal novo
              AND NOT EXISTS (SELECT 1 FROM work_feedback f
                               WHERE f.work_id = w.id AND f.user_id = $1)
              AND NOT EXISTS (SELECT 1 FROM playback_state p
                               WHERE p.work_id = w.id AND p.user_id = $1)
            ORDER BY t.value, md5(current_date::text || w.id::text)
        ),
        -- Um filme tem vários gêneros: sem esta segunda passada a mesma capa
        -- aparecia duas vezes na fileira.
        escolhidas AS (
            SELECT DISTINCT ON (id) id FROM um_por_genero ORDER BY id
        )
        SELECT w.id, w.kind, w.title, w.year, w.season_number, w.episode_number,
               w.match_state, w.match_confidence, w.dominant_color,
               w.artwork->>'poster' AS poster,
               w.artwork->>'backdrop' AS backdrop,
               w.artwork->>'still' AS still,
               NULL::text AS series_title,
               f.id AS media_file_id, f.duration_seconds, f.width, f.height,
               f.video_codec, f.audio_codec, f.container, f.size_bytes,
               NULL::float8 AS position_seconds, NULL::bool AS finished,
               NULL::text[] AS tags
        FROM escolhidas e
        JOIN work w ON w.id = e.id
        JOIN LATERAL (
            SELECT m.* FROM media_file m
            WHERE m.work_id = w.id AND m.status = 'probed'
            ORDER BY m.size_bytes DESC LIMIT 1
        ) f ON true
        ORDER BY md5(current_date::text || w.id::text)
        LIMIT 6
        "#,
    )
    .bind(user.id())
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(itens))
}
