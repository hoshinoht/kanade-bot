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

/// Live serve runs without the Discord gateway (`KANADE_DISCORD_GATEWAY=0`).
pub fn discord_disabled() {
    emit(&serde_json::json!({"level":"INFO", "event":"discord_disabled"}));
}

pub fn shutdown_started() {
    emit(&serde_json::json!({"level":"INFO", "event":"shutdown_started"}));
}

/// One structured line: `level`, `event`, then `fields` (an object) merged in.
/// Callers must never pass tokens, message content or store error text that
/// could carry secrets.
pub fn event(level: &'static str, event: &'static str, fields: serde_json::Value) {
    let mut line = serde_json::json!({"level": level, "event": event});
    if let (Some(line), serde_json::Value::Object(fields)) = (line.as_object_mut(), fields) {
        for (key, value) in fields {
            line.entry(key).or_insert(value);
        }
    }
    emit(&line);
}

/// `log!(warn, "name", field, key = value, ...)` is one [`event`] line with
/// `json!({"field": field, "key": value, ...})`. `info`, `warn` and `error`
/// spell the level; any other first argument is the level expression. Field
/// order is moot: `serde_json` (no `preserve_order`) sorts the keys.
macro_rules! log {
    (info, $($rest:tt)+) => { $crate::runtime::logging::log!(@emit "INFO", $($rest)+) };
    (warn, $($rest:tt)+) => { $crate::runtime::logging::log!(@emit "WARN", $($rest)+) };
    (error, $($rest:tt)+) => { $crate::runtime::logging::log!(@emit "ERROR", $($rest)+) };
    (@emit $level:expr, $event:expr $(, $field:ident $(= $value:expr)?)* $(,)?) => {
        $crate::runtime::logging::event(
            $level,
            $event,
            ::serde_json::json!({
                $(stringify!($field): $crate::runtime::logging::log!(@value $field $(= $value)?)),*
            }),
        )
    };
    (@value $field:ident) => { $field };
    (@value $field:ident = $value:expr) => { $value };
    ($level:expr, $($rest:tt)+) => { $crate::runtime::logging::log!(@emit $level, $($rest)+) };
}

pub(crate) use log;

#[cfg(test)]
thread_local! {
    static CAPTURED: std::cell::RefCell<Option<Vec<serde_json::Value>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test support: lines emitted on this thread go to [`captured`] instead of stderr.
#[cfg(test)]
pub fn capture() {
    CAPTURED.with(|lines| *lines.borrow_mut() = Some(Vec::new()));
}

#[cfg(test)]
pub fn captured() -> Vec<serde_json::Value> {
    CAPTURED.with(|lines| lines.borrow().clone().unwrap_or_default())
}

fn emit(event: &impl Serialize) {
    #[cfg(test)]
    {
        let captured = CAPTURED.with(|lines| match lines.borrow_mut().as_mut() {
            Some(lines) => {
                lines.extend(serde_json::to_value(event).ok());
                true
            }
            None => false,
        });
        if captured {
            return;
        }
    }
    if let Ok(value) = serde_json::to_string(event) {
        eprintln!("{value}");
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{capture, captured, event};

    #[test]
    fn log_macro_lines_match_event() {
        let cache_hit = true;
        let kind = "store";
        capture();
        log!(
            warn,
            "estimate_finished",
            cache_hit,
            detail = kind,
            nested = json!({"ids": [1, 2], "inner": {"ok": null}}),
        );
        event(
            "WARN",
            "estimate_finished",
            json!({"cache_hit": cache_hit, "detail": kind, "nested": {"ids": [1, 2], "inner": {"ok": null}}}),
        );
        log!(info, "tick_stopped");
        event("INFO", "tick_stopped", json!({}));
        log!(
            if cache_hit { "ERROR" } else { "INFO" },
            "tick_failed",
            kind
        );
        event("ERROR", "tick_failed", json!({"kind": kind}));
        let lines: Vec<String> = captured().iter().map(ToString::to_string).collect();
        assert_eq!(lines.len(), 6);
        for pair in lines.chunks(2) {
            assert_eq!(pair[0], pair[1]);
        }
        assert_eq!(
            lines[0],
            r#"{"cache_hit":true,"detail":"store","event":"estimate_finished","level":"WARN","nested":{"ids":[1,2],"inner":{"ok":null}}}"#
        );
    }
}
