//! The guide store port. Object-safe (one store per process, held as
//! `Arc<dyn GuideStore>` by the API and the publish worker); every write is
//! one transaction. Slot truth changes only after Discord confirmed an
//! effect or proved it gone, so the map is exact after a cancel or crash.
//!
//! Keyed (`Idempotency-Key`) forms of the admin writes live beside the other
//! replayed writes in the store's `ReplayStore` (scope `config`): the same
//! write plus its replay row in one transaction, nothing written on refusal.

use std::pin::Pin;

use chrono::{DateTime, Utc};

use super::blocks::Block;
use super::plan::{Confirmations, PlanCounts, PlanOp, PublishedSlot};
use crate::history::{Actor, Surface};
use crate::scheduler::StoreError;

pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StoreError>> + Send + 'a>>;

/// History's settings section of guide writes and publishes (decision 12).
pub const GUIDES_SECTION: &str = "guides";

/// One guide (`guides`): its draft and target channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guide {
    /// A hyphenated UUID.
    pub id: String,
    /// 1-100 characters.
    pub title: String,
    /// Unique across guides; `None` until an admin picks one.
    pub channel_id: Option<String>,
    pub blocks: Vec<Block>,
    /// Starts at 1; every saved change takes the next.
    pub version: u32,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

/// One guide write. `version` is the version the admin read: another
/// version is [`GuideWrite::Stale`] and nothing is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuideChange {
    /// A new guide at version 1 with no blocks.
    Create {
        id: String,
        title: String,
        channel_id: Option<String>,
    },
    /// The full form; an unchanged form is a no-op that writes nothing.
    Save {
        id: String,
        version: u32,
        title: String,
        channel_id: Option<String>,
        blocks: Vec<Block>,
    },
    /// Refused while a run is queued or running ([`GuideWrite::Busy`]), or
    /// while any slot is posted or unknown ([`GuideWrite::Published`],
    /// decision 18). Saves stay open during a run: the worker re-plans
    /// against the saved guide and stops `plan_changed` if it moved.
    Delete { id: String, version: u32 },
}

/// What a guide write did. Only the first three wrote anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuideWrite {
    Created(Guide),
    /// The row as stored now (unchanged by a no-op).
    Saved(Guide),
    /// The row as it was.
    Deleted(Guide),
    Missing,
    /// The row as it is now.
    Stale(Guide),
    /// Another guide targets the channel.
    ChannelTaken,
    /// A delete while messages are published (unpublish first).
    Published,
    /// A delete while a run is queued or running (it may still post).
    Busy,
}

impl GuideWrite {
    pub fn applied(&self) -> bool {
        matches!(self, Self::Created(_) | Self::Saved(_) | Self::Deleted(_))
    }
}

/// Who made a write, and when, for its History record (admin writes; run
/// outcomes are recorded as `system:guides`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuideRecord {
    pub at: DateTime<Utc>,
    pub actor: Actor,
    pub surface: Surface,
}

crate::spelled! {
    /// A publish run's kind.
    PublishKind {
        Publish => "publish",
        /// Delete every published message.
        Unpublish => "unpublish",
        /// Search the channel again for the guide's unknown slots (5.3).
        Search => "search",
    }
}

crate::spelled! {
    /// A publish run's lifecycle; `done`, `stopped` and `cancelled` are final.
    PublishState {
        Queued => "queued",
        Running => "running",
        Done => "done",
        Stopped => "stopped",
        Cancelled => "cancelled",
    }
}

crate::spelled! {
    /// Why a run stopped before its last op (5.1-5.5).
    StopReason {
        /// The recomputed plan's hash differs from the confirmed one, or an
        /// edit found its message gone: nothing more was sent.
        PlanChanged => "plan_changed",
        /// A create could not be confirmed or found.
        UnknownSlot => "unknown_slot",
        EditUnconfirmed => "edit_unconfirmed",
        /// A delete could not be confirmed; no create of the run followed.
        DeleteUnconfirmed => "delete_unconfirmed",
        /// Discord refused an op (e.g. missing permissions).
        Rejected => "rejected",
        /// Discord did not take a create (not sent, rate limited).
        DiscordUnavailable => "discord_unavailable",
        /// The process stopped; runs never resume on their own.
        Interrupted => "interrupted",
    }
}

crate::spelled! {
    /// One op's result (`guide_publish_ops.outcome`).
    OpOutcome {
        /// Recorded before the effect is attempted.
        Pending => "pending",
        Done => "done",
        /// The message was already gone.
        Gone => "gone",
        /// An ambiguous create found in the channel.
        Recovered => "recovered",
        Unknown => "unknown",
        Rejected => "rejected",
        /// Not sent; nothing landed.
        Skipped => "skipped",
    }
}

/// One publish run (`guide_publishes`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Publish {
    pub id: String,
    pub guide_id: String,
    /// The guide version it renders.
    pub version: u32,
    /// The confirmed [`super::Plan::hash`].
    pub plan_hash: String,
    pub channel_id: String,
    pub kind: PublishKind,
    pub confirmed: Confirmations,
    /// Delete the old channel's messages after a channel move (decision 8).
    pub delete_old: bool,
    pub state: PublishState,
    pub stop_reason: Option<StopReason>,
    pub requested_by: String,
    pub requested_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub cancel_requested_at: Option<DateTime<Utc>>,
}

impl Publish {
    pub fn live(&self) -> bool {
        matches!(self.state, PublishState::Queued | PublishState::Running)
    }
}

/// One op of a run's log (`guide_publish_ops`), recorded `pending` before
/// its effect is attempted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishOp {
    pub ordinal: u32,
    pub op: PlanOp,
    /// A create's journal source, `guide-post:<op uuid>`; never reused.
    pub source: Option<String>,
    pub outcome: OpOutcome,
    /// The messages an unconfirmed create's channel search matched (zero or
    /// several make the slot unknown); empty otherwise.
    pub candidates: Vec<String>,
    pub at: DateTime<Utc>,
}

/// What an op result does to the slot map, in the op result's transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlotEffect {
    /// Nothing confirmed (a retryable edit, a skipped create).
    Unchanged,
    /// Insert or replace the slot at its `(channel_id, position)`.
    Put(PublishedSlot),
    /// Drop the slot at `(channel_id, position)`, if present.
    Remove { channel_id: String, position: u32 },
}

/// A publish (or unpublish or search) request: the run row, `queued`, and
/// the counts its History record shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishRequest {
    pub publish: Publish,
    pub counts: PlanCounts,
}

/// What [`GuideStore::request_publish`] did. Only `Queued` wrote anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishRequested {
    Queued(Publish),
    Missing,
    /// The guide is at another version; the guide as it is now.
    Stale(Guide),
    /// The guide already has a live run (one per guide); that run.
    Busy(Publish),
}

/// What [`GuideStore::request_cancel`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CancelRequest {
    /// The run as stored now (`cancel_requested_at` set).
    Requested(Publish),
    /// The run had already finished; the run as it is.
    Finished(Publish),
    Missing,
}

/// What [`GuideStore::mark_not_posted`] did. Only `Removed` wrote anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotPosted {
    /// The unknown slot as it was; the next plan creates it again.
    Removed(PublishedSlot),
    Missing,
    /// The slot is posted or gone, not unknown.
    NotUnknown,
    /// The guide has a live run.
    Busy,
}

/// Persistence of `guides`, `guide_slots`, `guide_publishes` and
/// `guide_publish_ops`. Admin writes append their History record
/// ([`GUIDES_SECTION`]) in the same transaction; a refused write records
/// nothing.
pub trait GuideStore: Send + Sync {
    /// Every guide, by title then id.
    fn guides(&self) -> StoreFuture<'_, Vec<Guide>>;
    fn guide(&self, id: String) -> StoreFuture<'_, Option<Guide>>;
    /// Create, save or delete a guide (with its slots and runs) under the
    /// rules on [`GuideChange`]. Draft saves record no History row; create,
    /// rename, channel change and delete do (`guide.<id>`).
    fn apply_guide(&self, change: GuideChange, record: GuideRecord) -> StoreFuture<'_, GuideWrite>;

    /// The guide's published slots, by channel then position.
    fn slots(&self, guide_id: String) -> StoreFuture<'_, Vec<PublishedSlot>>;

    /// Queue a run after checking the guide version and that no run is
    /// live, with its History record (`publish.<id>`) in one transaction.
    fn request_publish(
        &self,
        request: PublishRequest,
        record: GuideRecord,
    ) -> StoreFuture<'_, PublishRequested>;
    /// One run with its op log, by ordinal.
    fn publish(&self, id: String) -> StoreFuture<'_, Option<(Publish, Vec<PublishOp>)>>;
    /// The guide's newest run, live or finished.
    fn latest_publish(&self, guide_id: String) -> StoreFuture<'_, Option<Publish>>;
    /// The oldest queued run of any guide, now `running` from `at`.
    fn claim_next(&self, at: DateTime<Utc>) -> StoreFuture<'_, Option<Publish>>;
    /// Append a `pending` op to a running run's log.
    fn record_op(&self, publish_id: String, op: PublishOp) -> StoreFuture<'_, ()>;
    /// Settle a pending op and apply its slot effect in one transaction;
    /// `candidates` are the search matches of an unconfirmed create
    /// ([`PublishOp::candidates`]).
    fn apply_op_result(
        &self,
        publish_id: String,
        ordinal: u32,
        outcome: OpOutcome,
        effect: SlotEffect,
        candidates: Vec<String>,
        at: DateTime<Utc>,
    ) -> StoreFuture<'_, ()>;
    /// End a live run (`done`, `stopped` with a reason, or `cancelled`)
    /// with its outcome History record (`system:guides`); the run as stored.
    fn finish(
        &self,
        publish_id: String,
        state: PublishState,
        stop_reason: Option<StopReason>,
        at: DateTime<Utc>,
    ) -> StoreFuture<'_, Publish>;
    /// Ask a live run to stop between ops (5.4).
    fn request_cancel(
        &self,
        guide_id: String,
        publish_id: String,
        at: DateTime<Utc>,
    ) -> StoreFuture<'_, CancelRequest>;
    /// "Mark as not posted" (5.3): drop an unknown slot, with its History
    /// record (`slot.<guide id>.<position>`). The journal attempt stays
    /// indeterminate as the no-replay marker.
    fn mark_not_posted(
        &self,
        guide_id: String,
        channel_id: String,
        position: u32,
        record: GuideRecord,
    ) -> StoreFuture<'_, NotPosted>;
    /// On start (5.5): every queued run ends `stopped: interrupted`; the
    /// running runs come back with their op logs for reconciliation, after
    /// which the worker finishes them `interrupted`.
    fn interrupted_on_start(
        &self,
        at: DateTime<Utc>,
    ) -> StoreFuture<'_, Vec<(Publish, Vec<PublishOp>)>>;
}
