//! Request body and time bounds. Declared oversize bodies are refused before
//! any handler runs; `DefaultBodyLimit` bounds chunked bodies at extraction.

use std::{sync::Arc, time::Duration};

use axum::{
    extract::{Request, State},
    http::header::CONTENT_LENGTH,
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::api::{error::ApiError, listeners::Site};

/// PWA request bodies are small JSON documents.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub body_bytes: usize,
    pub request_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            body_bytes: MAX_BODY_BYTES,
            request_timeout: REQUEST_TIMEOUT,
        }
    }
}

pub async fn enforce(State(site): State<Arc<Site>>, request: Request, next: Next) -> Response {
    let declared = request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if declared.is_some_and(|length| length > site.limits.body_bytes as u64) {
        return ApiError::PAYLOAD_TOO_LARGE.into_response();
    }
    tokio::time::timeout(site.limits.request_timeout, next.run(request))
        .await
        .unwrap_or_else(|_| ApiError::TIMEOUT.into_response())
}
