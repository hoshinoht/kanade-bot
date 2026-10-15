//! The character-data store port. Object-safe (one store per process, held
//! as `Arc<dyn CharDataStore>`); every write is one transaction.

use std::pin::Pin;

use chrono::{DateTime, Utc};

use super::estimate::{DeletedEstimates, MemberEstimate, PartyEstimate, StoredEstimates};
use super::model::{Link, Post, PostState, RefreshState, Snapshot};
use crate::domain::scheduler::StoreError;

pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StoreError>> + Send + 'a>>;

/// What [`CharDataStore::put_link`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkPut {
    Written,
    /// The stored version is not the expected one; the stored link (`None`
    /// when there is none). Nothing was written.
    Stale(Option<Link>),
}

/// Persistence of the character-data file. Reads of Nexon data take `now`
/// and refuse rows that are expired or stamped in the future, so a stalled
/// sweeper never shows them.
pub trait CharDataStore: Send + Sync {
    fn link(&self, character_id: String) -> StoreFuture<'_, Option<Link>>;
    /// Every link, by character id.
    fn links(&self) -> StoreFuture<'_, Vec<Link>>;
    /// Insert `link` when `expected` is 0 and no link exists, else replace
    /// the stored link at version `expected`. A link written `pending` or
    /// `stale` drops its snapshot and refresh state in the same write.
    /// [`StoreError::Constraint`] for a row that fails [`Link::check`] or a
    /// replacement that does not take the next version.
    fn put_link(&self, link: Link, expected: u32) -> StoreFuture<'_, LinkPut>;
    /// Delete the link with its snapshot and refresh state, and make every
    /// tracked post of the character due at `at`, in one write; the link
    /// as it was, `None` when there was none.
    fn unlink(&self, character_id: String, at: DateTime<Utc>) -> StoreFuture<'_, Option<Link>>;

    /// The snapshot if it is fresh at `now` ([`Snapshot::is_fresh`]).
    fn snapshot(
        &self,
        character_id: String,
        now: DateTime<Utc>,
    ) -> StoreFuture<'_, Option<Snapshot>>;
    /// Insert or replace the character's snapshot while its link is
    /// active; `false` (nothing written) otherwise.
    fn put_snapshot(&self, snapshot: Snapshot) -> StoreFuture<'_, bool>;
    /// Delete snapshots expired at `now` or stamped after `now` plus
    /// [`super::FUTURE_SKEW`]; how many.
    fn sweep_snapshots(&self, now: DateTime<Utc>) -> StoreFuture<'_, u64>;

    fn refresh_states(&self) -> StoreFuture<'_, Vec<RefreshState>>;
    /// Insert or replace while the character is linked; `false` otherwise.
    fn put_refresh_state(&self, state: RefreshState) -> StoreFuture<'_, bool>;

    /// Record a call at `at` unless `cap` calls already fall in the window
    /// ending at `at` (calls stamped later count too); `false` when full.
    fn reserve_call(&self, at: DateTime<Utc>, cap: u32) -> StoreFuture<'_, bool>;
    /// Calls stamped after `since`.
    fn calls_since(&self, since: DateTime<Utc>) -> StoreFuture<'_, u32>;
    /// Delete calls stamped at or before `before`; how many.
    fn prune_calls(&self, before: DateTime<Utc>) -> StoreFuture<'_, u64>;

    /// Track a post before it is sent; `false` when its ref is taken.
    fn create_post(&self, post: Post) -> StoreFuture<'_, bool>;
    /// Every tracked post, oldest first.
    fn posts(&self) -> StoreFuture<'_, Vec<Post>>;
    fn set_post_message(&self, reference: String, message_id: String) -> StoreFuture<'_, bool>;
    fn set_post_state(&self, reference: String, state: PostState) -> StoreFuture<'_, bool>;
    /// Forget a post whose stripping was confirmed.
    fn delete_post(&self, reference: String) -> StoreFuture<'_, bool>;

    /// The newest `limit` character results and the newest `limit` party
    /// totals kept at `now` ([`super::estimate_kept`]), each newest first.
    fn estimates(&self, now: DateTime<Utc>, limit: u32) -> StoreFuture<'_, StoredEstimates>;
    /// Insert or replace the character's result.
    /// [`StoreError::Constraint`] for a row that fails [`MemberEstimate::check`].
    fn put_member_estimate(&self, estimate: MemberEstimate) -> StoreFuture<'_, ()>;
    /// Insert or replace the total for the estimate's IGN set, with the
    /// characters it includes.
    /// [`StoreError::Constraint`] for a row that fails [`PartyEstimate::check`].
    fn put_party_estimate(&self, estimate: PartyEstimate) -> StoreFuture<'_, ()>;
    /// Delete the character's result and every party total including them,
    /// in one write; what went.
    fn delete_estimates(&self, character_id: String) -> StoreFuture<'_, DeletedEstimates>;
    /// Delete results and totals no longer kept at `now`
    /// ([`super::estimate_kept`]); how many.
    fn sweep_estimates(&self, now: DateTime<Utc>) -> StoreFuture<'_, u64>;
}
