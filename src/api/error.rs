use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Wire error `{error, message}`: a stable code plus generic, value-free text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub error: &'static str,
    pub message: &'static str,
}

#[derive(Serialize)]
struct Body {
    error: &'static str,
    message: &'static str,
}

impl ApiError {
    pub const NOT_FOUND: Self = Self {
        status: StatusCode::NOT_FOUND,
        error: "not_found",
        message: "No such endpoint on this origin.",
    };
    pub const METHOD_NOT_ALLOWED: Self = Self {
        status: StatusCode::METHOD_NOT_ALLOWED,
        error: "method_not_allowed",
        message: "That method is not allowed here.",
    };
    pub const MISDIRECTED: Self = Self {
        status: StatusCode::MISDIRECTED_REQUEST,
        error: "misdirected",
        message: "This origin does not serve that host.",
    };
    pub const PAYLOAD_TOO_LARGE: Self = Self {
        status: StatusCode::PAYLOAD_TOO_LARGE,
        error: "payload_too_large",
        message: "The request body is too large.",
    };
    pub const TIMEOUT: Self = Self {
        status: StatusCode::SERVICE_UNAVAILABLE,
        error: "timeout",
        message: "The request took too long.",
    };
    /// Public origin while the portal is closed: data and art answer this.
    pub const CLOSED: Self = Self {
        status: StatusCode::SERVICE_UNAVAILABLE,
        error: "closed",
        message: "The schedule is not public right now.",
    };
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(Body {
                error: self.error,
                message: self.message,
            }),
        )
            .into_response()
    }
}

pub async fn not_found() -> ApiError {
    ApiError::NOT_FOUND
}

pub async fn method_not_allowed() -> ApiError {
    ApiError::METHOD_NOT_ALLOWED
}

pub async fn closed() -> ApiError {
    ApiError::CLOSED
}
