//! Security events for the admin origin. Events carry identities and reason
//! codes only; tokens, codes, cookies and secrets never reach a sink.

use std::sync::Mutex;

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AuditEvent {
    LoginSucceeded {
        method: &'static str,
        actor: String,
    },
    LoginRefused {
        method: &'static str,
        reason: &'static str,
    },
    /// Loud by design: every break-glass use is an operator decision to review.
    BreakGlassUsed {
        via: &'static str,
        request: String,
    },
    SessionEnded {
        actor: String,
        reason: &'static str,
    },
}

impl AuditEvent {
    fn level(&self) -> &'static str {
        match self {
            Self::LoginSucceeded { .. } | Self::SessionEnded { .. } => "INFO",
            Self::LoginRefused { .. } | Self::BreakGlassUsed { .. } => "WARN",
        }
    }
}

pub trait AuditSink: Send + Sync {
    fn record(&self, event: AuditEvent);
}

/// JSON lines on stderr, like the runtime's other logs.
pub struct StderrAudit;

impl AuditSink for StderrAudit {
    fn record(&self, event: AuditEvent) {
        #[derive(Serialize)]
        struct Line<'a> {
            level: &'static str,
            #[serde(flatten)]
            event: &'a AuditEvent,
        }
        if let Ok(line) = serde_json::to_string(&Line {
            level: event.level(),
            event: &event,
        }) {
            eprintln!("{line}");
        }
    }
}

/// Test sink that keeps every event.
#[derive(Default)]
pub struct RecordingAudit(Mutex<Vec<AuditEvent>>);

impl RecordingAudit {
    pub fn events(&self) -> Vec<AuditEvent> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Every event as its serialized log line.
    pub fn lines(&self) -> Vec<String> {
        self.events()
            .iter()
            .filter_map(|event| serde_json::to_string(event).ok())
            .collect()
    }
}

impl AuditSink for RecordingAudit {
    fn record(&self, event: AuditEvent) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(event);
    }
}
