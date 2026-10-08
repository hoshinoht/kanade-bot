//! Member sign-in for the public origin (`member-auth-contract.md` §4): its
//! own Discord application (D2-A), the eligibility gate (D3-A), sessions
//! behind `__Host-kanade_pub` with idle/absolute expiry, a fresh-write
//! window, a 10-session cap (D5-A) and IP-change rotation with a short
//! read-only grace (D9-A). It shares only primitives with the admin realm
//! (crypto, wire, csrf, device, the Discord login, rate buckets, audit) and
//! never reads an admin credential.

mod devices;
mod gate;
mod session;

use std::{net::IpAddr, sync::Arc};

use chrono::{DateTime, TimeDelta, Utc};

pub use devices::{EndError, MemberDevice};
pub use gate::{Eligibility, EligibilityGate, StoreEligibility};
pub(crate) use session::SESSION_SAME_SITE;
pub use session::{MemberSession, require_session};

use super::{
    Clock, actor_id,
    audit::{AuditContext, AuditEvent, AuditRecord, AuditSink, Realm, StderrAudit},
    crypto::{self, TagKey},
    discord::{DiscordLogin, DiscordUser},
    rate::RateLimits,
    system_now,
};
use crate::{
    api::avatars::AvatarCache,
    infrastructure::store::web_sessions::{
        LoginMethod, SessionOrigin, WebSession, WebSessionStore,
    },
};

/// Live portal switch: `self_service.public_portal`, read per request
/// (built in serve composition like the digest's `PortalSwitch`).
pub type PortalOpen = Arc<dyn Fn() -> bool + Send + Sync>;

/// Context of the keyed client-address tag; the raw address is never stored.
const CLIENT_CONTEXT: &[u8] = b"kanade-public-client-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemberPolicy {
    /// 30 min (5–60).
    pub idle: TimeDelta,
    /// 8 h (1–24).
    pub absolute: TimeDelta,
    /// 15 min (5–30); `fresh <= idle <= absolute`.
    pub fresh: TimeDelta,
    /// Eligibility re-check interval: 5 min, fixed.
    pub recheck: TimeDelta,
    /// Read-only life of a rotated-out id: 30 s, fixed (D9).
    pub grace: TimeDelta,
    /// Live sessions per member: 10, fixed (D5).
    pub max_sessions: usize,
}

impl Default for MemberPolicy {
    fn default() -> Self {
        Self {
            idle: TimeDelta::minutes(30),
            absolute: TimeDelta::hours(8),
            fresh: TimeDelta::minutes(15),
            recheck: TimeDelta::minutes(5),
            grace: TimeDelta::seconds(30),
            max_sessions: 10,
        }
    }
}

pub struct MemberAuth {
    /// [`SessionOrigin::Public`] and [`LoginMethod::Discord`] rows only.
    sessions: Arc<dyn WebSessionStore>,
    gate: Arc<dyn EligibilityGate>,
    /// The public application (D2-A); `None` keeps the portal closed.
    discord: Option<DiscordLogin>,
    open: PortalOpen,
    policy: MemberPolicy,
    clock: Clock,
    audit: Arc<dyn AuditSink>,
    /// Own buckets, never the admin realm's.
    rate: RateLimits,
    /// The same cache the admin portraits use.
    portraits: Option<Arc<AvatarCache>>,
    /// Per process: a restart re-tags (so rotates) every session once.
    tags: Option<TagKey>,
}

impl std::fmt::Debug for MemberAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemberAuth")
            .field("discord", &self.discord.is_some())
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl MemberAuth {
    pub fn new(
        sessions: Arc<dyn WebSessionStore>,
        gate: Arc<dyn EligibilityGate>,
        open: PortalOpen,
    ) -> Self {
        Self {
            sessions,
            gate,
            discord: None,
            open,
            policy: MemberPolicy::default(),
            clock: Arc::new(system_now),
            audit: Arc::new(StderrAudit),
            rate: RateLimits::default(),
            portraits: None,
            tags: TagKey::generate(),
        }
    }

    /// The public application's login; it always asks with `prompt=none`.
    pub fn with_discord(mut self, login: DiscordLogin) -> Self {
        self.discord = Some(login.with_prompt_none());
        self
    }

    pub fn with_policy(mut self, policy: MemberPolicy) -> Self {
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

    pub fn with_rate_limits(mut self, rate: RateLimits) -> Self {
        self.rate = rate;
        self
    }

    pub fn with_portraits(mut self, portraits: Arc<AvatarCache>) -> Self {
        self.portraits = Some(portraits);
        self
    }

    /// The switch is on and the public application is configured.
    pub fn is_open(&self) -> bool {
        self.discord.is_some() && (self.open)()
    }

    pub(crate) fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    pub(crate) fn policy(&self) -> MemberPolicy {
        self.policy
    }

    pub(crate) fn discord(&self) -> Option<&DiscordLogin> {
        self.discord.as_ref()
    }

    pub(crate) fn gate(&self) -> &dyn EligibilityGate {
        self.gate.as_ref()
    }

    pub(crate) fn sessions(&self) -> &dyn WebSessionStore {
        self.sessions.as_ref()
    }

    pub(crate) fn rate(&self) -> &RateLimits {
        &self.rate
    }

    pub(crate) fn portraits(&self) -> Option<&AvatarCache> {
        self.portraits.as_deref()
    }

    pub(crate) fn audit(&self, context: &AuditContext, event: AuditEvent) {
        self.audit
            .record(AuditRecord::new(Realm::Member, context, event));
    }

    /// The keyed tag of the client address; `None` without system randomness.
    pub(crate) fn client_tag(&self, client: Option<IpAddr>) -> Option<String> {
        let address = client.map(|ip| ip.to_string()).unwrap_or_default();
        Some(self.tags.as_ref()?.hex(CLIENT_CONTEXT, address.as_bytes()))
    }

    /// Live: within the absolute and the idle lifetime.
    pub(crate) fn live(&self, row: &WebSession, now: DateTime<Utc>) -> bool {
        now < row.expires_at && now < row.last_seen_at + self.policy.idle
    }

    /// Gateway hook: the member left the guild (or was banned); every public
    /// session of theirs ends now.
    pub async fn member_left(&self, user_id: &str) -> u64 {
        self.end_all(&AuditContext::gateway(), user_id, "member_left")
            .await
            .unwrap_or(0)
    }

    /// Gateway hook: roles changed; a member no longer eligible loses every
    /// public session at once.
    pub async fn member_changed(&self, user_id: &str, eligible: bool) -> u64 {
        if eligible {
            return 0;
        }
        self.end_all(&AuditContext::gateway(), user_id, "not_eligible")
            .await
            .unwrap_or(0)
    }

    /// Delete every public session of `user_id` (superseded ids included).
    pub(crate) async fn end_all(
        &self,
        context: &AuditContext,
        user_id: &str,
        reason: &'static str,
    ) -> Result<u64, ()> {
        let ended = self
            .sessions
            .delete_subject_sessions(SessionOrigin::Public, LoginMethod::Discord, user_id)
            .await
            .map_err(|_| ())?;
        if ended > 0 {
            self.audit(
                context,
                AuditEvent::SessionEnded {
                    actor: actor_id(LoginMethod::Discord, user_id),
                    reason,
                },
            );
        }
        Ok(ended)
    }

    /// Start a session for an eligible `user`, replacing `replaces` (this
    /// browser's current session) and ending the member's oldest sessions
    /// beyond the cap, all in one store write. Returns the cookie value.
    ///
    /// The gate is asked again once the row exists: a roster event persists
    /// the member's state before it ends their sessions, so one that lands
    /// after the caller's check either is seen here or ends this new row
    /// itself. Every new row starts due for its re-check, so the first
    /// request through [`require_session`] asks the gate again too; that is
    /// what an unreadable second check relies on, since the replaced and
    /// capped rows are already gone and must not be lost for nothing.
    pub(crate) async fn start_session(
        &self,
        context: &AuditContext,
        user: &DiscordUser,
        device: Option<String>,
        replaces: Option<&str>,
    ) -> Result<String, StartError> {
        let now = self.now();
        let id = crypto::random_token().ok_or(StartError::Store)?;
        let session = WebSession {
            id_hash: crypto::sha256_hex(id.as_bytes()),
            origin: SessionOrigin::Public,
            method: LoginMethod::Discord,
            subject: user.id.clone(),
            display: user.display(),
            created_at: now,
            last_seen_at: now,
            checked_at: now - self.policy.recheck,
            expires_at: now + self.policy.absolute,
            avatar_hash: user.avatar.clone(),
            device,
            client_tag: Some(self.client_tag(context.client).ok_or(StartError::Store)?),
            superseded_until: None,
        };
        let replaces = replaces.map(|old| crypto::sha256_hex(old.as_bytes()));
        let capped = self
            .sessions
            .put_capped_session(
                &session,
                replaces.as_deref(),
                self.policy.max_sessions,
                now,
                now - self.policy.idle,
            )
            .await
            .map_err(|_| StartError::Store)?;
        for _ in 0..capped {
            self.audit(
                context,
                AuditEvent::SessionEnded {
                    actor: actor_id(LoginMethod::Discord, &user.id),
                    reason: "capped",
                },
            );
        }
        // `Unavailable` keeps the row: it is already due for its re-check.
        if self.gate.check(&user.id).await == Eligibility::NotEligible {
            let _ = self.end_all(context, &user.id, "not_eligible").await;
            return Err(StartError::NotEligible);
        }
        self.audit(
            context,
            AuditEvent::LoginSucceeded {
                method: LoginMethod::Discord.as_str(),
                actor: actor_id(LoginMethod::Discord, &user.id),
            },
        );
        Ok(id)
    }
}

/// Why [`MemberAuth::start_session`] handed out no session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartError {
    /// No randomness or the session store failed.
    Store,
    /// The re-check after the insert found the member ineligible.
    NotEligible,
}
