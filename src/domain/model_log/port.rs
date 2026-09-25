//! The model-log store port. Every write is atomic; logs are never updated,
//! only inserted and pruned.

use std::future::Future;

use chrono::{DateTime, Utc};

use super::filter::{ChatFilter, ExtractionFilter, LogFacets, LogPage};
use super::records::{
    AllowanceOverride, ChatInteraction, ExtractionLog, RescanJob, WatchedMessage,
};
use super::retention::PruneCounts;
use crate::domain::scheduler::StoreError;

/// What [`ModelLogStore::upsert_message`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageUpsert {
    Inserted,
    /// The content changed: content and `edited_at` replaced and
    /// `processed_at` cleared, so the message is read again.
    Edited,
    /// Same content; nothing written.
    Unchanged,
}

/// A cached message as a reader saw it: its content is the compare-and-set
/// token for [`ModelLogStore::mark_read`] (only an edit changes it).
#[derive(Clone, PartialEq, Eq)]
pub struct ReadMessage {
    pub id: String,
    pub content: String,
}

impl std::fmt::Debug for ReadMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadMessage")
            .field("id", &self.id)
            .field("content_len", &self.content.len())
            .finish()
    }
}

/// Extraction/chat persistence. Filter queries are constant SQL with bound
/// parameters (no caller-built SQL). Invalid shapes (a `guardrail` that is
/// not an object, `tool_calls`/`results` that are not arrays, a duplicate
/// id) are [`StoreError::Constraint`].
pub trait ModelLogStore {
    // Watched messages.

    /// Insert a message, or apply an edit to a cached one (the stored
    /// `created_at`, channel and author are kept).
    fn upsert_message(
        &self,
        message: WatchedMessage,
    ) -> impl Future<Output = Result<MessageUpsert, StoreError>> + Send;

    /// Mark cached messages processed at `at`; returns how many exist.
    fn mark_processed(
        &self,
        ids: &[String],
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// Mark read messages processed at `at`, each only if its content is
    /// still what was read; one edited meanwhile stays unprocessed. Returns
    /// how many were marked.
    fn mark_read(
        &self,
        read: &[ReadMessage],
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// Forget a deleted message; `true` when it was cached.
    fn delete_message(&self, id: &str) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// A channel's cached messages created at or after `since`, oldest
    /// first (`created_at`, then id); only unprocessed ones when asked.
    fn channel_messages(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
        unprocessed_only: bool,
    ) -> impl Future<Output = Result<Vec<WatchedMessage>, StoreError>> + Send;

    // Logs.

    fn record_extraction(
        &self,
        log: ExtractionLog,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    fn load_extraction(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<ExtractionLog>, StoreError>> + Send;

    fn list_extractions(
        &self,
        filter: &ExtractionFilter,
    ) -> impl Future<Output = Result<LogPage<ExtractionLog>, StoreError>> + Send;

    fn extraction_facets(&self) -> impl Future<Output = Result<LogFacets, StoreError>> + Send;

    /// Insert an interaction with its rounds.
    fn record_chat(
        &self,
        interaction: ChatInteraction,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    fn load_chat(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<ChatInteraction>, StoreError>> + Send;

    fn list_chats(
        &self,
        filter: &ChatFilter,
    ) -> impl Future<Output = Result<LogPage<ChatInteraction>, StoreError>> + Send;

    fn chat_facets(&self) -> impl Future<Output = Result<LogFacets, StoreError>> + Send;

    /// Delete extraction and chat logs with `at` before `before`, and
    /// processed cached messages created before it (unprocessed ones are
    /// kept). Runs in write transactions of at most
    /// [`PRUNE_BATCH`](super::PRUNE_BATCH) rows per table until nothing is
    /// left, so other writers interleave; each batch is atomic.
    fn prune_model_logs(
        &self,
        before: DateTime<Utc>,
    ) -> impl Future<Output = Result<PruneCounts, StoreError>> + Send;

    // Rescan jobs.

    fn insert_rescan_job(
        &self,
        job: RescanJob,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Replace a job's status, times, results and error (its request fields
    /// never change). `false` (nothing written) when the job is missing or
    /// already final.
    fn update_rescan_job(
        &self,
        job: RescanJob,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    fn load_rescan_job(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<RescanJob>, StoreError>> + Send;

    /// Newest first (`created_at`, then id), at most `limit`.
    fn recent_rescan_jobs(
        &self,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<RescanJob>, StoreError>> + Send;

    // Chat allowance overrides.

    fn set_allowance_override(
        &self,
        entry: AllowanceOverride,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// `true` when an override was removed.
    fn clear_allowance_override(
        &self,
        member_id: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Every override, by member id.
    fn allowance_overrides(
        &self,
    ) -> impl Future<Output = Result<Vec<AllowanceOverride>, StoreError>> + Send;

    // Self-service tips.

    /// Claim the member's one tip for the boss week starting `week`;
    /// `false` when it was already claimed (nothing written). Atomic, so
    /// concurrent claims grant exactly one.
    fn claim_tip(
        &self,
        member_id: &str,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Give a claimed tip back (its post never went out); `true` when one
    /// was held.
    fn release_tip(
        &self,
        member_id: &str,
        week: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;
}
