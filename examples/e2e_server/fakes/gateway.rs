//! The Kanata model gateway (`KANADE_MODEL_BASE_URL`): the mock's model
//! catalog (`config::catalog`) with its Kanata metadata, and completions
//! that stopped normally: a header or nudge rewrite answers its seed line
//! unchanged (the rewrite gate accepts it, so it is stored) after
//! [`REWRITE_DELAY`], an extraction answers nothing after
//! [`EXTRACT_DELAY`] (so a rescan is seen running), anything else answers
//! nothing at once. Also the governor's view the Limits page shows (mock
//! `limits::groups`), canned: the harness runs too few model calls to fill
//! a real one.

use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use chrono::{DateTime, Duration, Utc};
use kanade::infrastructure::llm::governor::{
    BreakerState, BreakerView, CallKind, Counters, GroupSnapshot, PermitUsage, QueuedCall,
    RateLevel, RetryLevel,
};
use serde_json::{Value, json};

/// How long a rewrite takes: a manual header rewrite of a few headers is
/// still running when a second one is asked for.
const REWRITE_DELAY: std::time::Duration = std::time::Duration::from_millis(750);
/// How long one extraction call takes.
const EXTRACT_DELAY: std::time::Duration = std::time::Duration::from_millis(300);
/// What the rewrite prompt puts before the seed (`chat::nudge::prompt`).
const SEED_LABEL: &str = "Line to rewrite: ";

/// When completions were last asked for, and how many are in flight.
#[derive(Default)]
pub struct Activity {
    in_flight: AtomicUsize,
    last: Mutex<Option<Instant>>,
}

impl Activity {
    fn touch(&self) {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
    }

    /// Resolves once no completion has been in flight or asked for during
    /// `quiet` (waiting at least that long).
    pub async fn settled(&self, quiet: std::time::Duration) {
        let since = Instant::now();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let last = (*self.last.lock().unwrap_or_else(PoisonError::into_inner)).unwrap_or(since);
            if self.in_flight.load(Ordering::SeqCst) == 0 && last.max(since).elapsed() >= quiet {
                return;
            }
        }
    }
}

/// One completion in flight, counted until dropped.
struct InFlight<'a>(&'a Activity);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.touch();
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn router(activity: Arc<Activity>) -> Router {
    Router::new()
        .route("/v1/models", get(|| async { Json(catalog()) }))
        .route("/v1/chat/completions", post(complete))
        .with_state(activity)
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

async fn complete(
    State(activity): State<Arc<Activity>>,
    Json(request): Json<Value>,
) -> Json<Value> {
    activity.in_flight.fetch_add(1, Ordering::SeqCst);
    activity.touch();
    let _counted = InFlight(&activity);
    let model = request.get("model").and_then(Value::as_str).unwrap_or("");
    let seed = request
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|messages| {
            messages
                .iter()
                .rev()
                .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        })
        .and_then(|message| message.get("content").and_then(Value::as_str))
        .and_then(|content| content.strip_prefix(SEED_LABEL));
    let content = match seed {
        Some(seed) => {
            tokio::time::sleep(REWRITE_DELAY).await;
            seed.to_owned()
        }
        None => {
            if model.starts_with("kanata/extract") {
                tokio::time::sleep(EXTRACT_DELAY).await;
            }
            String::new()
        }
    };
    Json(json!({
        "id": "e2e-completion",
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": content},
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
