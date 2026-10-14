//! The Kanata model gateway (`KANADE_MODEL_BASE_URL`): the mock's model
//! catalog (`config::catalog`) with its Kanata metadata, and every
//! completion an empty assistant reply that stopped normally. Also the
//! governor's view the Limits page shows (mock `limits::groups`), canned:
//! the harness runs no model calls to fill a real one.

use axum::{
    Json, Router,
    routing::{get, post},
};
use chrono::{DateTime, Duration, Utc};
use kanade::infrastructure::llm::governor::{
    BreakerState, BreakerView, CallKind, Counters, GroupSnapshot, PermitUsage, QueuedCall,
    RateLevel, RetryLevel,
};
use serde_json::{Value, json};

pub fn router() -> Router {
    Router::new()
        .route("/v1/models", get(|| async { Json(catalog()) }))
        .route("/v1/chat/completions", post(complete))
}

fn catalog() -> Value {
    let chat = json!({
        "admission": {"max_in_flight": 4, "adapter_max_in_flight": 8},
        "function_tools": true, "max_output_tokens": 8192,
        "reasoning_control": true, "reasoning_efforts": ["low", "medium"],
        "sampling_controls": true, "structured_output": true, "trust_zone": "local",
    });
    let extract = json!({
        "admission": {"max_in_flight": 1, "adapter_max_in_flight": 2},
        "context_tokens": 16_384, "function_tools": false, "max_output_tokens": 4096,
        "reasoning_control": true, "reasoning_efforts": ["low", "medium", "high"],
        "sampling_controls": true, "structured_output": true, "trust_zone": "local",
    });
    let models = [
        ("kanata/chat", chat.clone()),
        (
            "kanata/chat-cloud",
            json!({
                "admission": {"max_in_flight": 4, "adapter_max_in_flight": 8},
                "context_tokens": 200_000, "function_tools": true, "max_output_tokens": 16_384,
                "reasoning_control": true,
                "reasoning_efforts": ["minimal", "low", "medium", "high"],
                "sampling_controls": false, "structured_output": true, "trust_zone": "external",
            }),
        ),
        ("kanata/chat:high", chat.clone()),
        ("kanata/chat:low", chat),
        ("kanata/extract", extract.clone()),
        ("kanata/extract:none", extract),
        (
            "kanata/legacy",
            json!({
                "admission": {"max_in_flight": 2},
                "function_tools": false, "reasoning_control": true,
                "sampling_controls": true, "structured_output": true,
            }),
        ),
        (
            "kanata/rewrite-cloud",
            json!({
                "admission": {"max_in_flight": 2, "adapter_max_in_flight": 4},
                "context_tokens": 32_768, "function_tools": false, "max_output_tokens": 1024,
                "reasoning_control": false, "reasoning_efforts": [],
                "sampling_controls": true, "structured_output": false,
                "trust_zone": "private_network",
            }),
        ),
        (
            "kanata/rewrite-small",
            json!({
                "admission": {"max_in_flight": 1},
                "context_tokens": 8192, "function_tools": false, "max_output_tokens": 512,
                "reasoning_control": false, "reasoning_efforts": [],
                "sampling_controls": true, "structured_output": false, "trust_zone": "local",
            }),
        ),
        (
            "kanata/think",
            json!({
                "admission": {"max_in_flight": 1},
                "context_tokens": 65_536, "function_tools": true, "max_output_tokens": 8192,
                "reasoning_control": true, "reasoning_efforts": ["low", "high"],
                "sampling_controls": true, "structured_output": true, "trust_zone": "local",
            }),
        ),
    ];
    json!({
        "object": "list",
        "data": models
            .into_iter()
            .map(|(id, kanata)| json!({"id": id, "object": "model", "kanata": kanata}))
            .collect::<Vec<_>>(),
    })
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

/// The one capacity group at `now`: its permit taken, a rescan and an
/// extraction burst queued behind it, the breaker closed for ten hours.
pub fn limits(now: DateTime<Utc>) -> Vec<GroupSnapshot> {
    let queued = |position, kind, who: &str, waiting_s| QueuedCall {
        position,
        kind,
        who: who.into(),
        waiting_s,
    };
    vec![GroupSnapshot {
        name: "gateway".into(),
        backend: "Kanata".into(),
        models: ["kanata/extract", "kanata/chat", "kanata/rewrite-small"]
            .map(str::to_owned)
            .to_vec(),
        permits: PermitUsage {
            in_use: 1,
            total: 1,
        },
        queue: vec![
            queued(1, CallKind::Rescan, "admin token", 42),
            queued(2, CallKind::Extraction, "#hstar-party burst", 8),
        ],
        holders: Vec::new(),
        rate: RateLevel {
            available: 9,
            capacity: 12,
            refill_per_min: 2,
        },
        retry: RetryLevel {
            remaining: 5,
            capacity: 5,
        },
        breaker: BreakerView {
            state: BreakerState::Closed,
            failures: 0,
            since: now - Duration::hours(10),
            retry_at: None,
        },
        counters: Counters::default(),
    }]
}
