//! The rows of the character-data file: links (kanade-owned), snapshots
//! (NEXON Open API data, at most 30 days old), tracked Discord posts, the
//! rolling call budget and per-character refresh state.

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};

use crate::domain::history::{Actor, Surface};
use crate::domain::scheduler::StoreError;

/// The `/character link` notice version a consent records. Bumping it
/// sends every active or stale link back to `pending` until its member
/// confirms the new notice.
pub const NOTICE_VERSION: u32 = 1;
/// Shown with every piece of Nexon data (NEXON Open API terms).
pub const ATTRIBUTION: &str = "Data based on NEXON Open API";
/// The only region a link may name (the MapleSEA Open API).
pub const LINK_REGION: &str = "msea";
/// The longest Nexon data is kept, from the older of its two fetches.
pub const SNAPSHOT_TTL: TimeDelta = TimeDelta::days(30);
/// An ocid this old is resolved again before the next data call.
pub const OCID_RENEW_AFTER: TimeDelta = TimeDelta::days(29);
/// A row stamped further ahead than this was written under a wall clock
/// that has since stepped back: it is untrusted and deleted.
pub const FUTURE_SKEW: TimeDelta = TimeDelta::minutes(5);
/// The daily batch refreshes snapshots at least this old.
pub const REFRESH_AGE: TimeDelta = TimeDelta::hours(20);
/// The rolling window of the call budget.
pub const CALL_WINDOW: TimeDelta = TimeDelta::hours(24);
/// Hard stop on calls in any [`CALL_WINDOW`] (the development key allows
/// 1,000 a day).
pub const DAILY_CALLS: u32 = 800;
/// A tracked post is stripped this long after it was posted, a day before
/// its data would pass the TTL.
pub const STRIP_AFTER: TimeDelta = TimeDelta::days(29);
/// A failed strip (ambiguous, unsent or rate limited) is tried again this
/// long after its first failure, then every [`STRIP_RETRY_AGAIN`].
pub const STRIP_RETRY_FIRST: TimeDelta = TimeDelta::minutes(5);
pub const STRIP_RETRY_AGAIN: TimeDelta = TimeDelta::hours(1);
/// An admin's link request the member has not confirmed by then is removed.
pub const PENDING_TTL: TimeDelta = TimeDelta::days(14);
/// Bounds on stored Nexon text (names, classes, worlds) and image URLs.
pub const MAX_TEXT_CHARS: usize = 64;
pub const MAX_URL_CHARS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkStatus {
    /// Requested (by an admin) but not confirmed by the member: no calls.
    Pending,
    Active,
    /// The character could not be resolved or changed its name: the member
    /// is asked to link again; no calls.
    Stale,
}

/// `YYYY-MM-DD` (chrono's formatter is not compiled in).
pub fn date_text(date: NaiveDate) -> String {
    use chrono::Datelike;
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

/// The date at the start of `text` (`YYYY-MM-DD…`, e.g. a response's
/// `2025-01-21T00:00+08:00`).
pub fn parse_date(text: &str) -> Option<NaiveDate> {
    let head = text.get(..10)?;
    let mut parts = head.split('-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, day.parse().ok()?)
}

impl LinkStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Stale => "stale",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }

    /// The History value of a link in this state (section `characters`,
    /// key `link.<character>`); `unlinked` is the absent link.
    pub fn audit_value(status: Option<Self>) -> &'static str {
        match status {
            None => "unlinked",
            Some(Self::Pending) => "pending",
            Some(Self::Active) => "linked",
            Some(Self::Stale) => "stale",
        }
    }
}

/// Where the member gave their consent: Discord's `/character link` or the
/// public portal. Links consented before it was recorded have none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConsentSurface {
    Discord,
    Portal,
}

impl ConsentSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discord => "discord",
            Self::Portal => "portal",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "discord" => Some(Self::Discord),
            "portal" => Some(Self::Portal),
            _ => None,
        }
    }

    /// The History surface of a link change made here.
    pub fn history(self) -> Surface {
        match self {
            Self::Discord => Surface::Discord,
            Self::Portal => Surface::PublicPortal,
        }
    }
}

/// One `links` row: a roster character opted in to Nexon data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The roster character id (primary key).
    pub character_id: String,
    pub member_id: String,
    /// The roster IGN's case-insensitive key when the link was made.
    pub ign_key: String,
    pub region: String,
    pub status: LinkStatus,
    pub requested_by: Actor,
    pub requested_at: DateTime<Utc>,
    /// The notice version the member confirmed; `None` while pending.
    pub consent_version: Option<u32>,
    pub consented_at: Option<DateTime<Utc>>,
    /// When it first became active.
    pub linked_at: Option<DateTime<Utc>>,
    /// Where the consent was given; `None` while pending and on links
    /// consented before it was recorded.
    pub consent_surface: Option<ConsentSurface>,
    /// 1 on create, +1 on every status change.
    pub version: u32,
}

impl Link {
    /// # Errors
    /// [`StoreError::Constraint`] for a row no store may hold.
    pub fn check(&self) -> Result<(), StoreError> {
        let refuse = |what: &str| Err(StoreError::Constraint(format!("link {what}")));
        if self.character_id.is_empty() || self.member_id.is_empty() || self.ign_key.is_empty() {
            return refuse("needs a character, member and IGN");
        }
        if self.region != LINK_REGION {
            return refuse("must be on msea");
        }
        if self.version == 0 {
            return refuse("starts at version 1");
        }
        let consented = self.consent_version.is_some() && self.consented_at.is_some();
        if self.consent_version.is_some() != self.consented_at.is_some() {
            return refuse("consent needs both its version and time");
        }
        match self.status {
            LinkStatus::Pending if consented || self.consent_surface.is_some() => {
                refuse("pending has no consent")
            }
            LinkStatus::Active | LinkStatus::Stale if !consented || self.linked_at.is_none() => {
                refuse("active or stale needs consent and a link time")
            }
            _ => Ok(()),
        }
    }

    /// This link moved to `status` as its next version.
    pub fn with_status(mut self, status: LinkStatus) -> Self {
        self.status = status;
        self.version += 1;
        self
    }
}

/// One `snapshots` row: the Nexon data kept for an active link. Every field
/// from Nexon may be `None` (an empty response means "no data").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub character_id: String,
    /// Server-only: never in a DTO, a log, Discord or a scouter request.
    pub ocid: String,
    pub ocid_fetched_at: DateTime<Utc>,
    pub character_name: Option<String>,
    pub world_name: Option<String>,
    pub character_class: Option<String>,
    pub level: Option<u16>,
    pub combat_power: Option<u64>,
    /// Hotlinked, never downloaded.
    pub image_url: Option<String>,
    /// The response's reference date (SGT).
    pub data_date: Option<NaiveDate>,
    pub fetched_at: DateTime<Utc>,
    /// [`Snapshot::expiry`] of the two fetch times.
    pub expires_at: DateTime<Utc>,
}

impl Snapshot {
    /// When data fetched at `fetched_at` with an ocid fetched at
    /// `ocid_fetched_at` must be gone: the older one plus [`SNAPSHOT_TTL`].
    pub fn expiry(fetched_at: DateTime<Utc>, ocid_fetched_at: DateTime<Utc>) -> DateTime<Utc> {
        fetched_at.min(ocid_fetched_at) + SNAPSHOT_TTL
    }

    /// Readable at `now`: not expired and not stamped in the future.
    pub fn is_fresh(&self, now: DateTime<Utc>) -> bool {
        now < self.expires_at
            && self.fetched_at <= now + FUTURE_SKEW
            && self.ocid_fetched_at <= now + FUTURE_SKEW
    }

    /// The day the data describes: the response's reference date, else the
    /// fetch day in Singapore (the MapleSEA Open API's zone).
    pub fn as_of(&self) -> NaiveDate {
        self.data_date.unwrap_or_else(|| {
            self.fetched_at
                .with_timezone(&chrono_tz::Asia::Singapore)
                .date_naive()
        })
    }

    /// Any field beyond the ocid came back.
    pub fn has_data(&self) -> bool {
        self.character_name.is_some()
            || self.world_name.is_some()
            || self.character_class.is_some()
            || self.level.is_some()
            || self.combat_power.is_some()
            || self.image_url.is_some()
    }

    /// # Errors
    /// [`StoreError::Constraint`] for a row no store may hold.
    pub fn check(&self) -> Result<(), StoreError> {
        let refuse = |what: &str| Err(StoreError::Constraint(format!("snapshot {what}")));
        if self.character_id.is_empty() || self.ocid.is_empty() || self.ocid.len() > 128 {
            return refuse("needs a character and an ocid");
        }
        if self.expires_at != Self::expiry(self.fetched_at, self.ocid_fetched_at) {
            return refuse("expires 30 days after its older fetch");
        }
        let long = [
            &self.character_name,
            &self.world_name,
            &self.character_class,
        ]
        .into_iter()
        .flatten()
        .any(|text| text.chars().count() > MAX_TEXT_CHARS);
        if long
            || self
                .image_url
                .as_ref()
                .is_some_and(|url| url.len() > MAX_URL_CHARS)
        {
            return refuse("text is too long");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostState {
    Live,
    Stripping,
    /// The create's outcome is unknown and the post was not found again.
    Unknown,
    /// Neither edit nor delete was allowed: it needs a manual removal.
    Stuck,
}

impl PostState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Stripping => "stripping",
            Self::Unknown => "unknown",
            Self::Stuck => "stuck",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "live" => Some(Self::Live),
            "stripping" => Some(Self::Stripping),
            "unknown" => Some(Self::Unknown),
            "stuck" => Some(Self::Stuck),
            _ => None,
        }
    }
}

/// One `posts` row: a persistent Discord message showing Nexon data. It is
/// written before the send and deleted once its stripping is confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Post {
    /// 6 lowercase hex characters, carried in the post's footer.
    pub reference: String,
    pub channel_id: String,
    pub message_id: Option<String>,
    pub character_id: String,
    pub posted_at: DateTime<Utc>,
    pub strip_due_at: DateTime<Utc>,
    pub state: PostState,
}

impl Post {
    /// A post about to be sent at `at`: live, due [`STRIP_AFTER`] later.
    pub fn new(
        reference: String,
        channel_id: String,
        character_id: String,
        at: DateTime<Utc>,
    ) -> Self {
        Self {
            reference,
            channel_id,
            message_id: None,
            character_id,
            posted_at: at,
            strip_due_at: at + STRIP_AFTER,
            state: PostState::Live,
        }
    }

    /// # Errors
    /// [`StoreError::Constraint`] for a row no store may hold.
    pub fn check(&self) -> Result<(), StoreError> {
        let hex = self.reference.len() == 6
            && self
                .reference
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !hex || self.channel_id.is_empty() || self.character_id.is_empty() {
            return Err(StoreError::Constraint(
                "post needs a 6-hex ref, a channel and a character".into(),
            ));
        }
        Ok(())
    }

    /// Past its strip date at `now`.
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.strip_due_at <= now
    }

    /// Still unstripped when its data would pass the TTL.
    pub fn is_overdue(&self, now: DateTime<Utc>) -> bool {
        self.posted_at + SNAPSHOT_TTL <= now
    }
}

/// One `refresh_state` row: when a failed refresh may be tried again and
/// the Nexon error code (never a message or field) that stopped it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefreshState {
    pub character_id: String,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
}
