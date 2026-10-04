//! Public-origin routes. While the portal is closed this origin serves only
//! the shell, status and identity; data and art answer `closed`. Member OAuth
//! and data routes stay unmounted until public exposure is authorized.

use std::sync::Arc;

use axum::{Json, Router, routing::any, routing::get};
use serde::Serialize;

use super::{error, listeners::Site};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/public/status", get(status))
        .route("/api/public/{*rest}", any(error::closed))
        .route("/art/{*rest}", any(error::closed))
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct PublicStatus {
    #[cfg_attr(test, ts(type = "'open' | 'closed'"))]
    portal: &'static str,
}

async fn status() -> Json<PublicStatus> {
    Json(PublicStatus { portal: "closed" })
}
