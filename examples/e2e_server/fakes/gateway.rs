//! The model gateway (`KANADE_MODEL_BASE_URL`): Kanata by default, the
//! mock's model catalog (`config::catalog`) with its Kanata metadata, and
//! completions that stopped normally: a header or nudge rewrite answers its
//! seed line unchanged (the rewrite gate accepts it, so it is stored) after
//! [`REWRITE_DELAY`], an extraction answers nothing after
//! [`EXTRACT_DELAY`] (so a rescan is seen running), anything else answers
//! nothing at once. [`Mode`] (`/__e2e/gateway`) turns it into a plain
//! OpenAI-compatible server: the same ids without `kanata` objects, or no
//! model list at all (`/v1/models` 404); the server's capability
//! declarations ([`declarations`]) then carry chat and extraction. Also the
//! governor's view the Limits page shows (mock `limits::groups`), canned:
//! the harness runs too few model calls to fill a real one.

use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::Instant,
};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
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

/// When completions were last asked for, how many are in flight, and how
/// `/v1/models` answers.
#[derive(Default)]
pub struct Activity {
    in_flight: AtomicUsize,
    last: Mutex<Option<Instant>>,
    mode: AtomicU8,
}

/// How the gateway lists its models (`POST /__e2e/gateway {mode}`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Kanata: every model with its `kanata` metadata (the default).
    Kanata,
    /// A plain OpenAI-compatible server: the same ids, no metadata.
    Plain,
    /// A server without a model list: `/v1/models` answers 404.
    NoListing,
}

impl Mode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "kanata" => Some(Self::Kanata),
            "plain" => Some(Self::Plain),
            "no_listing" => Some(Self::NoListing),
            _ => None,
        }
    }
}

impl Activity {
    fn touch(&self) {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
    }

    pub fn set_mode(&self, mode: Mode) {
        self.mode.store(mode as u8, Ordering::SeqCst);
    }

    fn mode(&self) -> Mode {
        match self.mode.load(Ordering::SeqCst) {
            1 => Mode::Plain,
            2 => Mode::NoListing,
            _ => Mode::Kanata,
        }
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
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(complete))
        .with_state(activity)
}

async fn models(State(activity): State<Arc<Activity>>) -> Response {
    let listed = |kanata: bool| {
        json!({
            "object": "list",
            "data": catalog()
                .into_iter()
                .map(|(id, meta)| if kanata {
                    json!({"id": id, "object": "model", "kanata": meta})
                } else {
                    json!({"id": id, "object": "model", "owned_by": "e2e"})
                })
                .collect::<Vec<_>>(),
        })
    };
    match activity.mode() {
        Mode::Kanata => Json(listed(true)).into_response(),
        Mode::Plain => Json(listed(false)).into_response(),
        Mode::NoListing => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"message": "Not Found", "type": "invalid_request_error"}})),
        )
            .into_response(),
    }
}

/// The server's `KANADE_MODEL_CAPABILITIES`: every catalog model's
/// declarable metadata (no trust zone or admission). Kanata's published
/// metadata wins over it, so it only matters in the plain modes.
pub fn declarations() -> String {
    const DECLARABLE: [&str; 6] = [
        "context_tokens",
        "function_tools",
        "max_output_tokens",
        "reasoning_control",
        "sampling_controls",
        "structured_output",
    ];
    let declared: serde_json::Map<String, Value> = catalog()
        .into_iter()
        .map(|(id, meta)| {
            let mut fields: serde_json::Map<String, Value> = DECLARABLE
                .iter()
                .filter_map(|&field| Some((field.to_owned(), meta.get(field)?.clone())))
                .collect();
            // Declared lists must be non-empty and need reasoning control.
            if meta["reasoning_control"] == true
                && let Some(efforts) = meta["reasoning_efforts"].as_array()
                && !efforts.is_empty()
            {
                fields.insert("reasoning_efforts".into(), Value::Array(efforts.clone()));
            }
            (id.to_owned(), Value::Object(fields))
        })
        .collect();
    Value::Object(declared).to_string()
}

/// Each model id with its Kanata metadata.
fn catalog() -> [(&'static str, Value); 10] {
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
    [
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
    ]
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

/// The governor's groups at `now`. By default the one `gateway` group: its
/// permit taken, a rescan and an extraction burst queued behind it, the
/// breaker closed for ten hours. With `three` (`/__e2e/limits`), the mock's
/// declared cloud, local and legacy groups side by side: full and queueing,
/// half-open, open (mock `limits::groups`).
pub fn limits(now: DateTime<Utc>, three: bool) -> Vec<GroupSnapshot> {
    let queued = |position, kind, who: &str, waiting_s| QueuedCall {
        position,
        kind,
        who: who.into(),
        waiting_s,
    };
    let breaker = |state, failures, minutes: i64, retry_at| BreakerView {
        state,
        failures,
        since: now - Duration::minutes(minutes),
        retry_at,
    };
    let group =
        |name: &str, models: &[&str], (in_use, total), queue, rate, retry, breaker| GroupSnapshot {
            name: name.into(),
            backend: "Kanata".into(),
            models: models.iter().map(|model| (*model).to_owned()).collect(),
            permits: PermitUsage { in_use, total },
            queue,
            holders: Vec::new(),
            rate,
            retry,
            breaker,
            counters: Counters::default(),
        };
    let busy_queue = || {
        vec![
            queued(1, CallKind::Rescan, "admin token", 42),
            queued(2, CallKind::Extraction, "#hstar-party burst", 8),
        ]
    };
    let busy_rate = RateLevel {
        available: 9,
        capacity: 12,
        refill_per_min: 2,
    };
    let full_retry = RetryLevel {
        remaining: 5,
        capacity: 5,
    };
    if !three {
        return vec![group(
            "gateway",
            &["kanata/extract", "kanata/chat", "kanata/rewrite-small"],
            (1, 1),
            busy_queue(),
            busy_rate,
            full_retry,
            breaker(BreakerState::Closed, 0, 600, None),
        )];
    }
    vec![
        group(
            "cloud",
            &["kanata/chat-cloud", "kanata/rewrite-cloud"],
            (2, 2),
            busy_queue(),
            busy_rate,
            full_retry,
            breaker(BreakerState::Closed, 0, 600, None),
        ),
        group(
            "local",
            &["kanata/chat"],
            (2, 4),
            Vec::new(),
            RateLevel {
                available: 1,
                capacity: 20,
                refill_per_min: 4,
            },
            RetryLevel {
                remaining: 2,
                capacity: 5,
            },
            breaker(BreakerState::HalfOpen, 3, 4, None),
        ),
        group(
            "legacy",
            &["kanata/legacy"],
            (0, 2),
            Vec::new(),
            RateLevel {
                available: 6,
                capacity: 6,
                refill_per_min: 1,
            },
            RetryLevel {
                remaining: 0,
                capacity: 3,
            },
            breaker(BreakerState::Open, 5, 12, Some(now + Duration::minutes(3))),
        ),
    ]
}
