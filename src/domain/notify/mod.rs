//! Notification policy ported from v4 `bot/agent/{pings,client}` as a pure core.
//!
//! Planners read scheduler snapshots, schedule change notices and explicit
//! member, channel and journal views, and return [`NotificationIntent`]s; they
//! never send, write or read a clock. Executing intents belongs to the journal.

mod audience;
mod decline;
mod digest;
mod dispatch;
mod intent;
mod journal;
mod notice;
mod outbox;
mod policy;

pub use audience::{
    Audience, audience, display_names, everyone_on, not_declined, resolve_mentions, swap_audience,
};
pub use decline::{
    DECLINE_NOTICE_COOLDOWN, DeclineNotice, DeclineNoticeStore, MAX_PENDING_DECLINE_NOTICES,
};
pub use digest::{
    DigestAction, DigestDay, DigestInclusion, DigestPostInput, DigestSend, DigestTick,
    RecordReason, WeekReset, WeeklyDigest, digest_inclusion, plan_digest_post, plan_digest_tick,
    retire_digests_before,
};
pub use dispatch::{
    DeliverySettings, DispatchInput, DispatchPlan, Queued, ReminderClass, Retirement,
    SuppressReason, classify, countdown_minutes, due_reminders, plan_dispatch,
};
pub use intent::{
    ChannelChoice, ChannelDirectory, DeliveryTarget, DeliveryWarning, EffectKind, IntentContent,
    JournalView, NotificationIntent, PlannedSend, SendDisposition, choose_channel,
};
pub use journal::{
    ActiveClaims, AttemptId, AttemptRecord, AttemptState, Claim, DECLINE_RETRACTION_ACTOR,
    DECLINE_RETRACTION_REASON, DEDUPE_NAMESPACE, DIGEST_MARKER_KEY, DIGEST_REPLACEMENT_ACTOR,
    DIGEST_REPLACEMENT_REASON, DedupeKey, DeliveryJournal, DigestLog, JournalError, Lease,
    NOT_SENT_ACTOR, REJECTED_ACTOR, REQUEST_FINGERPRINT_VERSION, Receipt, Recovery,
    check_resolution, claim_key, effect_ordinal, request_fingerprint,
};
pub use notice::plan_notice;
pub use outbox::{
    DEFAULT_MAX_NOTICE_AGE, DrainReason, NoticeOutbox, OutboxNotice, PendingNotices,
    UndecodableNotice, change_source, draft_source,
};
pub use policy::{AllowedMentions, PingKind, allowed_mentions, wants_mention};
