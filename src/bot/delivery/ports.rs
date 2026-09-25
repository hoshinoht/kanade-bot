//! Adapters that let one tick share its store, id source and clock reading
//! with a short-lived [`SchedulerService`](crate::domain::scheduler::SchedulerService).

use std::future::Future;

use chrono::{DateTime, Utc};

use twilight_model::id::{Id, marker::MessageMarker};

use crate::bot::events::{CardIndex, LookupError};
use crate::domain::drafts::{
    DraftStatus, DraftStore, DraftUpdate, DraftWrite, LoadedDraft, MergeCommit, NewDraft,
    NewProposal, ProposalCreated, ProposalInfo, ProposalStore, StoredDraft, StoredProposal,
};
use crate::domain::history::{Actor, ChangeMeta, ChangeRecord, ChangeRef};
use crate::domain::schedule::{ChangeSet, ScheduleSnapshot};
use crate::domain::scheduler::{
    Clock, Committed, IdSource, RecordedRequest, ScheduleStore, Scope, StoreError,
};

/// A borrowed store: the scheduler service writes through it while the
/// journal is used directly on the same store.
#[derive(Debug)]
pub struct StoreRef<'a, S>(pub &'a S);

impl<S: ScheduleStore + Sync> ScheduleStore for StoreRef<'_, S> {
    fn load(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<ScheduleSnapshot, StoreError>> + Send {
        self.0.load(scope)
    }

    fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> impl Future<Output = Result<Option<RecordedRequest>, StoreError>> + Send {
        self.0.recorded_request(actor, request_id)
    }

    fn commit(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
    ) -> impl Future<Output = Result<Option<Committed>, StoreError>> + Send {
        self.0.commit(expected_revision, changes, meta)
    }
}

impl<S: CardIndex> CardIndex for StoreRef<'_, S> {
    fn runs_for_message(
        &self,
        message: Id<MessageMarker>,
    ) -> impl Future<Output = Result<Vec<String>, LookupError>> + Send {
        self.0.runs_for_message(message)
    }
}

/// A borrowed id source.
#[derive(Debug)]
pub struct IdsRef<'a, I>(pub &'a mut I);

impl<I: IdSource> IdSource for IdsRef<'_, I> {
    fn new_id(&mut self) -> String {
        self.0.new_id()
    }
}

/// The one clock reading a tick uses throughout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedClock(pub DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

impl<S: DraftStore + Sync> DraftStore for StoreRef<'_, S> {
    fn snapshot_with_head(
        &self,
    ) -> impl Future<Output = Result<(ScheduleSnapshot, ChangeRef), StoreError>> + Send {
        self.0.snapshot_with_head()
    }

    fn records_after(
        &self,
        base: &ChangeRef,
    ) -> impl Future<Output = Result<Vec<ChangeRecord>, StoreError>> + Send {
        self.0.records_after(base)
    }

    fn create_draft(
        &self,
        new: NewDraft,
    ) -> impl Future<Output = Result<crate::domain::drafts::DraftCreated, StoreError>> + Send {
        self.0.create_draft(new)
    }

    fn load_draft(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<LoadedDraft>, StoreError>> + Send {
        self.0.load_draft(id)
    }

    fn list_drafts(
        &self,
        status: Option<DraftStatus>,
    ) -> impl Future<Output = Result<Vec<StoredDraft>, StoreError>> + Send {
        self.0.list_drafts(status)
    }

    fn recorded_draft_request(
        &self,
        author: &Actor,
        request_id: &str,
    ) -> impl Future<Output = Result<Option<(String, StoredDraft)>, StoreError>> + Send {
        self.0.recorded_draft_request(author, request_id)
    }

    fn draft_events(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Vec<crate::domain::drafts::DraftEvent>, StoreError>> + Send
    {
        self.0.draft_events(id)
    }

    fn update_draft(
        &self,
        update: DraftUpdate,
    ) -> impl Future<Output = Result<DraftWrite, StoreError>> + Send {
        self.0.update_draft(update)
    }

    fn commit_merge(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
        draft_id: &str,
        expected_version: u64,
        note: Option<String>,
    ) -> impl Future<Output = Result<MergeCommit, StoreError>> + Send {
        self.0.commit_merge(
            expected_revision,
            changes,
            meta,
            draft_id,
            expected_version,
            note,
        )
    }

    fn expire_drafts(
        &self,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
        actor: &Actor,
    ) -> impl Future<Output = Result<Vec<String>, StoreError>> + Send {
        self.0.expire_drafts(week, at, actor)
    }
}

impl<S: ProposalStore + Sync> ProposalStore for StoreRef<'_, S> {
    fn create_proposal(
        &self,
        new: NewProposal,
    ) -> impl Future<Output = Result<ProposalCreated, StoreError>> + Send {
        self.0.create_proposal(new)
    }

    fn load_proposal(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<(LoadedDraft, ProposalInfo)>, StoreError>> + Send {
        self.0.load_proposal(id)
    }

    fn list_proposals(
        &self,
        live_only: bool,
    ) -> impl Future<Output = Result<Vec<StoredProposal>, StoreError>> + Send {
        self.0.list_proposals(live_only)
    }

    fn expire_proposals(
        &self,
        now: DateTime<Utc>,
        actor: &Actor,
    ) -> impl Future<Output = Result<Vec<String>, StoreError>> + Send {
        self.0.expire_proposals(now, actor)
    }
}
