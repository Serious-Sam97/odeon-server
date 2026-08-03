use axum::extract::{Path, Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AdminUser;
use crate::error::{AppError, AppResult};
use crate::scrub::{self, SpriteInfo};
use crate::AppState;

#[derive(Debug, Deserialize, Default)]
pub struct ScrubRequest {
    #[serde(default)]
    pub force: bool,
}

pub async fn start(
    State(state): State<AppState>,
    AdminUser(_): AdminUser,
    Query(params): Query<ScrubRequest>,
) -> AppResult<Json<Value>> {
    if state.scrub.lock().await.running {
        return Ok(Json(
            json!({ "started": false, "reason": "geração já em andamento" }),
        ));
    }

    let pool = state.pool.clone();
    let dir = state.config.scrub_dir.clone();
    let status = state.scrub.clone();
    let bus = state.events.clone();

    let job = crate::jobs::Job::start(
        &state.pool,
        "scrub",
        json!({ "force": params.force }),
        None,
    )
    .await;
    let job_id = job.as_ref().map(|j| j.id);

    tokio::spawn(async move {
        scrub::generate_all(pool, dir, status.clone(), params.force, job).await;
        let finished = status.lock().await.clone();
        crate::events::publish(
            &bus,
            crate::events::AppEvent::ScrubFinished {
                done: finished.done,
                failed: finished.failed,
            },
        );
    });

    Ok(Json(json!({ "started": true, "force": params.force, "job_id": job_id })))
}

/// Formato inalterado; sobrevive ao restart lendo o último job.
pub async fn status(State(state): State<AppState>) -> Json<scrub::ScrubStatus> {
    let current = state.scrub.lock().await.clone();
    if current.started_at.is_some() {
        return Json(current);
    }
    if let Some(job) = crate::jobs::latest(&state.pool, "scrub").await {
        if let Ok(mut anterior) = serde_json::from_value::<scrub::ScrubStatus>(job.progress) {
            anterior.running = false;
            return Json(anterior);
        }
    }
    Json(current)
}

/// Geometria da folha de sprites. O player usa isto pra calcular a célula:
/// `índice = floor(tempo / interval)`, `x = índice % columns`, `y = índice / columns`.
/// A folha de sprites de um arquivo.
///
/// Gated na R26 (§42) pelo mesmo motivo do menu: a folha **é o filme inteiro em
/// miniatura**. Servi-la a quem não pegou a caixa seria entregar o conteúdo em
/// resolução baixa e chamar de metadado.
pub async fn info(
    State(state): State<AppState>,
    crate::auth::AuthUser(user): crate::auth::AuthUser,
    Path(media_file_id): Path<Uuid>,
) -> AppResult<Json<SpriteInfo>> {
    if !crate::auth::acesso::pode_assistir(&state.pool, &user, media_file_id).await {
        return Err(crate::auth::acesso::negado());
    }

    sqlx::query_as::<_, SpriteInfo>(
        "SELECT media_file_id, path, interval_seconds, columns, rows,
                thumb_width, thumb_height, frame_count
         FROM scrub_sprite WHERE media_file_id = $1",
    )
    .bind(media_file_id)
    .fetch_optional(&state.pool)
    .await?
    .map(Json)
    .ok_or(AppError::NotFound)
}
