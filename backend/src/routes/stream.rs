//! Direct Play.
//!
//! O M0 não transcodifica nada: serve o arquivo original com suporte a HTTP
//! Range (seek). Como o acesso é via Tailscale nos seus próprios aparelhos, a
//! banda existe e os codecs são conhecidos — Direct Play cobre a esmagadora
//! maioria dos casos. Transcode é o M6, e é de propósito: é o maior sumidouro
//! de complexidade do projeto inteiro.

use axum::extract::{Path, Request, State};
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower_http::services::ServeFile;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::AppState;

pub async fn stream(
    State(state): State<AppState>,
    Path(media_file_id): Path<Uuid>,
    request: Request,
) -> AppResult<Response> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT path, status FROM media_file WHERE id = $1",
    )
    .bind(media_file_id)
    .fetch_optional(&state.pool)
    .await?;

    let (path, status) = row.ok_or(AppError::NotFound)?;

    if status == "missing" {
        return Err(AppError::BadRequest(
            "arquivo marcado como sumido — rode um scan".into(),
        ));
    }

    // ServeFile já implementa Range, If-Range, HEAD e Content-Type.
    let response = ServeFile::new(&path)
        .oneshot(request)
        .await
        .map_err(|e| AppError::Other(anyhow::anyhow!("falha ao servir arquivo: {e}")))?;

    Ok(response.map(axum::body::Body::new).into_response())
}
