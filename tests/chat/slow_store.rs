//! A proposal store that commits a proposal and then stalls before
//! returning, so a question's deadline falls between the committed staging
//! and the supersede that must follow it.

use std::future::Future;
use std::time::Duration;

use chrono::{DateTime, Utc};
use kanade::domain::drafts::{
    DraftCreated, DraftEvent, DraftStatus, DraftStore, DraftUpdate, DraftWrite, LoadedDraft,
    MergeCommit, NewDraft, NewProposal, ProposalCreated, ProposalInfo, ProposalStore, StoredDraft,
    StoredProposal,
};
use kanade::domain::history::{Actor, ChangeMeta, ChangeRecord, ChangeRef};
use kanade::domain::schedule::{ChangeSet, ScheduleSnapshot};
use kanade::domain::scheduler::{Committed, RecordedRequest, ScheduleStore, Scope, StoreError};
use kanade::infrastructure::store::MemoryScheduleStore;

pub struct SlowStore<'a> {
    pub inner: &'a MemoryScheduleStore,
    /// How long `create_proposal` stalls after its commit.
    pub stall: Duration,
}

impl ScheduleStore for SlowStore<'_> {
    fn load(
        &self,
        scope: &Scope,
    ) -> impl Future<Output = Result<ScheduleSnapshot, StoreError>> + Send {
        self.inner.load(scope)
    }

    fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> impl Future<Output = Result<Option<RecordedRequest>, StoreError>> + Send {
        self.inner.recorded_request(actor, request_id)
    }

    fn commit(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
    ) -> impl Future<Output = Result<Option<Committed>, StoreError>> + Send {
        self.inner.commit(expected_revision, changes, meta)
    }
}

impl DraftStore for SlowStore<'_> {
    fn snapshot_with_head(
        &self,
    ) -> impl Future<Output = Result<(ScheduleSnapshot, ChangeRef), StoreError>> + Send {
        self.inner.snapshot_with_head()
    }

    fn records_after(
        &self,
        base: &ChangeRef,
    ) -> impl Future<Output = Result<Vec<ChangeRecord>, StoreError>> + Send {
        self.inner.records_after(base)
    }

    fn create_draft(
        &self,
        new: NewDraft,
    ) -> impl Future<Output = Result<DraftCreated, StoreError>> + Send {
        self.inner.create_draft(new)
    }

    fn load_draft(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<LoadedDraft>, StoreError>> + Send {
        self.inner.load_draft(id)
    }

    fn recorded_draft_request(
        &self,
        author: &Actor,
        request_id: &str,
    ) -> impl Future<Output = Result<Option<(String, StoredDraft)>, StoreError>> + Send {
        self.inner.recorded_draft_request(author, request_id)
    }

    fn list_drafts(
        &self,
        status: Option<DraftStatus>,
    ) -> impl Future<Output = Result<Vec<StoredDraft>, StoreError>> + Send {
        self.inner.list_drafts(status)
    }

    fn draft_events(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Vec<DraftEvent>, StoreError>> + Send {
        self.inner.draft_events(id)
    }

    fn update_draft(
        &self,
        update: DraftUpdate,
    ) -> impl Future<Output = Result<DraftWrite, StoreError>> + Send {
        self.inner.update_draft(update)
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
        self.inner.commit_merge(
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
        self.inner.expire_drafts(week, at, actor)
    }
}

impl ProposalStore for SlowStore<'_> {
    async fn create_proposal(&self, new: NewProposal) -> Result<ProposalCreated, StoreError> {
        let created = self.inner.create_proposal(new).await;
        tokio::time::sleep(self.stall).await;
        created
    }

    fn load_proposal(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<(LoadedDraft, ProposalInfo)>, StoreError>> + Send {
        self.inner.load_proposal(id)
    }

    fn list_proposals(
        &self,
        live_only: bool,
    ) -> impl Future<Output = Result<Vec<StoredProposal>, StoreError>> + Send {
        self.inner.list_proposals(live_only)
    }

    fn expire_proposals(
        &self,
        now: DateTime<Utc>,
        actor: &Actor,
    ) -> impl Future<Output = Result<Vec<String>, StoreError>> + Send {
        self.inner.expire_proposals(now, actor)
    }
}
