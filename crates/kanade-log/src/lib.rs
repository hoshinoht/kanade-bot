//! Structured JSON log lines on stderr, one object per line.

use serde::Serialize;

#[doc(hidden)]
pub use serde_json::json as __json;

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
#[macro_export]
macro_rules! log {
    (info, $($rest:tt)+) => { $crate::log!(@emit "INFO", $($rest)+) };
    (warn, $($rest:tt)+) => { $crate::log!(@emit "WARN", $($rest)+) };
    (error, $($rest:tt)+) => { $crate::log!(@emit "ERROR", $($rest)+) };
    (@emit $level:expr, $event:expr $(, $field:ident $(= $value:expr)?)* $(,)?) => {
        $crate::event(
            $level,
            $event,
            $crate::__json!({
                $(stringify!($field): $crate::log!(@value $field $(= $value)?)),*
            }),
        )
    };
    (@value $field:ident) => { $field };
    (@value $field:ident = $value:expr) => { $value };
    ($level:expr, $($rest:tt)+) => { $crate::log!(@emit $level, $($rest)+) };
}

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static CAPTURED: std::cell::RefCell<Option<Vec<serde_json::Value>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test support: lines emitted on this thread go to [`captured`] instead of stderr.
#[cfg(any(test, feature = "test-support"))]
pub fn capture() {
    CAPTURED.with(|lines| *lines.borrow_mut() = Some(Vec::new()));
}

#[cfg(any(test, feature = "test-support"))]
pub fn captured() -> Vec<serde_json::Value> {
    CAPTURED.with(|lines| lines.borrow().clone().unwrap_or_default())
}

/// Writes `event` as one line (to the [`capture`] buffer when one is set).
pub fn emit(event: &impl Serialize) {
    #[cfg(any(test, feature = "test-support"))]
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
