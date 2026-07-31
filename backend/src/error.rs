use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("não encontrado")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("não autenticado")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "não encontrado".to_string()),
            AppError::Db(sqlx::Error::RowNotFound) => {
                (StatusCode::NOT_FOUND, "não encontrado".to_string())
            }
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "credenciais inválidas ou sessão expirada".to_string(),
            ),
            AppError::Forbidden(m) => (StatusCode::FORBIDDEN, m.clone()),
            other => {
                tracing::error!(error = %other, "erro interno");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "erro interno".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
