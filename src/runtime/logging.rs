use serde::Serialize;

use super::error::Error;

#[derive(Serialize)]
struct ErrorEvent<'a> {
    level: &'static str,
    event: &'static str,
    kind: &'a str,
    retryable: bool,
    message: &'a str,
}

pub fn error(error: &Error) {
    emit(&ErrorEvent {
        level: "ERROR",
        event: "command_failed",
        kind: error.kind(),
        retryable: error.retryable(),
        message: &error.to_string(),
    });
}

pub fn server_started(mode: &'static str, bind: &str) {
    #[derive(Serialize)]
    struct Event<'a> {
        level: &'static str,
        event: &'static str,
        mode: &'static str,
        bind: &'a str,
    }
    emit(&Event {
        level: "INFO",
        event: "server_started",
        mode,
        bind,
    });
}

/// A store was dropped without `close()`, so ownership may have been
/// released while SQLite connections were still closing.
pub fn store_dropped_unclosed(db_path: &std::path::Path) {
    emit(&serde_json::json!({
        "level": "WARN",
        "event": "store_dropped_unclosed",
        "db_path": db_path.display().to_string(),
    }));
}

/// `closed` is false when closing failed or the store was still shared.
pub fn store_closed(closed: bool) {
    emit(&serde_json::json!({
        "level": if closed { "INFO" } else { "WARN" },
        "event": if closed { "store_closed" } else { "store_close_failed" },
    }));
}

/// Live serve runs without the Discord gateway until it is wired.
pub fn discord_disabled() {
    emit(&serde_json::json!({"level":"INFO", "event":"discord_disabled"}));
}

pub fn shutdown_started() {
    emit(&serde_json::json!({"level":"INFO", "event":"shutdown_started"}));
}

fn emit(event: &impl Serialize) {
    if let Ok(value) = serde_json::to_string(event) {
        eprintln!("{value}");
    }
}
