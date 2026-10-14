//! The Kanata model gateway (`KANADE_MODEL_BASE_URL`): no models listed, and
//! every completion an empty assistant reply that stopped normally.

use axum::{
    Json, Router,
    routing::{get, post},
};
use serde_json::{Value, json};

pub fn router() -> Router {
    Router::new()
        .route(
            "/v1/models",
            get(|| async { Json(json!({"object": "list", "data": []})) }),
        )
        .route("/v1/chat/completions", post(complete))
}

async fn complete(Json(request): Json<Value>) -> Json<Value> {
    Json(json!({
        "id": "e2e-completion",
        "object": "chat.completion",
        "model": request.get("model").cloned().unwrap_or(Value::Null),
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": ""},
            "finish_reason": "stop",
        }],
        "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
    }))
}
