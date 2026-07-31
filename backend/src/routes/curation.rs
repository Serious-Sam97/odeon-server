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
pub async fn taste(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Json<TasteProfile>> {
    Ok(Json(
        curation::taste::build(&state.pool, user.id()).await?,
    ))
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
