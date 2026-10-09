//! `Idempotency-Key` replays (migration 0031) for admin API effects that are
//! not scheduler commits: the Limits window clear, the manual digest and
//! header rewrite (scope `limits`), rescan submit and cancel (`rescan`) and
//! the Config PATCH (`config`). A row keeps the digest of the normalized full
//! request and the answer to replay until it expires; an expired row reads as
//! absent, so its key applies anew.

use chrono::{DateTime, TimeDelta, Utc};

use crate::domain::scheduler::StoreError;
use crate::domain::settings::SettingsChange;

/// How long a key replays. The in-memory maps this replaces had no time
/// limit, only a count, so a key lived until the process restarted (days in
/// practice); a client retries within seconds to minutes. A day covers a tab
/// left open overnight while keeping the table bounded.
pub const REPLAY_TTL: TimeDelta = TimeDelta::hours(24);

/// The longest stored body: notices or a job id, never a whole view.
pub const MAX_BODY: usize = 16 * 1024;

/// Keys are scoped per desk: the same key reused across a desk's routes (a
/// digest key sent to a window clear) is a different request, not a new one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ReplayScope {
    /// Limits window clear, manual digest, manual header rewrite.
    Limits,
    /// Rescan submit and cancel.
    Rescan,
    /// Config PATCH.
    Config,
}

impl ReplayScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Limits => "limits",
            Self::Rescan => "rescan",
            Self::Config => "config",
        }
    }
}

/// One recorded keyed request and the answer it replays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredReplay {
    pub scope: ReplayScope,
    /// `<actor kind>:<actor id>`; keys are never shared between actors.
    pub actor: String,
    pub key: String,
    /// SHA-256 (64 lowercase hex) of the normalized full request.
    pub request: String,
    /// The 2xx status the route answered.
    pub status: u16,
    /// The JSON the route rebuilds its answer from.
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl StoredReplay {
    /// A replay recorded at `now`, expiring [`REPLAY_TTL`] later.
    pub fn new(
        scope: ReplayScope,
        actor: String,
        key: String,
        request: String,
        status: u16,
        body: String,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            scope,
            actor,
            key,
            request,
            status,
            body,
            created_at: now,
            expires_at: now + REPLAY_TTL,
        }
    }

    /// The shape both stores enforce (SQLite by CHECK as well).
    ///
    /// # Errors
    /// [`StoreError::Constraint`] naming the first bad field.
    pub fn check(&self) -> Result<(), StoreError> {
        let refuse = |what: &str| Err(StoreError::Constraint(format!("replay {what}")));
        if !(1..=256).contains(&self.actor.len()) {
            return refuse("actor is 1-256 bytes");
        }
        if !(1..=128).contains(&self.key.len()) {
            return refuse("key is 1-128 bytes");
        }
        if self.request.len() != 64
            || !self
                .request
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return refuse("request is 64 lowercase hex");
        }
        if !(200..=299).contains(&self.status) {
            return refuse("status is 2xx");
        }
        if self.body.len() > MAX_BODY
            || serde_json::from_str::<serde_json::Value>(&self.body).is_err()
        {
            return refuse("body is JSON of at most 16 KiB");
        }
        if self.expires_at <= self.created_at {
            return refuse("expires after it is created");
        }
        // As SQLite's `to_iso`: instants outside years 1..=9999 are refused.
        for at in [&self.created_at, &self.expires_at] {
            crate::domain::time::to_iso(at)
                .map_err(|error| StoreError::Constraint(format!("{at:?}: {error}")))?;
        }
        Ok(())
    }

    /// Live at `now`: an expired key is treated as never seen.
    pub fn live(&self, now: DateTime<Utc>) -> bool {
        now < self.expires_at
    }
}

/// Replay rows. There is no scheduled store maintenance, so every replay
/// write deletes the rows expired at its `created_at` in its own
/// transaction: the table holds at most a [`REPLAY_TTL`] of keyed requests.
pub trait ReplayStore {
    /// The live replay of `(scope, actor, key)` at `now`, if any.
    fn replay(
        &self,
        scope: ReplayScope,
        actor: &str,
        key: &str,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<Option<StoredReplay>, StoreError>> + Send;

    /// Store `replay`. It may replace an expired row or a live row of the
    /// same request (a route updating its own answer); a live row of another
    /// request is [`StoreError::Constraint`] and nothing is written.
    fn put_replay(
        &self,
        replay: StoredReplay,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Settings rows, an optional History record of them (as
    /// `put_settings_rows_recorded`) and `replay`, all in one transaction:
    /// a refused write stores none of them.
    fn put_settings_rows_replayed(
        &self,
        rows: Vec<(String, String)>,
        change: Option<SettingsChange>,
        replay: StoredReplay,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Delete every row expired at `now`; returns how many.
    fn prune_replays(
        &self,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;
}
