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

pub fn server_started(bind: &str) {
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
        mode: "offline",
        bind,
    });
}

pub fn shutdown_started() {
    emit(&serde_json::json!({"level":"INFO", "event":"shutdown_started"}));
}

fn emit(event: &impl Serialize) {
    if let Ok(value) = serde_json::to_string(event) {
        eprintln!("{value}");
    }
}
