//! `GET /api/admin/events` as the server sends it (`ready {seq}`, then
//! `{topic, seq}` hints, no data), emitted when the mock's own state changes:
//! after a successful admin write, or from `POST /__mock/arrive`, which stands
//! in for a change made in Discord. The member portal's `GET /api/public/events`
//! sends `ready {boot}` and `{topic}` hints (`schedule`, `mine`, `allowance`),
//! emitted by `POST /__mock/public/hint`.
//!
//! The mock has no streaming body, so each response is short: the browser's
//! `EventSource` reconnects (`retry`) with `Last-Event-ID`, and the next
//! response holds until a later hint (or a quiet interval) arrives. Pages see
//! the same events in the same order; only the connection churns. The member
//! stream's `id:` lines are that bookkeeping only (the server sends none);
//! its `data` never holds a seq.

use std::{collections::VecDeque, sync::Mutex, time::Duration};

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::watch;

use crate::{
    App,
    mock::{
        dto::{MoveRequest, RsvpRequest},
        history::Actor,
    },
};

/// Hints kept for reconnecting streams; an older `Last-Event-ID` gets `ready`.
const KEPT: usize = 64;
/// How long one response waits for a hint before answering with a heartbeat.
const HOLD: Duration = Duration::from_secs(10);
const RETRY_MS: u32 = 200;

pub struct Hints {
    log: Mutex<VecDeque<(u64, &'static str)>>,
    seq: watch::Sender<u64>,
}

impl Default for Hints {
    fn default() -> Self {
        Self {
            log: Mutex::new(VecDeque::new()),
            seq: watch::channel(0).0,
        }
    }
}

impl Hints {
    pub fn emit(&self, topic: &'static str) {
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        let seq = *self.seq.borrow() + 1;
        log.push_back((seq, topic));
        while log.len() > KEPT {
            log.pop_front();
        }
        self.seq.send_replace(seq);
    }

    /// Hints after `seq`, or `None` when some are no longer kept.
    fn after(&self, seq: u64) -> Option<Vec<(u64, &'static str)>> {
        let log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        if log.front().is_some_and(|(first, _)| *first > seq + 1) {
            return None;
        }
        Some(log.iter().filter(|(at, _)| *at > seq).copied().collect())
    }
}

fn stream(body: String) -> Response {
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

/// This mock process's `boot` id (the server's changes on every restart).
fn boot() -> &'static str {
    static BOOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BOOT.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        format!("mock-{nanos:x}")
    })
}

/// The admin stream's `ready {seq, boot}`, or the member stream's `ready {boot}`.
fn ready(seq: u64, member: bool) -> String {
    let data = if member {
        json!({ "boot": boot() })
    } else {
        json!({ "seq": seq, "boot": boot() })
    };
    format!("retry: {RETRY_MS}\nid: {seq}\nevent: ready\ndata: {data}\n\n")
}

pub async fn events(State(app): State<App>, headers: HeaderMap) -> Response {
    stream(answer(&app.hints, &headers, false).await)
}

/// `GET /api/public/events`: the member session's stream, behind the quiet
/// session check (no rotation, no touch), as on the server.
pub async fn member_events(State(app): State<App>, headers: HeaderMap) -> Response {
    match crate::public::member_quietly(&app, &headers).await {
        Ok(()) => stream(answer(&app.member_hints, &headers, true).await),
        Err(refused) => *refused,
    }
}

/// One short response: `ready` on a fresh connection, else the hints after
/// `Last-Event-ID` (held until one arrives or [`HOLD`] passes).
async fn answer(hints: &Hints, headers: &HeaderMap, member: bool) -> String {
    let last = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let Some(last) = last else {
        return ready(*hints.seq.borrow(), member);
    };
    let mut seq = hints.seq.subscribe();
    if *seq.borrow_and_update() <= last {
        let _ = tokio::time::timeout(HOLD, seq.changed()).await;
    }
    match hints.after(last) {
        None => ready(*hints.seq.borrow(), member),
        Some(list) if list.is_empty() => format!("retry: {RETRY_MS}\n: keep-alive\n\n"),
        Some(list) => {
            let mut body = format!("retry: {RETRY_MS}\n");
            for (seq, topic) in list {
                let data = if member {
                    json!({ "topic": topic })
                } else {
                    json!({ "topic": topic, "seq": seq })
                };
                body.push_str(&format!("id: {seq}\ndata: {data}\n\n"));
            }
            body
        }
    }
}

/// What a successful admin write changed, as the server's store hook names it.
fn topics(method: &Method, path: &str) -> &'static [&'static str] {
    if method == Method::GET || method == Method::HEAD {
        return &[];
    }
    let under = |prefix: &str| path.starts_with(prefix);
    if under("/api/admin/inbox/") {
        &["inbox", "schedule"]
    } else if under("/api/admin/runs/") || under("/api/admin/fixed") || under("/api/admin/history/")
    {
        &["schedule"]
    } else if under("/api/admin/config") {
        &["settings"]
    } else if under("/api/admin/digest") {
        &["delivery"]
    } else if under("/api/admin/rescan") {
        &["rescan"]
    } else if under("/api/admin/members/") {
        &["members"]
    } else {
        &[]
    }
}

/// Hints after every successful admin write.
pub async fn after_write(State(app): State<App>, request: Request, next: Next) -> Response {
    let topics = topics(request.method(), request.uri().path());
    let response = next.run(request).await;
    if response.status().is_success() {
        for topic in topics {
            app.hints.emit(topic);
        }
    }
    response
}

#[derive(Deserialize)]
pub struct Arrival {
    kind: String,
}

#[derive(Deserialize)]
pub struct MemberHint {
    topic: String,
}

/// e2e: the member stream hints `topic` (`schedule`, `mine` or `allowance`).
pub async fn member_hint(State(app): State<App>, Json(hint): Json<MemberHint>) -> StatusCode {
    let Some(topic) = ["schedule", "mine", "allowance"]
        .into_iter()
        .find(|topic| *topic == hint.topic)
    else {
        return StatusCode::UNPROCESSABLE_ENTITY;
    };
    app.member_hints.emit(topic);
    StatusCode::NO_CONTENT
}

/// e2e: a change made outside this portal (Discord, the extractor, chat).
pub async fn arrive(State(app): State<App>, Json(arrival): Json<Arrival>) -> Response {
    let mut store = app.store.lock().await;
    let topic = match arrival.kind.as_str() {
        // Ren's ✅ on Kalos.
        "reaction" => {
            let version = store.version();
            let reacted = store.tracked(Actor::new("member", "1005"), "discord", |s| {
                s.rsvp(
                    "r-kalos",
                    RsvpRequest {
                        member_id: "1005".into(),
                        answer: "yes".into(),
                        version,
                    },
                )
            });
            if reacted.is_err() {
                return StatusCode::CONFLICT.into_response();
            }
            "schedule"
        }
        // Another admin moves Hard Limbo from Friday 23:30 to Thursday 21:00.
        "move" => {
            let version = store.version();
            let moved = store.tracked(Actor::new("admin", "discord:1002"), "admin_portal", |s| {
                s.move_run(
                    "r-limbo",
                    MoveRequest {
                        day: 0,
                        time: Some("21:00".into()),
                        version,
                    },
                )
            });
            if moved.is_err() {
                return StatusCode::CONFLICT.into_response();
            }
            "schedule"
        }
        "run" => {
            if store.arrive_run().is_err() {
                return StatusCode::CONFLICT.into_response();
            }
            "schedule"
        }
        "proposal" => {
            store.arrive_proposal();
            "inbox"
        }
        "chat" => {
            store.arrive_chat();
            "chat"
        }
        "extraction" => {
            store.arrive_extraction();
            "extraction"
        }
        // Mika's roster sync brings a new alias.
        "member" => {
            if store.add_alias("1003", "mikan").is_err() {
                return StatusCode::CONFLICT.into_response();
            }
            "members"
        }
        _ => return StatusCode::UNPROCESSABLE_ENTITY.into_response(),
    };
    drop(store);
    app.hints.emit(topic);
    StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_name_the_topics_the_server_would() {
        assert_eq!(
            topics(&Method::POST, "/api/admin/runs/r-kalos/move"),
            ["schedule"]
        );
        assert_eq!(
            topics(&Method::POST, "/api/admin/inbox/p-1/approve"),
            ["inbox", "schedule"]
        );
        assert_eq!(topics(&Method::PATCH, "/api/admin/config"), ["settings"]);
        assert!(topics(&Method::GET, "/api/admin/config").is_empty());
        assert_eq!(
            topics(&Method::PATCH, "/api/admin/members/1001"),
            ["members"]
        );
        assert!(topics(&Method::PATCH, "/api/admin/limits/windows/x").is_empty());
    }

    #[test]
    fn a_reconnect_gets_what_it_missed_or_a_fresh_ready() {
        let hints = Hints::default();
        hints.emit("schedule");
        hints.emit("inbox");
        assert_eq!(hints.after(1), Some(vec![(2, "inbox")]));
        assert_eq!(hints.after(2), Some(vec![]));
        for _ in 0..KEPT {
            hints.emit("chat");
        }
        assert_eq!(hints.after(1), None, "too old: start over");
    }
}
