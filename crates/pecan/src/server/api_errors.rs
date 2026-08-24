//! Typed API errors with HTTP mapping.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pecan_core::CoreError;

/// An error that renders as a JSON problem document.
#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    /// 500 for domain failures.
    pub(crate) fn internal(error: impl std::fmt::Display) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: error.to_string() }
    }

    /// 400 for malformed client input.
    pub(crate) fn bad_request(message: String) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message }
    }

    /// 404 when a referenced resource is absent.
    pub(crate) fn not_found(message: &str) -> Self {
        Self { status: StatusCode::NOT_FOUND, message: message.to_owned() }
    }

    /// 409 when the action contradicts current agent state.
    pub(crate) fn conflict(message: &str) -> Self {
        Self { status: StatusCode::CONFLICT, message: message.to_owned() }
    }

    /// 401 when a credential-bearing action lacks its launch capability.
    pub(crate) fn unauthorized(message: &str) -> Self {
        Self { status: StatusCode::UNAUTHORIZED, message: message.to_owned() }
    }

    /// 502 when the pi worker misbehaves.
    pub(crate) fn bad_gateway(message: String) -> Self {
        Self { status: StatusCode::BAD_GATEWAY, message }
    }
}

impl From<CoreError> for ApiError {
    fn from(error: CoreError) -> Self {
        Self::internal(error)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.message });
        (self.status, axum::Json(body)).into_response()
    }
}
