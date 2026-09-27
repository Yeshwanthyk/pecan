//! Typed API errors with HTTP mapping.

use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use pecan_core::CoreError;

use super::worker::WorkerError;

/// An error that renders as a JSON problem document.
#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    message: String,
    /// Stable machine-readable reason the web client branches on.
    code: Option<&'static str>,
}

impl ApiError {
    /// 500 for domain failures.
    pub(crate) fn internal(error: impl std::fmt::Display) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }

    /// 400 for malformed client input.
    pub(crate) fn bad_request(message: String) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    /// 404 when a referenced resource is absent.
    pub(crate) fn not_found(message: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, message.to_owned())
    }

    /// 409 when the action contradicts current agent state.
    pub(crate) fn conflict(message: &str) -> Self {
        Self::new(StatusCode::CONFLICT, message.to_owned())
    }

    /// 401 when a credential-bearing action lacks its launch capability.
    pub(crate) fn unauthorized(message: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, message.to_owned())
    }

    /// 401 `unpaired`: this browser has no valid device cookie.
    pub(crate) fn unpaired() -> Self {
        Self::unpaired_with("this device is not paired; run `pecan pair` on the host")
    }

    /// 401 `unpaired` with a specific explanation.
    pub(crate) fn unpaired_with(message: &str) -> Self {
        Self { code: Some("unpaired"), ..Self::unauthorized(message) }
    }

    /// 403 when an authenticated principal may not take this action.
    pub(crate) fn forbidden(message: &str) -> Self {
        Self::new(StatusCode::FORBIDDEN, message.to_owned())
    }

    /// 502 when the pi worker misbehaves.
    pub(crate) fn bad_gateway(message: String) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, message)
    }

    fn new(status: StatusCode, message: String) -> Self {
        Self { status, message, code: None }
    }
}

impl From<CoreError> for ApiError {
    fn from(error: CoreError) -> Self {
        Self::internal(error)
    }
}

impl From<WorkerError> for ApiError {
    fn from(error: WorkerError) -> Self {
        Self::bad_gateway(error.to_string())
    }
}

/// Unwraps a JSON-extracted body, mapping a parse/validation failure to a
/// 400 rather than the 422 axum's [`JsonRejection`] would otherwise render.
///
/// # Errors
/// Returns [`ApiError::bad_request`] when the body is missing, malformed, or
/// fails deserialization.
pub(crate) fn json_body<T>(body: Result<axum::Json<T>, JsonRejection>) -> Result<T, ApiError> {
    body.map(|axum::Json(value)| value).map_err(|error| ApiError::bad_request(error.to_string()))
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = match self.code {
            Some(code) => serde_json::json!({ "error": self.message, "code": code }),
            None => serde_json::json!({ "error": self.message }),
        };
        (self.status, axum::Json(body)).into_response()
    }
}
