//! The scouter sidecar (`KANADE_SCOUTER_URL`): healthy, and a lookup finds
//! the character with no boss cards.

use axum::{Json, Router, extract::RawQuery, routing::get};
use serde_json::{Value, json};

pub fn router() -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/bosses", get(bosses))
}

/// `ign=` answers the one character, `igns=` a batch with no combined estimate.
async fn bosses(RawQuery(query): RawQuery) -> Json<Value> {
    match query
        .as_deref()
        .and_then(|query| query.strip_prefix("ign="))
    {
        Some(ign) => Json(json!({"ign": ign, "preset": "00010", "bosses": {}})),
        None => Json(json!({"combined": {}})),
    }
}
