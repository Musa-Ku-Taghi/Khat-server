use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use thiserror::Error;
use tokio::task::JoinError;
use tracing::error;

use crate::db::DbError;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Authentication failed")]
    Unauthorized,

    #[error("Resource not found")]
    NotFound,

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("File too large")]
    PayloadTooLarge,

    #[error("Unsupported media type")]
    UnsupportedMediaType,

    #[error("Internal server error")]
    Internal,

    #[error("Task join error")]
    Join,
}

impl From<rusqlite::Error> for AppError {
    fn from(err: rusqlite::Error) -> Self {
        AppError::Database(err.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Io(err.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::Serialization(err.to_string())
    }
}

impl From<JoinError> for AppError {
    fn from(_: JoinError) -> Self {
        AppError::Join
    }
}

impl From<DbError> for AppError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::IccidExists
            | DbError::UsernameExists
            | DbError::UsernameNotFound
            | DbError::PasswordIncorrect
            | DbError::InvalidInput(_)
            | DbError::FileNotFound
            | DbError::FileHashExists
            | DbError::FileTempExpired => AppError::BadRequest(err.to_string()),
            DbError::DatabaseError(_) => AppError::Database(err.to_string()),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match self {
            AppError::Database(_)
            | AppError::Serialization(_)
            | AppError::Io(_)
            | AppError::Join
            | AppError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::UnsupportedMediaType => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        };

        error!("AppError: {}", self);

        let kind = match status {
            StatusCode::UNAUTHORIZED => "unauthorized",
            StatusCode::NOT_FOUND => "not_found",
            StatusCode::BAD_REQUEST => "bad_request",
            StatusCode::PAYLOAD_TOO_LARGE => "payload_too_large",
            StatusCode::UNSUPPORTED_MEDIA_TYPE => "unsupported_media_type",
            _ => "server_error",
        };

        let body = serde_json::json!({
            "type": "error",
            "status": kind,
            "message": self.to_string(),
        });

        (status, axum::Json(body)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
