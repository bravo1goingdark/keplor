//! Server error types.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Errors produced by the ingestion pipeline and HTTP layer.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// A required field is missing or invalid.
    #[error("validation: {0}")]
    Validation(String),

    /// The provider string could not be recognised.
    #[error("unknown provider: {0}")]
    UnknownProvider(String),

    /// Timestamp parsing failed.
    #[error("invalid timestamp: {0}")]
    InvalidTimestamp(String),

    /// Storage layer error.
    #[error("store: {0}")]
    Store(#[from] keplor_store::StoreError),

    /// JSON deserialization failed.
    #[error("json: {0}")]
    Json(String),

    /// Internal error.
    #[error("internal: {0}")]
    Internal(String),
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            Self::Validation(_) | Self::InvalidTimestamp(_) | Self::Json(_) => {
                (StatusCode::BAD_REQUEST, self.to_string())
            },
            Self::UnknownProvider(_) => (StatusCode::UNPROCESSABLE_ENTITY, self.to_string()),
            Self::Store(
                keplor_store::StoreError::ChannelFull | keplor_store::StoreError::ChannelClosed,
            ) => {
                tracing::warn!(error = %self, "back-pressure: batch writer unavailable");
                (StatusCode::SERVICE_UNAVAILABLE, self.to_string())
            },
            Self::Store(_) | Self::Internal(_) => {
                tracing::error!(error = %self, "internal server error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_owned())
            },
        };

        let body = serde_json::json!({ "error": msg });
        (status, axum::Json(body)).into_response()
    }
}
