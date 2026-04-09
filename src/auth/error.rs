use axum::{
    Json,
    http::StatusCode,
    response::{
        IntoResponse,
        Response,
    },
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthServiceError {
    #[error("invalid token")]
    InvalidToken,
    #[error("permission denied")]
    PermissionDenied,
    #[error("invalid route")]
    InvalidRoute,
    #[error("internal service error")]
    ServiceError,
}

impl IntoResponse for AuthServiceError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::InvalidToken => StatusCode::UNAUTHORIZED,
            Self::PermissionDenied => StatusCode::FORBIDDEN,
            Self::InvalidRoute => StatusCode::BAD_REQUEST,
            Self::ServiceError => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = serde_json::json!({ "error": self.to_string() });
        (status, Json(body)).into_response()
    }
}
