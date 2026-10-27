use kanade_log::emit;
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
        version: &'static str,
    }
    emit(&Event {
        level: "INFO",
        event: "server_started",
        mode,
        bind,
        version: env!("CARGO_PKG_VERSION"),
    });
}

/// `closed` is false when closing failed or the store was still shared.
pub fn store_closed(closed: bool) {
    emit(&serde_json::json!({
        "level": if closed { "INFO" } else { "WARN" },
        "event": if closed { "store_closed" } else { "store_close_failed" },
    }));
}

/// Live serve runs without the `bot` component (`runtime.components`).
pub fn discord_disabled() {
    emit(&serde_json::json!({"level":"INFO", "event":"discord_disabled"}));
}

/// Live serve runs without the `web` component: the admin listener answers
/// only `/healthz`.
pub fn web_disabled() {
    emit(&serde_json::json!({"level":"INFO", "event":"web_disabled"}));
}

pub fn shutdown_started() {
    emit(&serde_json::json!({"level":"INFO", "event":"shutdown_started"}));
}
