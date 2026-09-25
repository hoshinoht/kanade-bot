//! The proposal storage every store must keep: proposals are drafts of kind
//! `proposal` with their source, supersede key and TTL deadline, superseded
//! and expired as the system, and merged only through `commit_merge`. Each
//! check runs against a fresh store; failures panic with the check name.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use crate::domain::drafts::{
    DEFAULT_PROPOSAL_TTL, DraftEventKind, DraftKind, DraftStatus, MergeCommit, NewDraft,
    NewProposal, ProposalCreated, ProposalSource, ProposalStore, SUPERSEDED, StagedOp, Target,
};
use crate::domain::history::{Actor, ChangeHistory, ChangeMeta, ChangeRef, Origin, Surface};
use crate::domain::schedule::{Change, ChangeSet, Run, RunSource, RunStatus};
use crate::domain::scheduler::{ScheduleStore, Scope, StoreError};

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<S: ScheduleStore + ChangeHistory + ProposalStore + Sync>(
    make: impl AsyncFn() -> S,
) {
    create_load_and_list(make().await).await;
    creation_refusals(make().await).await;
    same_source_ids_replay(make().await).await;
    newer_proposals_supersede_live_ones_with_their_key(make().await).await;
    ttl_expiry_closes_due_proposals_only(make().await).await;
    proposals_merge_through_commit_merge(make().await).await;
}

fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0)
        .single()
        .expect("valid instant")
}

fn extractor() -> Actor {
    Actor::system("extraction")
}

async fn base<S: ScheduleStore + ChangeHistory>(store: &S) -> (ChangeRef, u64) {
    let head = store.history_head().await.expect("head");
    let revision = store.load(&Scope::All).await.expect("load").revision;
    (head, revision)
}

fn proposal(id: &str, base: &(ChangeRef, u64), key: Option<&str>) -> NewProposal {
    NewProposal {
        id: id.into(),
        title: "Lotus to Friday".into(),
        author: extractor(),
        base: base.0.clone(),
        base_revision: base.1,
        subject: Some("run:run-1".into()),
        at: utc(20, 12, 0),
        ops: vec![StagedOp {
            ord: 0,
            op: crate::domain::drafts::DraftOp::ResetToFixed {
                run: Target::Existing("run-1".into()),
            },
            author: extractor(),
            added_at: utc(20, 12, 0),
        }],
        expires_week: Some(utc(17, 0, 0)),
        source: ProposalSource::Extraction,
        source_id: format!("x-{id}"),
        supersede_key: key.map(str::to_owned),
        ttl: DEFAULT_PROPOSAL_TTL,
    }
}

async fn created<S: ProposalStore>(store: &S, new: NewProposal) -> Vec<String> {
    match store.create_proposal(new).await.expect("create") {
        ProposalCreated::Created { superseded, .. } => superseded,
        other => panic!("proposal not created: {other:?}"),
    }
}

async fn create_load_and_list<S: ScheduleStore + ChangeHistory + ProposalStore>(store: S) {
    let base = base(&store).await;
    let new = proposal("p-1", &base, None);
    let superseded = created(&store, new.clone()).await;
    assert!(superseded.is_empty());
    let (loaded, info) = store
        .load_proposal("p-1")
        .await
        .expect("load")
        .expect("present");
    assert_eq!(loaded.draft.kind, DraftKind::Proposal, "create: kind");
    assert_eq!(loaded.draft.status, DraftStatus::Submitted);
    assert_eq!(loaded.draft.version, 1);
    assert_eq!(loaded.draft.author, extractor());
    assert_eq!(loaded.draft.subject.as_deref(), Some("run:run-1"));
    assert_eq!(loaded.draft.scope.expires_week(), Some(utc(17, 0, 0)));
    assert_eq!(
        loaded.draft_ops(),
        new.ops
            .iter()
            .map(|staged| staged.op.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(info.source, ProposalSource::Extraction);
    assert_eq!(info.source_id, "x-p-1");
    assert_eq!(
        info.expires_at,
        utc(21, 12, 0),
        "create: the deadline is created_at + TTL"
    );
    let plain = store
        .load_draft("p-1")
        .await
        .expect("load draft")
        .expect("present");
    assert_eq!(plain.draft.kind, DraftKind::Proposal, "drafts see the kind");
    let events = store.draft_events("p-1").await.expect("events");
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [DraftEventKind::Created, DraftEventKind::Submitted]
    );
    let listed = store.list_proposals(false).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].info, info);
    let drafts = store.list_drafts(None).await.expect("drafts");
    assert_eq!(
        drafts.iter().map(|draft| draft.kind).collect::<Vec<_>>(),
        [DraftKind::Proposal]
    );
    assert_eq!(store.load_proposal("absent").await.expect("load"), None);
}

async fn creation_refusals<S: ScheduleStore + ChangeHistory + ProposalStore>(store: S) {
    let base = base(&store).await;
    let mut member = proposal("p-1", &base, None);
    member.author = Actor::member("1");
    assert!(
        matches!(
            store.create_proposal(member).await,
            Err(StoreError::Constraint(_))
        ),
        "refusals: a proposal's author is a system component"
    );
    let mut zero = proposal("p-1", &base, None);
    zero.ttl = TimeDelta::zero();
    assert!(
        matches!(
            store.create_proposal(zero).await,
            Err(StoreError::Constraint(_))
        ),
        "refusals: the TTL is positive"
    );
    let plain = NewDraft {
        id: "p-2".into(),
        kind: DraftKind::Proposal,
        title: "sneaky".into(),
        author: extractor(),
        base: base.0.clone(),
        base_revision: base.1,
        request_type: None,
        subject: None,
        at: utc(20, 12, 0),
        request: None,
        submit: None,
    };
    assert!(
        matches!(
            store.create_draft(plain).await,
            Err(StoreError::Constraint(_))
        ),
        "refusals: create_draft never makes a proposal"
    );
    assert!(store.list_drafts(None).await.expect("drafts").is_empty());
}

async fn same_source_ids_replay<S: ScheduleStore + ChangeHistory + ProposalStore>(store: S) {
    let base = base(&store).await;
    created(&store, proposal("p-1", &base, Some("run-1"))).await;
    let ProposalCreated::Replayed(replayed) = store
        .create_proposal(proposal("p-1", &base, Some("run-1")))
        .await
        .expect("replay")
    else {
        panic!("replay: not replayed");
    };
    assert_eq!(replayed.id, "p-1");
    let mut other = proposal("p-1", &base, None);
    other.source = ProposalSource::Chat;
    assert!(
        matches!(
            store.create_proposal(other).await,
            Err(StoreError::Constraint(_))
        ),
        "replay: another source's id is refused"
    );
    let admin = NewDraft {
        id: "d-1".into(),
        kind: DraftKind::Admin,
        title: "admin".into(),
        author: Actor::admin("root"),
        base: base.0.clone(),
        base_revision: base.1,
        request_type: None,
        subject: None,
        at: utc(20, 12, 0),
        request: None,
        submit: None,
    };
    store.create_draft(admin).await.expect("admin draft");
    assert!(
        matches!(
            store.create_proposal(proposal("d-1", &base, None)).await,
            Err(StoreError::Constraint(_))
        ),
        "replay: an admin draft's id is refused"
    );
    assert_eq!(
        store.list_proposals(false).await.expect("list").len(),
        1,
        "replay: nothing new written"
    );
}

async fn newer_proposals_supersede_live_ones_with_their_key<
    S: ScheduleStore + ChangeHistory + ProposalStore,
>(
    store: S,
) {
    let base = base(&store).await;
    created(&store, proposal("p-1", &base, Some("run-1"))).await;
    created(&store, proposal("p-2", &base, Some("run-2"))).await;
    created(&store, proposal("p-3", &base, None)).await;
    let mut newer = proposal("p-4", &base, Some("run-1"));
    newer.at = utc(20, 13, 0);
    assert_eq!(created(&store, newer).await, ["p-1"], "supersede: same key");
    let (old, _) = store
        .load_proposal("p-1")
        .await
        .expect("load")
        .expect("present");
    assert_eq!(old.draft.status, DraftStatus::Discarded);
    assert_eq!(old.draft.close_reason.as_deref(), Some(SUPERSEDED));
    assert_eq!(
        old.draft.closed_by,
        Some(extractor()),
        "supersede: closed by the system author"
    );
    assert_eq!(old.draft.updated_at, utc(20, 13, 0));
    assert_eq!(old.draft.version, 1, "supersede: no version bump");
    let events = store.draft_events("p-1").await.expect("events");
    let last = events.last().expect("event");
    assert_eq!(last.kind, DraftEventKind::Discarded);
    assert_eq!(last.detail.as_deref(), Some(SUPERSEDED));
    let mut newest = proposal("p-5", &base, Some("run-1"));
    newest.at = utc(20, 14, 0);
    assert_eq!(
        created(&store, newest).await,
        ["p-4"],
        "supersede: closed proposals are left alone"
    );
    let live = store.list_proposals(true).await.expect("live");
    assert_eq!(
        live.iter()
            .map(|proposal| proposal.draft.id.as_str())
            .collect::<Vec<_>>(),
        ["p-2", "p-3", "p-5"]
    );
}

async fn ttl_expiry_closes_due_proposals_only<S: ScheduleStore + ChangeHistory + ProposalStore>(
    store: S,
) {
    let base = base(&store).await;
    created(&store, proposal("p-1", &base, None)).await;
    let mut short = proposal("p-2", &base, None);
    short.ttl = TimeDelta::hours(2);
    created(&store, short).await;
    let delivery = Actor::system("delivery");
    assert!(
        store
            .expire_proposals(utc(20, 13, 59), &delivery)
            .await
            .expect("early")
            .is_empty()
    );
    assert_eq!(
        store
            .expire_proposals(utc(20, 14, 0), &delivery)
            .await
            .expect("due"),
        ["p-2"],
        "ttl: due at the deadline"
    );
    let (expired, _) = store
        .load_proposal("p-2")
        .await
        .expect("load")
        .expect("present");
    assert_eq!(expired.draft.status, DraftStatus::Expired);
    assert_eq!(expired.draft.closed_by, Some(delivery.clone()));
    assert_eq!(expired.draft.version, 1, "ttl: no version bump");
    assert_eq!(
        store
            .expire_proposals(utc(21, 12, 0), &delivery)
            .await
            .expect("later"),
        ["p-1"],
        "ttl: expired proposals are not expired again"
    );
    assert!(store.list_proposals(true).await.expect("live").is_empty());
}

async fn proposals_merge_through_commit_merge<S: ScheduleStore + ChangeHistory + ProposalStore>(
    store: S,
) {
    let base = base(&store).await;
    created(&store, proposal("p-1", &base, None)).await;
    let week = utc(14, 16, 0);
    let run = Run {
        id: "run-9".into(),
        fixed_run_id: None,
        channel_id: Some("900".into()),
        week_start: week,
        datetime: utc(18, 20, 0),
        bosses: vec!["HFA".into()],
        participants: vec!["1".into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    };
    let meta = ChangeMeta {
        origin: Origin::new(Actor::admin("root"), Surface::ExtractionApproval)
            .with_request_id("merge:p-1@v1"),
        at: utc(20, 15, 0),
        notices: Vec::new(),
        refs: vec![base.0.clone()],
        request_digest: Some("digest".into()),
        expect: Default::default(),
    };
    let MergeCommit::Committed(committed) = store
        .commit_merge(
            base.1,
            ChangeSet {
                changes: vec![Change::PutRun(run)],
            },
            meta,
            "p-1",
            1,
            None,
        )
        .await
        .expect("merge")
    else {
        panic!("merge: not committed");
    };
    let (merged, _) = store
        .load_proposal("p-1")
        .await
        .expect("load")
        .expect("present");
    assert_eq!(merged.draft.status, DraftStatus::Merged);
    assert_eq!(merged.draft.merged_seq, Some(committed.seq));
    assert_eq!(merged.draft.kind, DraftKind::Proposal);
    let record = store
        .load_change(committed.seq)
        .await
        .expect("record")
        .expect("record");
    assert_eq!(record.origin.surface, Surface::ExtractionApproval);
    assert!(
        store
            .expire_proposals(utc(30, 0, 0), &Actor::system("delivery"))
            .await
            .expect("expire")
            .is_empty(),
        "merge: a merged proposal never expires"
    );
}
