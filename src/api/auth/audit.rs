//! Security events for both realms. Records carry the realm, the request id,
//! identities and reason codes; admin records also carry the client IP.
//! Tokens, codes, state, verifiers, cookies and secrets never reach a sink,
//! and member (public origin) records never carry an IP either.

use std::{net::IpAddr, sync::Mutex};

use axum::{extract::FromRequestParts, http::request::Parts};
use serde::Serialize;

use crate::api::guard::proxy::{ClientIp, RequestId};

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
        /// The Discord user id once `/users/@me` answered.
        #[serde(skip_serializing_if = "Option::is_none")]
        user: Option<String>,
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
    /// A member session's id rotated after its client address changed.
    SessionRotated {
        actor: String,
    },
    RateLimited {
        route: &'static str,
    },
    /// Discord token revocation failed; the token itself is never logged.
    RevokeFailed {
        reason: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        user: Option<String>,
    },
}

impl AuditEvent {
    fn level(&self) -> &'static str {
        match self {
            Self::LoginSucceeded { .. }
            | Self::SessionEnded { .. }
            | Self::SessionRotated { .. } => "INFO",
            Self::LoginRefused { .. }
            | Self::BreakGlassUsed { .. }
            | Self::RateLimited { .. }
            | Self::RevokeFailed { .. } => "WARN",
        }
    }
}

/// Who and which request an event belongs to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditContext {
    pub request_id: String,
    pub client: Option<IpAddr>,
}

impl AuditContext {
    pub fn of(parts: &Parts) -> Self {
        Self {
            request_id: parts
                .extensions
                .get::<RequestId>()
                .map(|id| id.0.clone())
                .unwrap_or_default(),
            client: parts
                .extensions
                .get::<ClientIp>()
                .and_then(|client| client.0),
        }
    }

    /// Events from the bot's gateway rather than an HTTP request.
    pub fn gateway() -> Self {
        Self {
            request_id: "gateway".into(),
            client: None,
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for AuditContext {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Self::of(parts))
    }
}

/// Which sign-in realm a record belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Realm {
    Admin,
    /// The public origin's members.
    Member,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AuditRecord {
    pub realm: Realm,
    pub request_id: String,
    pub client: Option<String>,
    #[serde(flatten)]
    pub event: AuditEvent,
}

impl AuditRecord {
    pub fn new(realm: Realm, context: &AuditContext, event: AuditEvent) -> Self {
        Self {
            realm,
            request_id: context.request_id.clone(),
            // Members' addresses are never logged (D5-A, item 20).
            client: match realm {
                Realm::Admin => context.client.map(|ip| ip.to_string()),
                Realm::Member => None,
            },
            event,
        }
    }
}

pub trait AuditSink: Send + Sync {
    fn record(&self, record: AuditRecord);
}

/// JSON lines on stderr, like the runtime's other logs.
pub struct StderrAudit;

impl AuditSink for StderrAudit {
    fn record(&self, record: AuditRecord) {
        #[derive(Serialize)]
        struct Line<'a> {
            level: &'static str,
            #[serde(flatten)]
            record: &'a AuditRecord,
        }
        if let Ok(line) = serde_json::to_string(&Line {
            level: record.event.level(),
            record: &record,
        }) {
            eprintln!("{line}");
        }
    }
}

/// Test sink that keeps every record.
#[derive(Default)]
pub struct RecordingAudit(Mutex<Vec<AuditRecord>>);

impl RecordingAudit {
    pub fn records(&self) -> Vec<AuditRecord> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn events(&self) -> Vec<AuditEvent> {
        self.records()
            .into_iter()
            .map(|record| record.event)
            .collect()
    }

    /// Every record as its serialized log line.
    pub fn lines(&self) -> Vec<String> {
        self.records()
            .iter()
            .filter_map(|record| serde_json::to_string(record).ok())
            .collect()
    }
}

impl AuditSink for RecordingAudit {
    fn record(&self, record: AuditRecord) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(record);
    }
}
