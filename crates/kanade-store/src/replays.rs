//! `Idempotency-Key` replays (migration 0031) for admin API effects that are
//! not scheduler commits: the Limits window clear, the manual digest and
//! header rewrite (scope `limits`), rescan submit and cancel (`rescan`), the
//! Config PATCH, the admin member edits, the admin character writes and run
//! picks, and the guide writes (`config`: the writes History lists beside
//! the change chain). A row keeps the digest of the normalized full request
//! and the answer to replay until it expires; an expired row reads as
//! absent, so its key applies anew.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::json;

use kanade_domain::characters::{
    CharacterChange, CharacterRecord, CharacterWrite, PickChange, PickWrite,
};
use kanade_domain::guides::{
    CancelRequest, GuideChange, GuideRecord, GuideWrite, NotPosted, PublishRequest,
    PublishRequested,
};
use kanade_domain::history::{Actor, Surface};
use kanade_domain::members::{MemberProfile, PortalEdit};
use kanade_domain::scheduler::StoreError;
use kanade_domain::settings::{RowDiff, SettingsChange};

/// How long a key replays. The in-memory maps this replaces had no time
/// limit, only a count, so a key lived until the process restarted (days in
/// practice); a client retries within seconds to minutes. A day covers a tab
/// left open overnight while keeping the table bounded.
pub const REPLAY_TTL: TimeDelta = TimeDelta::hours(24);

/// The longest stored body: notices or a job id, never a whole view.
pub const MAX_BODY: usize = 16 * 1024;

kanade_domain::spelled! {
    /// Keys are scoped per desk: the same key reused across a desk's routes (a
    /// digest key sent to a window clear) is a different request, not a new one.
    ReplayScope {
        /// Limits window clear, manual digest, manual header rewrite.
        Limits => "limits",
        /// Rescan submit and cancel.
        Rescan => "rescan",
        /// Config PATCH, admin member edits, admin character and guide writes.
        Config => "config",
        /// Admin draft writes (migration 0038); no longer written, kept so
        /// stored rows decode until they expire.
        Drafts => "drafts",
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

    /// The persisted shape, also enforced by SQLite CHECK constraints.
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
            kanade_domain::time::to_iso(at)
                .map_err(|error| StoreError::Constraint(format!("{at:?}: {error}")))?;
        }
        Ok(())
    }

    /// Live at `now`: an expired key is treated as never seen.
    pub fn live(&self, now: DateTime<Utc>) -> bool {
        now < self.expires_at
    }
}

/// History's settings section of an admin member edit (beside the Config
/// saves and the Limits window clears).
pub const MEMBERS_SECTION: &str = "members";

/// Who made a portal member edit, and when, for its History record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortalRecord {
    pub at: DateTime<Utc>,
    pub actor: Actor,
    pub surface: Surface,
}

impl PortalRecord {
    /// The History record of an edit from `before` to `after`: one row
    /// `member.<user id>` whose sides are the portal fields as JSON; `None`
    /// when no portal field changed.
    pub fn change(&self, before: &MemberProfile, after: &MemberProfile) -> Option<SettingsChange> {
        let (from, to) = (portal_fields(before), portal_fields(after));
        (from != to).then(|| SettingsChange {
            id: 0,
            at: self.at,
            actor: self.actor.clone(),
            surface: self.surface,
            section: MEMBERS_SECTION.into(),
            revision: 0,
            values: BTreeMap::from([(
                format!("member.{}", after.member.user_id),
                RowDiff { from, to },
            )]),
        })
    }
}

fn portal_fields(profile: &MemberProfile) -> String {
    json!({
        "member": profile.member.name().unwrap_or(&profile.member.user_id),
        "ping_level": profile.member.ping_level.as_str(),
        "reply_style": profile.reply_style,
        "aliases": profile.aliases,
        "tease_ok": profile.tease_ok,
        "running_joke": profile.running_joke,
    })
    .to_string()
}

/// What a recorded portal edit did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortalWrite {
    /// No such member; nothing written.
    Missing,
    /// The row as written (unchanged by a no-op edit).
    Edited(MemberProfile),
    /// The key already answered this request; nothing written.
    Replayed,
    /// The key is live for another request; nothing written.
    KeyTaken,
}

/// A live row under `replay`'s key, as a recorded edit sees it.
pub(crate) fn replayed(found: Option<&StoredReplay>, replay: &StoredReplay) -> Option<PortalWrite> {
    let found = found.filter(|found| found.live(replay.created_at))?;
    Some(if found.request == replay.request {
        PortalWrite::Replayed
    } else {
        PortalWrite::KeyTaken
    })
}

/// What a keyed write did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyedWrite<T> {
    /// The write ran; its replay is stored only if it went through.
    Applied(T),
    /// The key already answered this request: the stored answer.
    Replayed(StoredReplay),
    /// The key is live for another request; nothing written.
    KeyTaken,
}

/// The live row under `replay`'s key, as a keyed write sees it.
pub(crate) fn keyed<T>(
    found: Option<StoredReplay>,
    replay: &StoredReplay,
) -> Option<KeyedWrite<T>> {
    let found = found.filter(|found| found.live(replay.created_at))?;
    Some(if found.request == replay.request {
        KeyedWrite::Replayed(found)
    } else {
        KeyedWrite::KeyTaken
    })
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

    /// A portal member edit (as `MemberStore::apply_portal`), its History
    /// record ([`PortalRecord::change`], only when a field changed) and
    /// `replay`, in one transaction. A live replay under the key answers
    /// [`PortalWrite::Replayed`] (or [`PortalWrite::KeyTaken`] for another
    /// request) and nothing is written; a refused edit writes nothing.
    fn apply_portal_recorded(
        &self,
        user_id: &str,
        edit: PortalEdit,
        record: PortalRecord,
        replay: Option<StoredReplay>,
    ) -> impl Future<Output = Result<PortalWrite, StoreError>> + Send;

    /// A character write (as `CharacterStore::apply_character`, History
    /// record included) and `replay`, in one transaction. A live replay
    /// under the key answers [`KeyedWrite::Replayed`] (or
    /// [`KeyedWrite::KeyTaken`]) and nothing is written; `replay` is stored
    /// only with a write that went through ([`CharacterWrite::applied`]).
    fn apply_character_replayed(
        &self,
        change: CharacterChange,
        record: CharacterRecord,
        replay: StoredReplay,
    ) -> impl Future<Output = Result<KeyedWrite<CharacterWrite>, StoreError>> + Send;

    /// A run pick write (as `CharacterStore::apply_pick`, History record
    /// included) and `replay`, in one transaction, keyed as
    /// [`Self::apply_character_replayed`] (stored only with
    /// [`PickWrite::applied`]).
    fn apply_pick_replayed(
        &self,
        change: PickChange,
        record: CharacterRecord,
        replay: StoredReplay,
    ) -> impl Future<Output = Result<KeyedWrite<PickWrite>, StoreError>> + Send;

    /// A guide create, save or delete (as `GuideStore::apply_guide`, History
    /// record included) and `replay`, in one transaction, keyed as
    /// [`Self::apply_character_replayed`] (stored only with
    /// [`GuideWrite::applied`]).
    fn apply_guide_replayed(
        &self,
        change: GuideChange,
        record: GuideRecord,
        replay: StoredReplay,
    ) -> impl Future<Output = Result<KeyedWrite<GuideWrite>, StoreError>> + Send;

    /// A publish, unpublish or search request (as
    /// `GuideStore::request_publish`) and `replay`, in one transaction;
    /// stored only with [`PublishRequested::Queued`].
    fn request_publish_replayed(
        &self,
        request: PublishRequest,
        record: GuideRecord,
        replay: StoredReplay,
    ) -> impl Future<Output = Result<KeyedWrite<PublishRequested>, StoreError>> + Send;

    /// A run cancel (as `GuideStore::request_cancel`, at the replay's
    /// `created_at`) and `replay`, in one transaction; stored only with
    /// [`CancelRequest::Requested`].
    fn request_cancel_replayed(
        &self,
        guide_id: String,
        publish_id: String,
        replay: StoredReplay,
    ) -> impl Future<Output = Result<KeyedWrite<CancelRequest>, StoreError>> + Send;

    /// "Mark as not posted" (as `GuideStore::mark_not_posted`, History record
    /// included) and `replay`, in one transaction; stored only with
    /// [`NotPosted::Removed`].
    fn mark_not_posted_replayed(
        &self,
        guide_id: String,
        channel_id: String,
        position: u32,
        record: GuideRecord,
        replay: StoredReplay,
    ) -> impl Future<Output = Result<KeyedWrite<NotPosted>, StoreError>> + Send;

    /// Delete every row expired at `now`; returns how many.
    fn prune_replays(
        &self,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;
}
