//! Admin authentication: Discord OAuth (primary), the edge's Tailscale
//! identity (fallback) and the break-glass token, all ending in a server-side
//! session behind the `__Host-kanade_admin` cookie. Later slices take
//! [`AdminSession`] as a handler argument; it authenticates, re-checks the
//! identity, enforces CSRF on unsafe methods and yields the history actor.

pub mod audit;
pub mod crypto;
pub mod csrf;
pub mod discord;
pub mod discord_http;
#[cfg(any(test, feature = "test-support"))]
pub mod fake;
mod secrets;
mod session;
pub mod staff;
pub mod wire;

use std::{collections::BTreeSet, sync::Arc};

use chrono::{DateTime, TimeDelta, Utc};

pub use secrets::from_settings;
pub use session::AdminSession;

use self::{
    audit::{AuditEvent, AuditSink, StderrAudit},
    crypto::SealedSecret,
    discord::DiscordLogin,
    staff::StaffGate,
};
use crate::infrastructure::store::web_sessions::{
    LoginMethod, SessionOrigin, WebSession, WebSessionStore,
};

pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Tailscale login header the edge sets after `whois` (v4 `HEADER_LOGIN`).
pub const TAILSCALE_LOGIN: &str = "tailscale-user-login";
pub const TAILSCALE_NAME: &str = "tailscale-user-name";
/// Actor id of break-glass changes.
pub const TOKEN_ACTOR: &str = "token";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPolicy {
    pub idle: TimeDelta,
    pub absolute: TimeDelta,
    /// Staff and edge-identity re-check interval.
    pub recheck: TimeDelta,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle: TimeDelta::minutes(60),
            absolute: TimeDelta::hours(12),
            recheck: TimeDelta::minutes(5),
        }
    }
}

/// `last_seen_at` is written at most this often, so reads don't all write.
const TOUCH_EVERY: TimeDelta = TimeDelta::seconds(60);

pub(crate) struct BreakGlass {
    sealed: SealedSecret,
    /// Short SHA-256 prefix stored as the session subject: rotating the token ends its sessions.
    fingerprint: String,
}

pub struct AdminAuth {
    sessions: Arc<dyn WebSessionStore>,
    staff: Arc<dyn StaffGate>,
    discord: Option<DiscordLogin>,
    tailscale_logins: BTreeSet<String>,
    breakglass: Option<BreakGlass>,
    policy: SessionPolicy,
    clock: Clock,
    audit: Arc<dyn AuditSink>,
}

impl std::fmt::Debug for AdminAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminAuth")
            .field("discord", &self.discord.is_some())
            .field("tailscale_logins", &self.tailscale_logins.len())
            .field("breakglass", &self.breakglass.is_some())
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl AdminAuth {
    pub fn new(sessions: Arc<dyn WebSessionStore>, staff: Arc<dyn StaffGate>) -> Self {
        Self {
            sessions,
            staff,
            discord: None,
            tailscale_logins: BTreeSet::new(),
            breakglass: None,
            policy: SessionPolicy::default(),
            clock: Arc::new(system_now),
            audit: Arc::new(StderrAudit),
        }
    }

    pub fn with_discord(mut self, login: DiscordLogin) -> Self {
        self.discord = Some(login);
        self
    }

    /// Logins compare ASCII case-insensitively.
    pub fn with_tailscale_logins(mut self, logins: impl IntoIterator<Item = String>) -> Self {
        self.tailscale_logins = logins
            .into_iter()
            .map(|login| login.trim().to_ascii_lowercase())
            .filter(|login| !login.is_empty())
            .collect();
        self
    }

    /// `None` when the token cannot be sealed (no system randomness).
    pub fn with_breakglass(mut self, token: &[u8]) -> Option<Self> {
        self.breakglass = Some(BreakGlass {
            sealed: SealedSecret::new(token)?,
            fingerprint: crypto::sha256_hex(token)[..16].to_owned(),
        });
        Some(self)
    }

    pub fn with_policy(mut self, policy: SessionPolicy) -> Self {
        self.policy = policy;
        self
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn with_audit(mut self, audit: Arc<dyn AuditSink>) -> Self {
        self.audit = audit;
        self
    }

    pub(crate) fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    pub(crate) fn policy(&self) -> SessionPolicy {
        self.policy
    }

    pub(crate) fn discord(&self) -> Option<&DiscordLogin> {
        self.discord.as_ref()
    }

    pub(crate) fn staff(&self) -> &dyn StaffGate {
        self.staff.as_ref()
    }

    pub(crate) fn sessions(&self) -> &dyn WebSessionStore {
        self.sessions.as_ref()
    }

    pub(crate) fn audit(&self, event: AuditEvent) {
        self.audit.record(event);
    }

    pub(crate) fn tailscale_enabled(&self) -> bool {
        !self.tailscale_logins.is_empty()
    }

    pub(crate) fn tailscale_allows(&self, login: &str) -> bool {
        self.tailscale_logins.contains(&login.to_ascii_lowercase())
    }

    pub(crate) fn breakglass_enabled(&self) -> bool {
        self.breakglass.is_some()
    }

    /// The token's fingerprint when `candidate` is the break-glass token.
    pub(crate) fn breakglass_matches(&self, candidate: &[u8]) -> Option<&str> {
        self.breakglass
            .as_ref()
            .filter(|glass| glass.sealed.matches(candidate))
            .map(|glass| glass.fingerprint.as_str())
    }

    pub(crate) fn breakglass_fingerprint(&self) -> Option<&str> {
        self.breakglass
            .as_ref()
            .map(|glass| glass.fingerprint.as_str())
    }

    /// Start a session, deleting `replaces` (the caller's current session) in
    /// the same write: a login always rotates the id. Returns the cookie value.
    pub(crate) async fn start_session(
        &self,
        method: LoginMethod,
        subject: &str,
        display: &str,
        replaces: Option<&str>,
    ) -> Option<String> {
        let now = self.now();
        let id = crypto::random_token()?;
        let session = WebSession {
            id_hash: crypto::sha256_hex(id.as_bytes()),
            origin: SessionOrigin::Admin,
            method,
            subject: subject.to_owned(),
            display: display.to_owned(),
            created_at: now,
            last_seen_at: now,
            checked_at: now,
            expires_at: now + self.policy.absolute,
        };
        let _ = self
            .sessions
            .prune_sessions(now, now - self.policy.idle)
            .await;
        let replaces = replaces.map(|old| crypto::sha256_hex(old.as_bytes()));
        self.sessions
            .put_session(&session, replaces.as_deref())
            .await
            .ok()?;
        self.audit(AuditEvent::LoginSucceeded {
            method: method.as_str(),
            actor: actor_id(method, subject),
        });
        Some(id)
    }
}

/// Wall clock without chrono's `clock` feature; before the epoch reads as the epoch.
fn system_now() -> DateTime<Utc> {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_timestamp(
        i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        since.subsec_nanos(),
    )
    .unwrap_or(DateTime::UNIX_EPOCH)
}

/// History actor id: `discord:<user id>`, `tailscale:<login>` or `token`.
pub fn actor_id(method: LoginMethod, subject: &str) -> String {
    match method {
        LoginMethod::Discord => format!("discord:{subject}"),
        LoginMethod::Tailscale => format!("tailscale:{subject}"),
        LoginMethod::Token => TOKEN_ACTOR.into(),
    }
}
