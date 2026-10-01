//! The delivery journal port: every outbound post is claimed durably before
//! transport and bound to its native rows after, so it is sent at most once
//! (v4 `maintenance/delivery/journal.py`).
//!
//! Lifecycle: [`DeliveryJournal::claim`] records an `intent`; the executor
//! sends, then [`bind`](DeliveryJournal::bind)s the receipt, or marks the
//! attempt `indeterminate` when the outcome is unknown, or retires it when
//! Discord definitely rejected it. Unresolved attempts keep their targets
//! held, so a restart never resends them. Timestamps are always passed in.

use std::collections::BTreeSet;
use std::fmt;
use std::future::Future;

use chrono::{DateTime, Utc};
use ring::digest::{SHA256, digest};

use super::digest::WeeklyDigest;
use super::intent::{DeliveryTarget, JournalView, NotificationIntent};
use crate::domain::time::DateOutOfRange;

/// v4's dedupe-key namespace.
pub const DEDUPE_NAMESPACE: &str = "kanade.delivery.dedupe.v1";
/// Actor recorded when a definitely rejected send is retired.
pub const REJECTED_ACTOR: &str = "service:delivery-rejected";
/// Actor recorded when a send that never reached Discord is released. Such a
/// retirement is proven unsent, so it never counts as unproven.
pub const NOT_SENT_ACTOR: &str = "service:delivery-not-sent";
/// v4 `_DIGEST_REPLACEMENT_ACTOR` / `_REASON`.
pub const DIGEST_REPLACEMENT_ACTOR: &str = "service:digest-replacement";
pub const DIGEST_REPLACEMENT_REASON: &str = "confirmed Discord deletion for digest replacement";
/// v4's resolved-by metadata for a confirmed decline-notice deletion.
pub const DECLINE_RETRACTION_ACTOR: &str = "service:decline-retraction";
/// v4's resolved-by reason for a confirmed decline-notice deletion.
pub const DECLINE_RETRACTION_REASON: &str = "confirmed Discord deletion for decline retraction";
/// The config key holding the last posted digest week.
pub const DIGEST_MARKER_KEY: &str = "last_digest_week";

fn sha256_hex(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// One journalled send.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AttemptId(pub String);

impl fmt::Display for AttemptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// v4 `delivery_attempts.state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttemptState {
    /// Claimed; the send may be in flight.
    Intent,
    /// The send's outcome is unknown; its targets stay held.
    Indeterminate,
    /// Delivered and bound to its native rows.
    Bound,
    /// Resolved without (or after) a live message; no longer held.
    Retired,
}

impl AttemptState {
    pub const ALL: &[Self] = &[
        Self::Intent,
        Self::Indeterminate,
        Self::Bound,
        Self::Retired,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::Indeterminate => "indeterminate",
            Self::Bound => "bound",
            Self::Retired => "retired",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|state| state.as_str() == value)
    }

    /// Unresolved attempts may have delivered, so they hold their targets.
    pub fn is_unresolved(self) -> bool {
        matches!(self, Self::Intent | Self::Indeterminate)
    }
}

/// v4 `_dedupe_key`: SHA-256 over a namespaced JSON identity.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DedupeKey(String);

impl DedupeKey {
    fn hash(scope: &str, identity: serde_json::Value) -> Self {
        let payload = serde_json::json!([DEDUPE_NAMESPACE, scope, identity]);
        Self(sha256_hex(payload.to_string().as_bytes()))
    }

    /// Native scope: the sorted `(binding_type, key_primary, key_secondary)`
    /// claim keys of the targets.
    ///
    /// # Errors
    /// [`DateOutOfRange`] for a digest week outside v4's years.
    pub fn native(targets: &[DeliveryTarget]) -> Result<Self, DateOutOfRange> {
        let mut keys = targets
            .iter()
            .map(claim_key)
            .collect::<Result<Vec<_>, _>>()?;
        keys.sort();
        let identity = keys
            .into_iter()
            .map(|(kind, primary, secondary)| serde_json::json!([kind, primary, secondary]))
            .collect();
        Ok(Self::hash("native", serde_json::Value::Array(identity)))
    }

    /// Operation scope: one effect of one leased operation.
    pub fn operation(operation_id: &str, ordinal: i64) -> Self {
        Self::hash("operation", serde_json::json!([operation_id, ordinal]))
    }

    /// Source scope: one effect of a durable source (an outbox notice's
    /// `(source, ordinal)`), whatever lease claims it.
    pub fn source(source: &str, ordinal: i64) -> Self {
        Self::hash("source", serde_json::json!([source, ordinal]))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// v4 `DeliveryTarget.claim_key`; reminders and digests have no secondary key.
///
/// # Errors
/// [`DateOutOfRange`] for a digest week outside v4's years.
pub fn claim_key(target: &DeliveryTarget) -> Result<(String, String, String), DateOutOfRange> {
    let secondary = match target {
        DeliveryTarget::DebugCard { kind, .. } => kind.clone(),
        DeliveryTarget::Decline { user_id, .. } => user_id.clone(),
        _ => String::new(),
    };
    Ok((
        target.binding_type().to_owned(),
        target.key_primary()?,
        secondary,
    ))
}

/// A content-free hash of what was asked for (fingerprint version 2: v5
/// hashes the intent, not a rendered payload).
pub fn request_fingerprint(intent: &NotificationIntent) -> String {
    let targets: Vec<serde_json::Value> = intent
        .targets
        .iter()
        .map(|target| {
            serde_json::json!([
                target.binding_type(),
                target.key_primary().unwrap_or_default()
            ])
        })
        .collect();
    let value = serde_json::json!({
        "fingerprint_version": REQUEST_FINGERPRINT_VERSION,
        "effect": intent.effect.as_str(),
        "effect_context": intent.effect_context,
        "channel_id": intent.channel_id,
        "targets": targets,
        "mentions": intent.mentions,
    });
    sha256_hex(value.to_string().as_bytes())
}

pub const REQUEST_FINGERPRINT_VERSION: i64 = 2;

/// What Discord returned for an accepted post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receipt {
    /// The channel the message actually landed in.
    pub channel_id: String,
    pub message_id: String,
}

/// A live operation lease: journal writes require one (OPEN mode only).
#[derive(Clone, PartialEq, Eq)]
pub struct Lease {
    pub operation_id: String,
    pub instance_id: String,
    pub generation: i64,
    token: String,
}

impl Lease {
    pub fn new(operation_id: String, instance_id: String, generation: i64, token: String) -> Self {
        Self {
            operation_id,
            instance_id,
            generation,
            token,
        }
    }

    /// The stored proof of ownership; the token itself is never persisted.
    pub fn token_hash(&self) -> String {
        sha256_hex(self.token.as_bytes())
    }
}

impl fmt::Debug for Lease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lease")
            .field("operation_id", &self.operation_id)
            .field("instance_id", &self.instance_id)
            .field("generation", &self.generation)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// The ordinal an effect takes in its operation (v4 `_persist_intent`):
/// `requested` for a target-less effect, else `next`.
///
/// # Errors
/// [`JournalError::InvalidInput`] for an ordinal on an intent with targets,
/// a negative one, or one past `next`.
pub fn effect_ordinal(
    intent: &NotificationIntent,
    requested: Option<i64>,
    next: i64,
) -> Result<i64, JournalError> {
    match requested {
        None => Ok(next),
        Some(_) if !intent.operation_scoped() => Err(JournalError::InvalidInput(
            "effect ordinals key target-less effects only".into(),
        )),
        Some(ordinal) if ordinal < 0 || ordinal > next => Err(JournalError::InvalidInput(format!(
            "effect ordinal {ordinal} skips ahead of {next}"
        ))),
        Some(ordinal) => Ok(ordinal),
    }
}

/// The result of a claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Claim {
    /// A new attempt now holds the targets; send, then bind or resolve it.
    Fresh(AttemptId),
    /// An earlier active attempt holds these targets; send nothing.
    Held,
}

/// Targets held by unresolved attempts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActiveClaims {
    held: BTreeSet<DeliveryTarget>,
}

impl ActiveClaims {
    pub fn new(held: BTreeSet<DeliveryTarget>) -> Self {
        Self { held }
    }

    pub fn targets(&self) -> &BTreeSet<DeliveryTarget> {
        &self.held
    }
}

impl JournalView for ActiveClaims {
    fn holds(&self, target: &DeliveryTarget) -> bool {
        self.held.contains(target)
    }
}

/// One attempt as recorded, for admin reports and checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptRecord {
    pub id: AttemptId,
    pub state: AttemptState,
    /// The claimed channel; after binding, the receipt's channel.
    pub channel_id: String,
    pub message_id: Option<String>,
    pub resolved_by: Option<String>,
    pub resolution_reason: String,
    pub targets: Vec<DeliveryTarget>,
}

/// The digest bookkeeping a weekly tick reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DigestLog {
    /// `last_digest_week` as stored (UTC ISO text).
    pub last_digest_week: Option<String>,
    /// Every week's card, active or retired, by week.
    pub digests: Vec<WeeklyDigest>,
}

/// What start-up recovery changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Attempts left in `intent` by the previous process.
    pub indeterminate: Vec<AttemptId>,
    pub orphaned_leases: usize,
}

/// Journal refusals and failures; nothing was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JournalError {
    /// The lease is not live, or the store is not OPEN.
    LeaseNotLive,
    /// A native target is gone, already sent, active, or retired unproven.
    TargetUnavailable(String),
    /// The attempt or its native rows are not in the expected state.
    StateChanged(String),
    /// A target cannot be keyed (e.g. a week outside v4's years), or a
    /// resolution actor/reason is empty or too long.
    InvalidInput(String),
    Backend(String),
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LeaseNotLive => f.write_str("delivery lease is not live"),
            Self::TargetUnavailable(detail) => write!(f, "delivery target unavailable: {detail}"),
            Self::StateChanged(detail) => write!(f, "delivery state changed: {detail}"),
            Self::InvalidInput(detail) => write!(f, "invalid delivery input: {detail}"),
            Self::Backend(detail) => write!(f, "delivery journal failed: {detail}"),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<DateOutOfRange> for JournalError {
    fn from(error: DateOutOfRange) -> Self {
        Self::InvalidInput(error.to_string())
    }
}

/// The stored bounds on who resolved an attempt and why (space-trimmed lengths
/// 1..=128 and 1..=512, as the schema's CHECKs).
///
/// # Errors
/// [`JournalError::InvalidInput`] naming the offending field.
pub fn check_resolution(actor: &str, reason: &str) -> Result<(), JournalError> {
    let within =
        |text: &str, max: usize| (1..=max).contains(&text.trim_matches(' ').chars().count());
    if !within(actor, 128) {
        return Err(JournalError::InvalidInput("resolution actor".into()));
    }
    if !within(reason, 512) {
        return Err(JournalError::InvalidInput("resolution reason".into()));
    }
    Ok(())
}

/// Durable delivery bookkeeping. Every write is one atomic transaction;
/// writes that change native rows advance the schedule store's revision.
pub trait DeliveryJournal {
    /// Targets held by unresolved attempts, for planning.
    fn load_view(&self) -> impl Future<Output = Result<ActiveClaims, JournalError>> + Send;

    fn load_attempt(
        &self,
        attempt: &AttemptId,
    ) -> impl Future<Output = Result<Option<AttemptRecord>, JournalError>> + Send;

    fn load_digests(&self) -> impl Future<Output = Result<DigestLog, JournalError>> + Send;

    /// Start a live operation lease for this process.
    fn begin_lease(
        &self,
        instance_id: &str,
        operation_kind: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<Lease, JournalError>> + Send;

    fn end_lease(
        &self,
        lease: &Lease,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;

    /// Under a live lease: [`Claim::Held`] when an active attempt has the same
    /// dedupe key or holds any target; otherwise every target must still exist
    /// and be unsent (no active digest card, no unproven retirement), and a
    /// new `intent` attempt claims them all.
    ///
    /// A target-less intent (a notice) is keyed by its effect ordinal within
    /// the lease's operation: `effect_ordinal` repeats a caller-chosen ordinal
    /// so a retried effect is [`Claim::Held`] (v4 operation scope); `None`
    /// takes the next one. Ordinals may not skip ahead and are refused for
    /// intents with targets.
    fn claim(
        &self,
        lease: &Lease,
        intent: &NotificationIntent,
        effect_ordinal: Option<i64>,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<Claim, JournalError>> + Send;

    /// Under a live lease, claim a target-less `intent` by its durable source
    /// (an outbox notice) instead of the lease's operation, so a claim made
    /// under an earlier lease (a crash) still holds it. [`Claim::Held`] while
    /// any attempt with that key exists other than one released as
    /// [`NOT_SENT_ACTOR`]: an active one (including bound and indeterminate)
    /// or one retired rejected or unproven is never claimed again. The new
    /// attempt takes the lease's next effect ordinal.
    ///
    /// # Errors
    /// [`JournalError::InvalidInput`] for an intent with targets.
    fn claim_source(
        &self,
        lease: &Lease,
        intent: &NotificationIntent,
        source: &str,
        ordinal: i64,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<Claim, JournalError>> + Send;

    /// Bind a delivered `intent`: stamp every reminder target sent with the
    /// message id, or record the week's digest card (and raise the digest
    /// marker to `record_week`), and record the receipt's channel. The receipt
    /// must name the claimed channel.
    fn bind(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        receipt: &Receipt,
        record_week: Option<DateTime<Utc>>,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;

    /// The send's outcome is unknown: keep its targets held.
    fn mark_indeterminate(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;

    /// Discord definitely refused an `intent`: retire it with `reason` and
    /// mark its native rows handled (reminders sent without a message, the
    /// digest week recorded) so it is never retried.
    fn retire_rejected(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        reason: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;

    /// v4 `_retire_digest_replacement`: after its message was confirmed
    /// deleted, retire the week's active card, release its exact bound claim
    /// and retire that attempt. Refused unless such a claim exists.
    fn retire_for_replacement(
        &self,
        lease: &Lease,
        digest: &WeeklyDigest,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;

    /// After Discord confirmed a decline notice is gone, atomically clear its
    /// exact binding, release the target and retire its bound attempt.
    fn retire_decline_retraction(
        &self,
        lease: &Lease,
        run_id: &str,
        user_id: &str,
        channel_id: &str,
        message_id: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;

    /// Resolve a pending retraction after the create was proven unsent. The
    /// retired attempt becomes the durable no-replay marker for that candidate.
    fn resolve_decline_retract_pending(
        &self,
        lease: &Lease,
        run_id: &str,
        user_id: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, JournalError>> + Send;

    /// Operator recovery: retire an unresolved attempt whose own lease is no
    /// longer live, without proof of delivery. Its reminders are never
    /// reopened afterwards.
    fn retire_unproven(
        &self,
        attempt: &AttemptId,
        actor: &str,
        reason: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;

    /// The transport proved an `intent` of this lease's operation was never
    /// delivered (not sent, rate limited): release its targets and dedupe key
    /// and retire it as [`NOT_SENT_ACTOR`] with `reason`, leaving native rows
    /// untouched so the next claim is fresh. Refused for any other state or
    /// for another operation's attempt.
    fn release_unsent(
        &self,
        lease: &Lease,
        attempt: &AttemptId,
        reason: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;

    /// Raise `last_digest_week` to `week` without a post (first tick, no post
    /// channel): never backwards, never to a week after `at`. Idempotent.
    fn record_digest_week(
        &self,
        lease: &Lease,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), JournalError>> + Send;

    /// v4 `retire_weekly_digests_before`: stamp `retired_at = at` on active
    /// cards of weeks before `week`, keeping them as the weekly log and
    /// leaving their attempts alone. Returns how many were retired.
    fn retire_digests_before(
        &self,
        lease: &Lease,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<usize, JournalError>> + Send;

    /// After taking ownership: every `intent` becomes `indeterminate` and every
    /// live lease is orphaned.
    fn recover_on_start(
        &self,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<Recovery, JournalError>> + Send;
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn lease_debug_redacts_the_token() {
        let lease = Lease::new("op".into(), "instance".into(), 0, "s3cret-token".into());
        let shown = format!("{lease:?}");
        assert!(!shown.contains("s3cret-token"), "{shown}");
        assert!(
            shown.contains("op") && shown.contains("<redacted>"),
            "{shown}"
        );
    }

    /// Hashes printed by running v4 `DeliveryJournal._dedupe_key` itself
    /// (the rollback tree, via `uv run`), not re-derived.
    #[test]
    fn dedupe_keys_match_v4() {
        let reminders = [
            DeliveryTarget::Reminder("r-2".into()),
            DeliveryTarget::Reminder("r-1".into()),
        ];
        assert_eq!(
            DedupeKey::native(&reminders).unwrap().as_str(),
            "4e8d3b5c4bdc455e53692b497f2e26d1ea704c7a59fee000577ad2ad85956191"
        );
        let week = Utc.with_ymd_and_hms(2026, 8, 26, 16, 0, 0).unwrap();
        assert_eq!(
            DedupeKey::native(&[DeliveryTarget::Digest(week)])
                .unwrap()
                .as_str(),
            "e58490b332c0121e5a9a9fcfa017fc923b08e207fff1194fb2a869dddd61d788"
        );
        assert_eq!(
            DedupeKey::native(&[DeliveryTarget::Reminder("ü\"x".into())])
                .unwrap()
                .as_str(),
            "bc05cae04ce4376b7aad0c6159b3be2abf09d270da78e957a104e41746054bcf"
        );
        assert_eq!(
            DedupeKey::operation("op-1", 3).as_str(),
            "ea9d061598a0b9f6f89e25269c9e7e5767fe8f7f8e2e6e4956e0d67effbd3c31"
        );
    }
}
