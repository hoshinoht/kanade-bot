//! The NEXON Open API (module `character_data`, plain HTTP through
//! `Overrides::nexon`): every call answers an empty object, which the client
//! reads as "no such character".

use axum::{Json, Router};
use serde_json::{Value, json};

pub fn router() -> Router {
    Router::new().fallback(|| async { Json::<Value>(json!({})) })
}
