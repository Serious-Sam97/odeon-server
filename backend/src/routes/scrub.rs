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

    tokio::spawn(async move {
        scrub::generate_all(pool, dir, status.clone(), params.force).await;
        let finished = status.lock().await.clone();
        crate::events::publish(
            &bus,
            crate::events::AppEvent::ScrubFinished {
                done: finished.done,
                failed: finished.failed,
            },
        );
    });

    Ok(Json(json!({ "started": true, "force": params.force })))
}

pub async fn status(State(state): State<AppState>) -> Json<scrub::ScrubStatus> {
    Json(state.scrub.lock().await.clone())
}

/// Geometria da folha de sprites. O player usa isto pra calcular a célula:
/// `índice = floor(tempo / interval)`, `x = índice % columns`, `y = índice / columns`.
pub async fn info(
    State(state): State<AppState>,
    Path(media_file_id): Path<Uuid>,
) -> AppResult<Json<SpriteInfo>> {
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
