//! Security events for both realms. Records carry the realm, the request id,
//! identities and reason codes; admin records also carry the client IP,
//! member (public origin) records only its keyed tag, never the address.
//! Tokens, codes, state, verifiers, cookies and secrets never reach a sink.
//! Production keeps them twice: JSON lines on stderr and the stored sign-in
//! audit log ([`StoreAudit`], History › Sign-ins).

use std::{net::IpAddr, sync::Arc, sync::Mutex, time::Duration};

use axum::{
    extract::FromRequestParts,
    http::{Method, Uri, request::Parts},
};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::api::guard::proxy::{ClientIp, RequestId};
use crate::infrastructure::store::auth_audit::{AuditKind, AuditRealm, AuditRow, AuthAuditStore};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AuditEvent {
    LoginSucceeded {
        method: &'static str,
        actor: String,
        /// Browser and system label of the new session.
        #[serde(skip_serializing_if = "Option::is_none")]
        device: Option<String>,
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
    /// A member write refused: `reason` is `csrf`, `reauth_required` or the
    /// refusal's 403/404/409 code; `route` is the method and path.
    /// Logged only: the stored log's event CHECK has no such kind.
    WriteRefused {
        actor: String,
        route: String,
        reason: String,
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
            | Self::RevokeFailed { .. }
            | Self::WriteRefused { .. } => "WARN",
        }
    }

    /// [`Self::WriteRefused`] of `method` `uri` (path only, at most 256
    /// bytes; no query) by `actor`.
    pub fn write_refused(actor: String, method: &Method, uri: &Uri, reason: &str) -> Self {
        let mut route = format!("{method} {}", uri.path());
        let mut end = route.len().min(256);
        while !route.is_char_boundary(end) {
            end -= 1;
        }
        route.truncate(end);
        Self::WriteRefused {
            actor,
            route,
            reason: reason.to_owned(),
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
    /// The realm's clock; log lines carry their own time.
    #[serde(skip)]
    pub at: DateTime<Utc>,
    pub realm: Realm,
    pub request_id: String,
    /// Admin: the client address. Member: the keyed tag of it, set by the
    /// member realm (D5-A, item 20: the address itself is never kept).
    pub client: Option<String>,
    #[serde(flatten)]
    pub event: AuditEvent,
}

impl AuditRecord {
    pub fn new(realm: Realm, context: &AuditContext, event: AuditEvent, at: DateTime<Utc>) -> Self {
        Self {
            at,
            realm,
            request_id: context.request_id.clone(),
            client: match realm {
                Realm::Admin => context.client.map(|ip| ip.to_string()),
                Realm::Member => None,
            },
            event,
        }
    }

    /// The stored row (`seq` is assigned by the store); `None` for events the
    /// log's event CHECK cannot hold, which stay stderr lines only.
    pub fn row(&self) -> Option<AuditRow> {
        let owned = |text: &str| Some(text.to_owned());
        let (event, actor, method, reason, request, device) = match &self.event {
            AuditEvent::LoginSucceeded {
                method,
                actor,
                device,
            } => (
                AuditKind::LoginSucceeded,
                owned(actor),
                owned(method),
                None,
                None,
                device.clone(),
            ),
            AuditEvent::LoginRefused {
                method,
                reason,
                user,
            } => (
                AuditKind::LoginRefused,
                user.clone(),
                owned(method),
                owned(reason),
                None,
                None,
            ),
            AuditEvent::BreakGlassUsed { via, request } => (
                AuditKind::BreakGlassUsed,
                owned("token"),
                owned(via),
                None,
                owned(request),
                None,
            ),
            AuditEvent::SessionEnded { actor, reason } => (
                AuditKind::SessionEnded,
                owned(actor),
                None,
                owned(reason),
                None,
                None,
            ),
            AuditEvent::SessionRotated { actor } => (
                AuditKind::SessionRotated,
                owned(actor),
                None,
                None,
                None,
                None,
            ),
            AuditEvent::RateLimited { route } => {
                (AuditKind::RateLimited, None, None, owned(route), None, None)
            }
            AuditEvent::RevokeFailed { reason, user } => (
                AuditKind::RevokeFailed,
                user.clone(),
                None,
                owned(reason),
                None,
                None,
            ),
            AuditEvent::WriteRefused { .. } => return None,
        };
        // Bounded as the store checks; a longer value is cut, never refused.
        let cut = |text: Option<String>, max: usize| {
            text.filter(|text| !text.is_empty()).map(|mut text| {
                if text.len() > max {
                    let mut end = max;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                }
                text
            })
        };
        Some(AuditRow {
            seq: 0,
            at: self.at,
            realm: match self.realm {
                Realm::Admin => AuditRealm::Admin,
                Realm::Member => AuditRealm::Member,
            },
            event,
            actor: cut(actor, 256),
            method: cut(method, 32),
            reason: cut(reason, 64),
            request: cut(request, 256),
            client: cut(self.client.clone(), 64),
            device: cut(device, 64),
            request_id: cut(Some(self.request_id.clone()), 128).unwrap_or_default(),
        })
    }
}

pub trait AuditSink: Send + Sync {
    fn record(&self, record: AuditRecord);
}

/// A stored row gives up after this, so a busy store never holds sign-in.
pub const AUDIT_WRITE_DEADLINE: Duration = Duration::from_secs(2);

/// Production: the stderr line, then the stored row, written off the request
/// path. A failed write is a WARN line and never fails the sign-in.
pub struct StoreAudit<S> {
    store: Arc<S>,
}

impl<S> StoreAudit<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

impl<S: AuthAuditStore + Send + Sync + 'static> AuditSink for StoreAudit<S> {
    fn record(&self, record: AuditRecord) {
        let row = record.row();
        StderrAudit.record(record);
        let Some(row) = row else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let store = Arc::clone(&self.store);
        runtime.spawn(async move {
            let written = tokio::time::timeout(AUDIT_WRITE_DEADLINE, store.append_audit(row)).await;
            if !matches!(written, Ok(Ok(_))) {
                // Store error text may carry paths, so only the event is logged.
                crate::runtime::logging::event("WARN", "audit_unrecorded", serde_json::json!({}));
            }
        });
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_writes_are_log_lines_without_a_stored_row() {
        let context = AuditContext {
            request_id: "req-1".into(),
            client: None,
        };
        let refused = AuditRecord::new(
            Realm::Member,
            &context,
            AuditEvent::write_refused(
                "discord:1001".into(),
                &Method::PUT,
                &"/api/public/runs/r-1/answer?x=1".parse().unwrap(),
                "not_in_run",
            ),
            DateTime::UNIX_EPOCH,
        );
        assert_eq!(refused.row(), None);
        assert_eq!(refused.event.level(), "WARN");
        let line: serde_json::Value = serde_json::to_value(&refused).unwrap();
        assert_eq!(line["event"], "write_refused");
        assert_eq!(line["route"], "PUT /api/public/runs/r-1/answer");
        assert_eq!(line["reason"], "not_in_run");
        assert_eq!(line["actor"], "discord:1001");
        assert_eq!(line["realm"], "member");
        let long = format!("/{}", "a".repeat(400));
        let AuditEvent::WriteRefused { route, .. } = AuditEvent::write_refused(
            "discord:1001".into(),
            &Method::POST,
            &long.parse().unwrap(),
            "csrf",
        ) else {
            unreachable!()
        };
        assert_eq!(route.len(), 256);

        let limited = AuditRecord::new(
            Realm::Member,
            &context,
            AuditEvent::RateLimited {
                route: "member_read",
            },
            DateTime::UNIX_EPOCH,
        );
        let row = limited.row().expect("a stored kind");
        assert_eq!(row.event, AuditKind::RateLimited);
        assert_eq!(row.reason.as_deref(), Some("member_read"));
    }
}
