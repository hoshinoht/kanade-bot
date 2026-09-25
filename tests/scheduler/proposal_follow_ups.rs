//! A proposal's post-merge follow-ups (new-timing materialisation, sibling
//! retirement) that failed are re-run by a repeated ✅.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, FixedOffset, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::bot::delivery::StoreRef;
use kanade::domain::drafts::{
    DraftCreated, DraftEvent, DraftStatus, DraftStore, DraftUpdate, DraftWrite, LoadedDraft,
    MergeCommit, NewDraft, NewProposal, ProposalCreated, ProposalInfo, ProposalSource,
    ProposalStore, StoredDraft, StoredProposal,
};
use kanade::domain::history::{Actor, ChangeMeta, ChangeRecord, ChangeRef};
use kanade::domain::ids::RandomIds;
use kanade::domain::members::{Directory, Member};
use kanade::domain::proposals::{Approver, ChangeKind, Payload, ProposedChange};
use kanade::domain::schedule::{ChangeSet, ReminderPolicy, SchedulePolicy, ScheduleSnapshot};
use kanade::domain::scheduler::{
    Committed, DraftError, ProposalError, ProposalRequest, RecordedRequest, ScheduleStore,
    SchedulerService, Scope, StoreError, Supersede,
};
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::common::TestClock;

/// The memory store, with materialise commits and proposal listing
/// failing while `broken` is set (a crash between merge and follow-ups).
#[derive(Default)]
struct Flaky {
    inner: MemoryScheduleStore,
    broken: AtomicBool,
}

impl Flaky {
    fn broken(&self) -> bool {
        self.broken.load(Ordering::SeqCst)
    }
}

fn simulated() -> StoreError {
    StoreError::Backend("simulated follow-up failure".into())
}

impl ScheduleStore for Flaky {
    async fn load(&self, scope: &Scope) -> Result<ScheduleSnapshot, StoreError> {
        self.inner.load(scope).await
    }

    async fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> Result<Option<RecordedRequest>, StoreError> {
        self.inner.recorded_request(actor, request_id).await
    }

    async fn commit(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
    ) -> Result<Option<Committed>, StoreError> {
        let materialising = meta
            .origin
            .request_id
            .as_deref()
            .is_some_and(|id| id.ends_with(":materialise"));
        if self.broken() && materialising {
            return Err(simulated());
        }
        self.inner.commit(expected_revision, changes, meta).await
    }
}

impl DraftStore for Flaky {
    async fn snapshot_with_head(&self) -> Result<(ScheduleSnapshot, ChangeRef), StoreError> {
        self.inner.snapshot_with_head().await
    }

    async fn records_after(&self, base: &ChangeRef) -> Result<Vec<ChangeRecord>, StoreError> {
        self.inner.records_after(base).await
    }

    async fn create_draft(&self, new: NewDraft) -> Result<DraftCreated, StoreError> {
        self.inner.create_draft(new).await
    }

    async fn load_draft(&self, id: &str) -> Result<Option<LoadedDraft>, StoreError> {
        self.inner.load_draft(id).await
    }

    async fn recorded_draft_request(
        &self,
        author: &Actor,
        request_id: &str,
    ) -> Result<Option<(String, StoredDraft)>, StoreError> {
        self.inner.recorded_draft_request(author, request_id).await
    }

    async fn list_drafts(
        &self,
        status: Option<DraftStatus>,
    ) -> Result<Vec<StoredDraft>, StoreError> {
        self.inner.list_drafts(status).await
    }

    async fn draft_events(&self, id: &str) -> Result<Vec<DraftEvent>, StoreError> {
        self.inner.draft_events(id).await
    }

    async fn update_draft(&self, update: DraftUpdate) -> Result<DraftWrite, StoreError> {
        self.inner.update_draft(update).await
    }

    async fn commit_merge(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
        draft_id: &str,
        expected_version: u64,
        note: Option<String>,
    ) -> Result<MergeCommit, StoreError> {
        self.inner
            .commit_merge(
                expected_revision,
                changes,
                meta,
                draft_id,
                expected_version,
                note,
            )
            .await
    }

    async fn expire_drafts(
        &self,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
        actor: &Actor,
    ) -> Result<Vec<String>, StoreError> {
        self.inner.expire_drafts(week, at, actor).await
    }
}

impl ProposalStore for Flaky {
    async fn create_proposal(&self, new: NewProposal) -> Result<ProposalCreated, StoreError> {
        self.inner.create_proposal(new).await
    }

    async fn load_proposal(
        &self,
        id: &str,
    ) -> Result<Option<(LoadedDraft, ProposalInfo)>, StoreError> {
        self.inner.load_proposal(id).await
    }

    async fn list_proposals(&self, live_only: bool) -> Result<Vec<StoredProposal>, StoreError> {
        if self.broken() {
            return Err(simulated());
        }
        self.inner.list_proposals(live_only).await
    }

    async fn expire_proposals(
        &self,
        now: DateTime<Utc>,
        actor: &Actor,
    ) -> Result<Vec<String>, StoreError> {
        self.inner.expire_proposals(now, actor).await
    }
}

struct Guild;

impl Directory for Guild {
    fn member(&self, user_id: &str) -> Option<Member> {
        Some(Member {
            user_id: user_id.to_owned(),
            has_role: true,
            ..Member::default()
        })
    }

    fn is_watched(&self, _channel_id: &str) -> bool {
        true
    }
}

fn kl(day: u32, hour: u32) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 8, day, hour, 0, 0)
        .unwrap()
}

fn weekly(time: u32) -> ProposedChange {
    ProposedChange {
        channel_id: Some("222".into()),
        bosses: vec!["HKalos".into()],
        participants: vec!["1001".into()],
        payload: Payload::Fix {
            weekday: Some(Weekday::Tue),
            time: NaiveTime::from_hms_opt(time, 0, 0),
        },
        ..ProposedChange::new(ChangeKind::Fix)
    }
}

#[tokio::test]
async fn a_repeated_approval_finishes_follow_ups_a_crash_left_undone() {
    let policy = SchedulePolicy::new(
        ReminderPolicy {
            zone: Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    );
    let store = Flaky::default();
    let mut service = SchedulerService::new(StoreRef(&store), RandomIds, TestClock::new(kl(27, 1)));
    let mut ids = Vec::new();
    for time in [21, 22] {
        let proposed = service
            .propose(
                ProposalRequest {
                    change: weekly(time),
                    source: ProposalSource::Chat,
                    source_id: format!("chat-{time}"),
                    supersede: Supersede::Keep,
                },
                &policy,
                &Guild,
            )
            .await
            .unwrap();
        ids.push(proposed.proposal.id);
    }
    let approver = Approver {
        user_id: "1001".into(),
        has_role: true,
        is_admin: false,
    };
    store.broken.store(true, Ordering::SeqCst);
    let approved = service
        .approve_proposal(&ids[0], &approver, &policy, &Guild)
        .await
        .unwrap();
    assert_eq!(approved.follow_up_errors.len(), 2, "{approved:?}");
    let timing = approved.fixed_run_id.clone().expect("new timing");
    let runs_of = async |store: &Flaky| {
        store
            .load(&Scope::All)
            .await
            .unwrap()
            .runs
            .iter()
            .filter(|run| run.fixed_run_id.as_deref() == Some(timing.as_str()))
            .count()
    };
    assert_eq!(runs_of(&store).await, 0);
    let sibling = async |store: &Flaky| {
        store
            .load_proposal(&ids[1])
            .await
            .unwrap()
            .unwrap()
            .0
            .draft
            .status
    };
    assert_eq!(sibling(&store).await, DraftStatus::Submitted);

    store.broken.store(false, Ordering::SeqCst);
    assert!(matches!(
        service
            .approve_proposal(&ids[0], &approver, &policy, &Guild)
            .await
            .unwrap_err(),
        ProposalError::Draft(DraftError::AlreadyApplied { seq, .. }) if seq == approved.merge.seq
    ));
    assert_eq!(sibling(&store).await, DraftStatus::Discarded);
    assert!(
        runs_of(&store).await > 0,
        "the new timing's weeks are materialised"
    );
}
