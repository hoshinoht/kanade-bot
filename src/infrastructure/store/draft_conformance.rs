//! The draft storage every store must keep, driven through
//! [`SchedulerService`] against a fresh store per check. Failures panic with
//! the check name.

use chrono::{DateTime, Utc, Weekday};

use super::fixtures::{Ids, TestClock, admin, refused, timing, utc2026};
use crate::domain::drafts::{
    DraftChange, DraftCreated, DraftEvent, DraftEventKind, DraftScope, DraftStale, DraftStatus,
    DraftStore, DraftUpdate, DraftWrite, LoadedDraft, MergeCommit, NewDraft, StoredDraft,
    closed_at,
};
use crate::domain::history::{Actor, ChangeHistory, ChangeMeta, ChangeRef, Origin, Surface};
use crate::domain::schedule::{
    Change, ChangeSet, NewFixedRun, Run, RunSource, RunStatus, ScheduleSnapshot,
};
use crate::domain::scheduler::{ScheduleStore, SchedulerService, Scope, StoreError};

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<S: ScheduleStore + ChangeHistory + DraftStore + Sync>(
    make: impl AsyncFn() -> S,
) {
    create_load_list_and_log(make().await).await;
    create_request_ids_replay_or_mismatch(make().await).await;
    op_edits_bump_the_version_and_refuse_stale_writes(make().await).await;
    rebase_moves_the_base_and_close_is_final(make().await).await;
    commit_merge_writes_rows_record_and_close(make().await).await;
    commit_merge_replays_request_ids(make().await).await;
    commit_merge_refuses_empty_stale_and_conflicted(make().await).await;
    expiry_closes_only_past_weeks(make().await).await;
    records_after_pages_the_chain(make().await).await;
    snapshot_with_head_covers_the_snapshot(make().await).await;
    request_limits_are_counted_inside_the_insert(make().await).await;
    concurrent_submissions_stay_at_the_cap(make().await).await;
    commit_merge_records_its_note(make().await).await;
}

fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    utc2026(8, day, hour, minute)
}

type Service<S> = SchedulerService<S, Ids, TestClock>;

async fn service<S: ScheduleStore>(store: S) -> Service<S> {
    SchedulerService::new(store, Ids::default(), TestClock::at(utc(27, 1, 0)))
}

/// A Saturday 20:00 timing with no channel and member 1 alone.
fn solo_timing() -> NewFixedRun {
    NewFixedRun {
        channel_id: None,
        participants: vec!["1".into()],
        ..timing(Weekday::Sat)
    }
}

async fn head<S: ScheduleStore + ChangeHistory>(service: &Service<S>) -> ChangeRef {
    service.store().history_head().await.expect("head")
}

async fn snapshot<S: ScheduleStore>(service: &Service<S>) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.expect("load")
}

/// A fresh service with the history head and revision drafts start from.
async fn started<S: ScheduleStore + ChangeHistory>(store: S) -> (Service<S>, ChangeRef, u64) {
    let service = service(store).await;
    let base = head(&service).await;
    let revision = snapshot(&service).await.revision;
    (service, base, revision)
}

async fn loaded<S: ScheduleStore + DraftStore>(service: &Service<S>, id: &str) -> LoadedDraft {
    let loaded = service.store().load_draft(id).await.expect("load");
    loaded.expect("present")
}

async fn events<S: ScheduleStore + DraftStore>(service: &Service<S>, id: &str) -> Vec<DraftEvent> {
    service.store().draft_events(id).await.expect("events")
}

async fn kinds<S: ScheduleStore + DraftStore>(
    service: &Service<S>,
    id: &str,
) -> Vec<DraftEventKind> {
    events(service, id)
        .await
        .iter()
        .map(|event| event.kind)
        .collect()
}

/// Admin `root`'s `change` to `id`, expected at `version`.
fn update(id: &str, version: u64, at: DateTime<Utc>, change: DraftChange) -> DraftUpdate {
    DraftUpdate {
        draft_id: id.into(),
        expected_version: version,
        actor: Actor::admin("root"),
        at,
        change,
    }
}

fn close(status: DraftStatus, reason: Option<&str>) -> DraftChange {
    DraftChange::Close {
        status,
        reason: reason.map(Into::into),
        notices: Vec::new(),
    }
}

async fn write<S: ScheduleStore + DraftStore>(
    service: &Service<S>,
    update: DraftUpdate,
) -> DraftWrite {
    service.store().update_draft(update).await.expect("update")
}

/// `update`, which the store must write.
async fn written<S: ScheduleStore + DraftStore>(
    service: &Service<S>,
    update: DraftUpdate,
    check: &str,
) -> StoredDraft {
    match write(service, update).await {
        DraftWrite::Written(draft) => draft,
        other => panic!("{check}: not written: {other:?}"),
    }
}

/// One run in boss week 26 Aug: the merge every check commits.
fn run_9() -> ChangeSet {
    ChangeSet {
        changes: vec![Change::PutRun(run("run-9", utc(26, 16, 0), utc(29, 20, 0)))],
    }
}

/// Merge `changes` into `draft` at `version` under `meta`, with no note.
async fn merge<S: ScheduleStore + DraftStore>(
    service: &Service<S>,
    revision: u64,
    changes: ChangeSet,
    meta: ChangeMeta,
    draft: &str,
    version: u64,
) -> Result<MergeCommit, StoreError> {
    let store = service.store();
    store
        .commit_merge(revision, changes, meta, draft, version, None)
        .await
}

/// [`merge`] of [`run_9`] at version 1, which the store must commit.
async fn committed<S: ScheduleStore + DraftStore>(
    service: &Service<S>,
    revision: u64,
    meta: ChangeMeta,
    draft: &str,
    check: &str,
) -> crate::domain::scheduler::Committed {
    match merge(service, revision, run_9(), meta, draft, 1).await {
        Ok(MergeCommit::Committed(committed)) => committed,
        other => panic!("{check}: not committed: {other:?}"),
    }
}

/// Create `new`, which the store must take as a new draft.
async fn create<S: ScheduleStore + DraftStore>(
    service: &Service<S>,
    new: NewDraft,
    check: &str,
) -> StoredDraft {
    match service.store().create_draft(new).await.expect("create") {
        DraftCreated::Created(created) => created,
        other => panic!("{check}: not created: {other:?}"),
    }
}

fn draft(base: &ChangeRef, revision: u64) -> NewDraft {
    NewDraft {
        id: "draft-1".into(),
        kind: crate::domain::drafts::DraftKind::Admin,
        title: "retime the raid".into(),
        author: Actor::admin("root"),
        base: base.clone(),
        base_revision: revision,
        request_type: None,
        subject: None,
        at: utc(27, 1, 0),
        request: None,
        submit: None,
    }
}

fn run(id: &str, week: DateTime<Utc>, at: DateTime<Utc>) -> Run {
    Run {
        id: id.into(),
        fixed_run_id: None,
        channel_id: Some("900".into()),
        week_start: week,
        datetime: at,
        bosses: vec!["HFA".into()],
        participants: vec!["1".into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

fn merge_meta(request_id: &str, digest: &str) -> ChangeMeta {
    ChangeMeta {
        origin: Origin::new(Actor::admin("root"), Surface::DraftMerge).with_request_id(request_id),
        at: utc(27, 2, 0),
        notices: vec!["notice.draft.merged".into()],
        refs: Vec::new(),
        request_digest: Some(digest.into()),
        expect: Default::default(),
        outbox: Vec::new(),
    }
}

async fn create_load_list_and_log<S: ScheduleStore + ChangeHistory + DraftStore>(store: S) {
    let (service, base, revision) = started(store).await;
    let created = create(&service, draft(&base, revision), "create_load_list_and_log").await;
    assert_eq!(created.version, 1);
    assert_eq!(created.status, DraftStatus::Open);
    assert_eq!(created.base, base);
    let loaded = loaded(&service, "draft-1").await;
    assert_eq!(loaded.draft, created);
    assert!(loaded.ops.is_empty());
    let missing = service.store().load_draft("missing").await.expect("load");
    assert!(missing.is_none());
    let list = async |status| service.store().list_drafts(status).await.expect("list");
    let all = list(None).await;
    assert_eq!(all.len(), 1);
    assert_eq!(all[0], created);
    let open = list(Some(DraftStatus::Open)).await;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0], created, "the draft lists under its status");
    assert!(list(Some(DraftStatus::Merged)).await.is_empty());
    let mut second = draft(&base, revision);
    second.id = "draft-2".into();
    create(&service, second, "create_load_list_and_log: second draft").await;
    let ids: Vec<String> = list(None).await.iter().map(|d| d.id.clone()).collect();
    assert_eq!(ids, ["draft-1", "draft-2"], "drafts list in creation order");
    let events = events(&service, "draft-1").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, DraftEventKind::Created);
    assert_eq!(events[0].version, 1);
    assert_eq!(events[0].actor, Actor::admin("root"));
    // The draft writes touched neither the schedule nor its history.
    assert_eq!(head(&service).await, base);
    assert_eq!(snapshot(&service).await.revision, revision);
}

async fn create_request_ids_replay_or_mismatch<S: ScheduleStore + ChangeHistory + DraftStore>(
    store: S,
) {
    let (service, base, revision) = started(store).await;
    let request = |digest: &str| {
        let mut new = draft(&base, revision);
        new.id = "draft-r".into();
        new.request = Some(crate::domain::drafts::DraftRequest {
            request_id: "create-1".into(),
            digest: digest.into(),
        });
        new
    };
    let first = create(&service, request("digest"), "create_request_ids").await;
    let again = service.store().create_draft(request("digest")).await;
    let Ok(DraftCreated::Replayed(replayed)) = again else {
        panic!("create_request_ids: not replayed: {again:?}");
    };
    assert_eq!(replayed, first);
    let other = service.store().create_draft(request("other")).await;
    let Ok(DraftCreated::Mismatch { draft_id }) = other else {
        panic!("create_request_ids: not a mismatch: {other:?}");
    };
    assert_eq!(draft_id, first.id);
}

async fn op_edits_bump_the_version_and_refuse_stale_writes<
    S: ScheduleStore + ChangeHistory + DraftStore,
>(
    store: S,
) {
    let (service, base, revision) = started(store).await;
    let created = create(&service, draft(&base, revision), "op_edits").await;
    let staged = |ord: usize| crate::domain::drafts::StagedOp {
        ord,
        op: crate::domain::drafts::DraftOp::ResetToFixed {
            run: crate::domain::drafts::Target::Existing("run-1".into()),
        },
        author: Actor::admin("root"),
        added_at: utc(27, 1, 30),
    };
    let ops = |id: &str, version: u64| {
        let change = DraftChange::ReplaceOps {
            ops: vec![staged(7)],
            event: DraftEventKind::OpAdded,
            ord: 0,
            expires_week: Some(utc(26, 16, 0)),
        };
        update(id, version, utc(27, 1, 30), change)
    };
    // A stale version writes nothing.
    assert!(matches!(
        write(&service, ops(&created.id, 7)).await,
        DraftWrite::Stale(DraftStale::Moved { version: 1, .. })
    ));
    let written = written(&service, ops(&created.id, 1), "op_edits").await;
    assert_eq!(written.version, 2);
    let loaded = loaded(&service, &created.id).await;
    assert_eq!(loaded.ops.len(), 1);
    assert_eq!(loaded.ops[0].ord, 0, "positions are renumbered on write");
    assert_eq!(
        loaded.draft.scope,
        DraftScope::Week(utc(26, 16, 0)),
        "the update stores its derived scope"
    );
    let events = events(&service, &created.id).await;
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [DraftEventKind::Created, DraftEventKind::OpAdded]
    );
    assert_eq!(events[1].version, 2);
    // A missing draft reports itself.
    assert!(matches!(
        write(&service, ops("missing", 2)).await,
        DraftWrite::Stale(DraftStale::Missing)
    ));
}

async fn rebase_moves_the_base_and_close_is_final<S: ScheduleStore + ChangeHistory + DraftStore>(
    store: S,
) {
    let (mut service, base, revision) = started(store).await;
    let created = create(&service, draft(&base, revision), "rebase").await;
    let id = created.id.as_str();
    // New history lands after the base.
    service
        .as_origin(admin())
        .add_fixed_run(solo_timing())
        .await
        .expect("upstream");
    let tip = head(&service).await;
    let at = utc(27, 3, 0);
    let rebase = DraftChange::Rebase {
        base: tip.clone(),
        base_revision: snapshot(&service).await.revision,
        expires_week: None,
        ops: None,
    };
    let rebased = written(&service, update(id, 1, at, rebase), "rebase").await;
    assert_eq!(rebased.version, 2);
    assert_eq!(rebased.base, tip);
    assert_eq!(
        rebased.scope,
        DraftScope::Weekly,
        "the rebase stores its derived scope"
    );
    // A rename bumps the version and keeps everything else.
    let retitle = DraftChange::Retitle {
        title: "weekend reshuffle".into(),
    };
    let renamed = written(&service, update(id, 2, at, retitle), "retitle").await;
    assert_eq!(
        (renamed.title.as_str(), renamed.version, &renamed.base),
        ("weekend reshuffle", 3, &tip)
    );
    // A settling rebase replaces the operations in the same write.
    let staged = crate::domain::drafts::StagedOp {
        ord: 5,
        op: crate::domain::drafts::DraftOp::ResetToFixed {
            run: crate::domain::drafts::Target::Existing("run-1".into()),
        },
        author: Actor::admin("root"),
        added_at: at,
    };
    let settle = DraftChange::Rebase {
        base: tip.clone(),
        base_revision: snapshot(&service).await.revision,
        expires_week: None,
        ops: Some(vec![staged.clone()]),
    };
    let settled = written(&service, update(id, 3, at, settle), "settling rebase").await;
    assert_eq!(settled.version, 4);
    let loaded = loaded(&service, id).await;
    assert_eq!(
        loaded.ops,
        [crate::domain::drafts::StagedOp { ord: 0, ..staged }],
        "the rebase renumbers the operations it stores"
    );
    assert_eq!(loaded.draft.title, "weekend reshuffle");
    // Only a merge commit closes a draft as merged (it names the record);
    // `Close` refuses non-closing statuses and writes nothing.
    for status in [
        DraftStatus::Merged,
        DraftStatus::Open,
        DraftStatus::Submitted,
    ] {
        let closing = update(id, 4, at, close(status, None));
        refused(service.store().update_draft(closing).await);
    }
    let discard = update(id, 4, at, close(DraftStatus::Discarded, Some("stale")));
    let closed = written(&service, discard, "rebase: close").await;
    assert_eq!(closed.status, DraftStatus::Discarded);
    assert_eq!(closed.version, 4, "closing does not bump the version");
    assert_eq!(closed.closed_by, Some(Actor::admin("root")));
    // A closed draft refuses every write.
    assert!(matches!(
        write(
            &service,
            update(id, 4, at, close(DraftStatus::Withdrawn, None))
        )
        .await,
        DraftWrite::Stale(DraftStale::Moved {
            status: DraftStatus::Discarded,
            ..
        })
    ));
    let events = events(&service, id).await;
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [
            DraftEventKind::Created,
            DraftEventKind::Rebased,
            DraftEventKind::Retitled,
            DraftEventKind::Rebased,
            DraftEventKind::Discarded
        ]
    );
    // The log dates the close (the History graph places it by this time),
    // and it is the row's last change.
    assert_eq!(closed_at(&events), Some(at));
    assert_eq!(closed.updated_at, at);
}

async fn commit_merge_writes_rows_record_and_close<
    S: ScheduleStore + ChangeHistory + DraftStore,
>(
    store: S,
) {
    let (service, base, revision) = started(store).await;
    let created = create(&service, draft(&base, revision), "commit_merge").await;
    let meta = merge_meta("merge-1", "digest");
    let committed = committed(&service, revision, meta, &created.id, "commit_merge").await;
    assert!(!committed.replayed);
    let after = snapshot(&service).await;
    assert!(after.runs.iter().any(|run| run.id == "run-9"));
    assert_eq!(after.revision, revision + 1);
    assert_eq!(committed.revision, revision + 1);
    let tip = head(&service).await;
    assert_eq!(tip.seq, committed.seq);
    let loaded = loaded(&service, &created.id).await;
    assert_eq!(loaded.draft.status, DraftStatus::Merged);
    assert_eq!(loaded.draft.merged_seq, Some(committed.seq));
    assert_eq!(loaded.draft.version, 2);
    assert_eq!(
        loaded.draft.closed_by,
        Some(Actor::admin("root")),
        "the merge names its merger"
    );
    let events = events(&service, &created.id).await;
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [DraftEventKind::Created, DraftEventKind::Merged]
    );
    // `merge_meta`'s time.
    assert_eq!(closed_at(&events), Some(utc(27, 2, 0)));
    assert_eq!(loaded.draft.updated_at, utc(27, 2, 0));
    // The record carries the merge surface and request.
    let record = service.store().load_change(committed.seq).await;
    let record = record.expect("record").expect("record");
    assert_eq!(record.origin.surface, Surface::DraftMerge);
    assert_eq!(record.origin.request_id.as_deref(), Some("merge-1"));
    // Merging again (another request id) reports the merged draft.
    let week = utc(26, 16, 0);
    let again = ChangeSet {
        changes: vec![Change::PutRun(run("run-10", week, utc(30, 20, 0)))],
    };
    let meta = merge_meta("merge-2", "digest");
    assert!(matches!(
        merge(&service, revision + 1, again, meta, &created.id, 2).await,
        Ok(MergeCommit::Stale(DraftStale::Moved {
            status: DraftStatus::Merged,
            merged_seq: Some(seq),
            ..
        })) if seq == committed.seq
    ));
}

async fn commit_merge_replays_request_ids<S: ScheduleStore + ChangeHistory + DraftStore>(store: S) {
    let (service, base, revision) = started(store).await;
    for id in ["draft-a", "draft-b"] {
        let mut new = draft(&base, revision);
        new.id = id.into();
        create(&service, new, "commit_merge_replay").await;
    }
    let meta = || merge_meta("merge-1", "digest");
    let first = committed(&service, revision, meta(), "draft-a", "commit_merge_replay").await;
    // The exact retry replays the record and writes nothing new.
    let replayed = committed(&service, revision + 1, meta(), "draft-b", "replay").await;
    assert!(replayed.replayed);
    assert_eq!(replayed.seq, first.seq);
    assert_eq!(head(&service).await.seq, first.seq, "nothing was written");
    // Another digest for the same request id is a mismatch.
    let other = merge_meta("merge-1", "other");
    assert!(matches!(
        merge(&service, revision + 1, run_9(), other, "draft-b", 1).await,
        Err(StoreError::IdempotencyMismatch { .. })
    ));
}

async fn commit_merge_refuses_empty_stale_and_conflicted<
    S: ScheduleStore + ChangeHistory + DraftStore,
>(
    store: S,
) {
    let (mut service, base, revision) = started(store).await;
    let created = create(&service, draft(&base, revision), "commit_merge_refusals").await;
    let meta = || merge_meta("merge-1", "digest");
    // An empty change set is refused.
    let empty = ChangeSet {
        changes: Vec::new(),
    };
    refused(merge(&service, revision, empty, meta(), &created.id, 1).await);
    // A missing draft reports itself.
    assert!(matches!(
        merge(&service, revision, run_9(), meta(), "missing", 1).await,
        Ok(MergeCommit::Stale(DraftStale::Missing))
    ));
    // A draft at another version reports itself.
    assert!(matches!(
        merge(&service, revision, run_9(), meta(), &created.id, 7).await,
        Ok(MergeCommit::Stale(DraftStale::Moved { version: 1, .. }))
    ));
    // A stale revision conflicts and writes nothing.
    service
        .as_origin(admin())
        .add_fixed_run(solo_timing())
        .await
        .expect("upstream");
    assert!(matches!(
        merge(&service, revision, run_9(), meta(), &created.id, 1).await,
        Err(StoreError::Conflict { .. })
    ));
    assert_eq!(
        head(&service).await.seq,
        base.seq + 1,
        "only the upstream write landed"
    );
}

async fn expiry_closes_only_past_weeks<S: ScheduleStore + ChangeHistory + DraftStore>(store: S) {
    let (service, base, revision) = started(store).await;
    for id in ["draft-past", "draft-current"] {
        let mut new = draft(&base, revision);
        new.id = id.into();
        create(&service, new, "expiry").await;
    }
    // The scope arrives with the operations, in the same write.
    let scope = DraftChange::ReplaceOps {
        ops: Vec::new(),
        event: DraftEventKind::OpAdded,
        ord: 0,
        expires_week: Some(utc(19, 16, 0)),
    };
    let scoped = update("draft-past", 1, utc(27, 1, 30), scope);
    written(&service, scoped, "expiry: scope").await;
    let expire = async |at| {
        let delivery = Actor::system("delivery");
        let store = service.store();
        store
            .expire_drafts(utc(26, 16, 0), at, &delivery, Vec::new())
            .await
    };
    assert_eq!(expire(utc(27, 4, 0)).await.expect("expire"), ["draft-past"]);
    let past = loaded(&service, "draft-past").await;
    assert_eq!(past.draft.status, DraftStatus::Expired);
    assert_eq!(past.draft.version, 2, "expiry does not bump the version");
    assert_eq!(
        past.draft.closed_by,
        Some(Actor::system("delivery")),
        "a closed draft names who closed it"
    );
    assert_eq!(past.draft.merged_seq, None, "only a merge names its record");
    let current = loaded(&service, "draft-current").await;
    assert_eq!(current.draft.status, DraftStatus::Open);
    assert_eq!(current.draft.scope, DraftScope::Weekly);
    assert_eq!(current.draft.closed_by, None);
    assert_eq!(
        kinds(&service, "draft-past").await,
        [
            DraftEventKind::Created,
            DraftEventKind::OpAdded,
            DraftEventKind::Expired
        ]
    );
    let past_closed = async || closed_at(&events(&service, "draft-past").await);
    assert_eq!(past_closed().await, Some(utc(27, 4, 0)));
    // Expiring again finds nothing new.
    assert!(expire(utc(27, 5, 0)).await.expect("expire").is_empty());
    // The close time stays the first expiry's.
    assert_eq!(past_closed().await, Some(utc(27, 4, 0)));
    let live = events(&service, "draft-current").await;
    assert_eq!(closed_at(&live), None, "a live draft has no close time");
}

async fn records_after_pages_the_chain<S: ScheduleStore + ChangeHistory + DraftStore>(store: S) {
    let mut service = service(store).await;
    let base = head(&service).await;
    for n in 0..3 {
        service
            .as_origin(admin())
            .add_fixed_run(NewFixedRun {
                owner_id: format!("{n}"),
                ..solo_timing()
            })
            .await
            .expect("upstream");
    }
    let records = service.store().records_after(&base).await.expect("records");
    assert_eq!(records.len(), 3);
    assert_eq!(records[0].seq, base.seq + 1);
    assert_eq!(records[2].prev_hash, records[1].hash);
    let tip = head(&service).await;
    assert_eq!(records[2].hash, tip.hash);
    // Nothing after the tip is an empty page run.
    let after_tip = service.store().records_after(&tip).await.expect("records");
    assert!(after_tip.is_empty());
}

async fn snapshot_with_head_covers_the_snapshot<S: ScheduleStore + ChangeHistory + DraftStore>(
    store: S,
) {
    let mut service = service(store).await;
    service
        .as_origin(admin())
        .add_fixed_run(solo_timing())
        .await
        .expect("upstream");
    let (snapshot, head) = service.store().snapshot_with_head().await.expect("read");
    let tip = service.store().history_head().await.expect("head");
    assert_eq!(
        head, tip,
        "the head covers every record the snapshot reflects"
    );
    let record = service.store().load_change(head.seq).await;
    let tip_revision = record.expect("record").expect("record").revision;
    assert_eq!(snapshot.revision, tip_revision);
    assert_eq!(snapshot.fixed_runs.len(), 1);
}

/// A member request `id` by member `1`, submitted at `at` with no
/// operations (the store only counts and stores).
fn request(base: &ChangeRef, id: &str, at: DateTime<Utc>) -> NewDraft {
    NewDraft {
        id: id.into(),
        kind: crate::domain::drafts::DraftKind::Request,
        title: "join".into(),
        author: Actor::member("1"),
        base: base.clone(),
        base_revision: 0,
        request_type: Some("join".into()),
        subject: Some("run:r-1".into()),
        at,
        request: None,
        submit: Some(crate::domain::drafts::Submission {
            ops: Vec::new(),
            expires_week: None,
            limits: crate::domain::requests::DEFAULT_LIMITS,
        }),
    }
}

async fn withdraw<S: DraftStore>(store: &S, id: &str, at: DateTime<Utc>) {
    let withdrawal = DraftUpdate {
        actor: Actor::member("1"),
        ..update(id, 1, at, close(DraftStatus::Withdrawn, None))
    };
    let withdrawn = store.update_draft(withdrawal).await.expect("withdraw");
    let DraftWrite::Written(_) = withdrawn else {
        panic!("request_limits: not withdrawn: {withdrawn:?}");
    };
}

async fn request_limits_are_counted_inside_the_insert<
    S: ScheduleStore + ChangeHistory + DraftStore,
>(
    store: S,
) {
    use crate::domain::drafts::RequestLimit;

    let service = service(store).await;
    let base = head(&service).await;
    let store = service.store();
    let submit = async |id: &str, at| {
        let created = store.create_draft(request(&base, id, at)).await;
        created.expect("submit")
    };
    for n in 0..3 {
        let created = submit(&format!("q-{n}"), utc(27, 1, n)).await;
        let DraftCreated::Created(draft) = created else {
            panic!("request_limits: {created:?}");
        };
        assert_eq!(draft.status, DraftStatus::Submitted, "request_limits");
    }
    assert_eq!(
        kinds(&service, "q-0").await,
        [DraftEventKind::Created, DraftEventKind::Submitted],
        "request_limits: submitted at creation"
    );
    // A fourth pending request is over the cap; nothing is written.
    assert_eq!(
        submit("q-3", utc(27, 2, 0)).await,
        DraftCreated::Limited(RequestLimit::Pending { count: 3, max: 3 }),
        "request_limits: pending cap"
    );
    assert!(
        store.load_draft("q-3").await.expect("load").is_none(),
        "request_limits: nothing written"
    );
    // Withdrawn requests free the cap but still count in the window.
    for n in 0..3 {
        withdraw(store, &format!("q-{n}"), utc(27, 3, 0)).await;
    }
    for n in 3..6 {
        let created = submit(&format!("q-{n}"), utc(27, 4, n)).await;
        assert!(
            matches!(created, DraftCreated::Created(_)),
            "request_limits: {created:?}"
        );
    }
    for n in 3..6 {
        withdraw(store, &format!("q-{n}"), utc(27, 5, 0)).await;
    }
    assert_eq!(
        submit("q-6", utc(27, 6, 0)).await,
        DraftCreated::Limited(RequestLimit::Rate { count: 6, max: 6 }),
        "request_limits: rolling window"
    );
    // A day after the first submissions the window has room again.
    let created = submit("q-7", utc(28, 1, 1)).await;
    assert!(
        matches!(created, DraftCreated::Created(_)),
        "request_limits: window rolls {created:?}"
    );
}

async fn concurrent_submissions_stay_at_the_cap<
    S: ScheduleStore + ChangeHistory + DraftStore + Sync,
>(
    store: S,
) {
    let service = service(store).await;
    let base = head(&service).await;
    let store = service.store();
    for n in 0..2 {
        let created = store
            .create_draft(request(&base, &format!("c-{n}"), utc(27, 1, n)))
            .await
            .expect("submit");
        assert!(matches!(created, DraftCreated::Created(_)), "concurrent");
    }
    let (left, right) = tokio::join!(
        store.create_draft(request(&base, "c-2", utc(27, 2, 0))),
        store.create_draft(request(&base, "c-3", utc(27, 2, 0))),
    );
    let outcomes = [left.expect("left"), right.expect("right")];
    let created = outcomes
        .iter()
        .filter(|outcome| matches!(outcome, DraftCreated::Created(_)))
        .count();
    assert_eq!(created, 1, "concurrent: one wins {outcomes:?}");
    let submitted = store.list_drafts(Some(DraftStatus::Submitted)).await;
    assert_eq!(submitted.expect("list").len(), 3, "concurrent: at the cap");
}

async fn commit_merge_records_its_note<S: ScheduleStore + ChangeHistory + DraftStore>(store: S) {
    let (service, base, revision) = started(store).await;
    let created = create(&service, draft(&base, revision), "merge_note").await;
    let meta = merge_meta("merge-1", "digest");
    let note = Some("choices=update_all".into());
    let merged = service
        .store()
        .commit_merge(revision, run_9(), meta, &created.id, 1, note);
    let Ok(MergeCommit::Committed(committed)) = merged.await else {
        panic!("merge_note: not committed");
    };
    let events = events(&service, &created.id).await;
    let merged = events.last().expect("merged event");
    assert_eq!(merged.kind, DraftEventKind::Merged, "merge_note");
    assert_eq!(
        merged.detail,
        Some(format!("{} choices=update_all", committed.seq)),
        "merge_note: the note follows the seq"
    );
}
