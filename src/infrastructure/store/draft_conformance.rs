//! The draft storage every store must keep, driven through
//! [`SchedulerService`] against a fresh store per check. Failures panic with
//! the check name.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};

use crate::domain::drafts::{
    DraftChange, DraftCreated, DraftEventKind, DraftScope, DraftStale, DraftStatus, DraftStore,
    DraftUpdate, DraftWrite, MergeCommit, NewDraft,
};
use crate::domain::history::{Actor, ChangeHistory, ChangeMeta, ChangeRef, Origin, Surface};
use crate::domain::ids::IdGenerator;
use crate::domain::schedule::{
    Change, ChangeSet, NewFixedRun, Run, RunSource, RunStatus, ScheduleSnapshot,
};
use crate::domain::scheduler::{Clock, ScheduleStore, SchedulerService, Scope, StoreError};

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

#[derive(Clone, Default)]
struct Ids(Arc<Mutex<u64>>);

impl IdGenerator for Ids {
    fn new_id(&mut self) -> String {
        let mut next = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *next += 1;
        format!("id-{next:04}")
    }
}

#[derive(Clone)]
struct TestClock(Arc<Mutex<DateTime<Utc>>>);

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, day, hour, minute, 0)
        .single()
        .expect("valid instant")
}

fn admin() -> Origin {
    Origin::new(Actor::admin("root"), Surface::AdminPortal)
}

type Service<S> = SchedulerService<S, Ids, TestClock>;

async fn service<S: ScheduleStore>(store: S) -> Service<S> {
    SchedulerService::new(
        store,
        Ids::default(),
        TestClock(Arc::new(Mutex::new(utc(27, 1, 0)))),
    )
}

async fn head<S: ScheduleStore + ChangeHistory>(service: &Service<S>) -> ChangeRef {
    service.store().history_head().await.expect("head")
}

async fn snapshot<S: ScheduleStore>(service: &Service<S>) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.expect("load")
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
    let service = service(store).await;
    let base = head(&service).await;
    let revision = snapshot(&service).await.revision;
    let DraftCreated::Created(created) = service
        .store()
        .create_draft(draft(&base, revision))
        .await
        .expect("create")
    else {
        panic!("create_load_list_and_log: not created");
    };
    assert_eq!(created.version, 1);
    assert_eq!(created.status, DraftStatus::Open);
    assert_eq!(created.base, base);
    let loaded = service
        .store()
        .load_draft("draft-1")
        .await
        .expect("load")
        .expect("present");
    assert_eq!(loaded.draft, created);
    assert!(loaded.ops.is_empty());
    assert!(
        service
            .store()
            .load_draft("missing")
            .await
            .expect("load")
            .is_none()
    );
    let all = service.store().list_drafts(None).await.expect("list");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0], created);
    let open = service
        .store()
        .list_drafts(Some(DraftStatus::Open))
        .await
        .expect("list");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0], created, "the draft lists under its status");
    assert!(
        service
            .store()
            .list_drafts(Some(DraftStatus::Merged))
            .await
            .expect("list")
            .is_empty()
    );
    let mut second = draft(&base, revision);
    second.id = "draft-2".into();
    let DraftCreated::Created(_) = service.store().create_draft(second).await.expect("create")
    else {
        panic!("create_load_list_and_log: second draft not created");
    };
    let ids: Vec<String> = service
        .store()
        .list_drafts(None)
        .await
        .expect("list")
        .iter()
        .map(|draft| draft.id.clone())
        .collect();
    assert_eq!(ids, ["draft-1", "draft-2"], "drafts list in creation order");
    let events = service
        .store()
        .draft_events("draft-1")
        .await
        .expect("events");
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
    let service = service(store).await;
    let base = head(&service).await;
    let revision = snapshot(&service).await.revision;
    let request = |digest: &str| {
        let mut new = draft(&base, revision);
        new.id = "draft-r".into();
        new.request = Some(crate::domain::drafts::DraftRequest {
            request_id: "create-1".into(),
            digest: digest.into(),
        });
        new
    };
    let DraftCreated::Created(first) = service
        .store()
        .create_draft(request("digest"))
        .await
        .expect("create")
    else {
        panic!("create_request_ids: not created");
    };
    let DraftCreated::Replayed(replayed) = service
        .store()
        .create_draft(request("digest"))
        .await
        .expect("replay")
    else {
        panic!("create_request_ids: not replayed");
    };
    assert_eq!(replayed, first);
    let DraftCreated::Mismatch { draft_id } = service
        .store()
        .create_draft(request("other"))
        .await
        .expect("mismatch")
    else {
        panic!("create_request_ids: not a mismatch");
    };
    assert_eq!(draft_id, first.id);
}

async fn op_edits_bump_the_version_and_refuse_stale_writes<
    S: ScheduleStore + ChangeHistory + DraftStore,
>(
    store: S,
) {
    let service = service(store).await;
    let base = head(&service).await;
    let revision = snapshot(&service).await.revision;
    let DraftCreated::Created(created) = service
        .store()
        .create_draft(draft(&base, revision))
        .await
        .expect("create")
    else {
        panic!("op_edits: not created");
    };
    let staged = |ord: usize| crate::domain::drafts::StagedOp {
        ord,
        op: crate::domain::drafts::DraftOp::ResetToFixed {
            run: crate::domain::drafts::Target::Existing("run-1".into()),
        },
        author: Actor::admin("root"),
        added_at: utc(27, 1, 30),
    };
    let update = |version: u64| DraftUpdate {
        draft_id: created.id.clone(),
        expected_version: version,
        actor: Actor::admin("root"),
        at: utc(27, 1, 30),
        change: DraftChange::ReplaceOps {
            ops: vec![staged(7)],
            event: DraftEventKind::OpAdded,
            ord: 0,
            expires_week: Some(utc(26, 16, 0)),
        },
    };
    // A stale version writes nothing.
    assert!(matches!(
        service
            .store()
            .update_draft(update(7))
            .await
            .expect("stale"),
        DraftWrite::Stale(DraftStale::Moved { version: 1, .. })
    ));
    let DraftWrite::Written(written) = service
        .store()
        .update_draft(update(1))
        .await
        .expect("write")
    else {
        panic!("op_edits: not written");
    };
    assert_eq!(written.version, 2);
    let loaded = service
        .store()
        .load_draft(&created.id)
        .await
        .expect("load")
        .expect("present");
    assert_eq!(loaded.ops.len(), 1);
    assert_eq!(loaded.ops[0].ord, 0, "positions are renumbered on write");
    assert_eq!(
        loaded.draft.scope,
        DraftScope::Week(utc(26, 16, 0)),
        "the update stores its derived scope"
    );
    let events = service
        .store()
        .draft_events(&created.id)
        .await
        .expect("events");
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [DraftEventKind::Created, DraftEventKind::OpAdded]
    );
    assert_eq!(events[1].version, 2);
    // A missing draft reports itself.
    assert!(matches!(
        service
            .store()
            .update_draft(DraftUpdate {
                draft_id: "missing".into(),
                ..update(2)
            })
            .await
            .expect("missing"),
        DraftWrite::Stale(DraftStale::Missing)
    ));
}

async fn rebase_moves_the_base_and_close_is_final<S: ScheduleStore + ChangeHistory + DraftStore>(
    store: S,
) {
    let mut service = service(store).await;
    let base = head(&service).await;
    let revision = snapshot(&service).await.revision;
    let DraftCreated::Created(created) = service
        .store()
        .create_draft(draft(&base, revision))
        .await
        .expect("create")
    else {
        panic!("rebase: not created");
    };
    // New history lands after the base.
    service
        .as_origin(admin())
        .add_fixed_run(NewFixedRun {
            owner_pinned: false,
            owner_id: "1".into(),
            channel_id: None,
            bosses: vec!["HFA".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(20, 0, 0).expect("valid time"),
            participants: vec!["1".into()],
            note: None,
        })
        .await
        .expect("upstream");
    let tip = head(&service).await;
    let at = utc(27, 3, 0);
    let DraftWrite::Written(rebased) = service
        .store()
        .update_draft(DraftUpdate {
            draft_id: created.id.clone(),
            expected_version: 1,
            actor: Actor::admin("root"),
            at,
            change: DraftChange::Rebase {
                base: tip.clone(),
                base_revision: snapshot(&service).await.revision,
                expires_week: None,
            },
        })
        .await
        .expect("rebase")
    else {
        panic!("rebase: not written");
    };
    assert_eq!(rebased.version, 2);
    assert_eq!(rebased.base, tip);
    assert_eq!(
        rebased.scope,
        DraftScope::Weekly,
        "the rebase stores its derived scope"
    );
    // Only a merge commit closes a draft as merged (it names the record);
    // `Close` refuses non-closing statuses and writes nothing.
    for status in [
        DraftStatus::Merged,
        DraftStatus::Open,
        DraftStatus::Submitted,
    ] {
        assert!(matches!(
            service
                .store()
                .update_draft(DraftUpdate {
                    draft_id: created.id.clone(),
                    expected_version: 2,
                    actor: Actor::admin("root"),
                    at,
                    change: DraftChange::Close {
                        status,
                        reason: None,
                        notices: Vec::new(),
                    },
                })
                .await,
            Err(StoreError::Constraint(_))
        ));
    }
    let DraftWrite::Written(closed) = service
        .store()
        .update_draft(DraftUpdate {
            draft_id: created.id.clone(),
            expected_version: 2,
            actor: Actor::admin("root"),
            at,
            change: DraftChange::Close {
                status: DraftStatus::Discarded,
                reason: Some("stale".into()),
                notices: Vec::new(),
            },
        })
        .await
        .expect("close")
    else {
        panic!("rebase: not closed");
    };
    assert_eq!(closed.status, DraftStatus::Discarded);
    assert_eq!(closed.version, 2, "closing does not bump the version");
    assert_eq!(closed.closed_by, Some(Actor::admin("root")));
    // A closed draft refuses every write.
    assert!(matches!(
        service
            .store()
            .update_draft(DraftUpdate {
                draft_id: created.id.clone(),
                expected_version: 2,
                actor: Actor::admin("root"),
                at,
                change: DraftChange::Close {
                    status: DraftStatus::Withdrawn,
                    reason: None,
                    notices: Vec::new(),
                },
            })
            .await
            .expect("closed"),
        DraftWrite::Stale(DraftStale::Moved {
            status: DraftStatus::Discarded,
            ..
        })
    ));
    let events = service
        .store()
        .draft_events(&created.id)
        .await
        .expect("events");
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [
            DraftEventKind::Created,
            DraftEventKind::Rebased,
            DraftEventKind::Discarded
        ]
    );
}

async fn commit_merge_writes_rows_record_and_close<
    S: ScheduleStore + ChangeHistory + DraftStore,
>(
    store: S,
) {
    let service = service(store).await;
    let base = head(&service).await;
    let revision = snapshot(&service).await.revision;
    let DraftCreated::Created(created) = service
        .store()
        .create_draft(draft(&base, revision))
        .await
        .expect("create")
    else {
        panic!("commit_merge: not created");
    };
    let week = utc(26, 16, 0);
    let changes = ChangeSet {
        changes: vec![Change::PutRun(run("run-9", week, utc(29, 20, 0)))],
    };
    let MergeCommit::Committed(committed) = service
        .store()
        .commit_merge(
            revision,
            changes,
            merge_meta("merge-1", "digest"),
            &created.id,
            1,
            None,
        )
        .await
        .expect("merge")
    else {
        panic!("commit_merge: not committed");
    };
    assert!(!committed.replayed);
    let after = snapshot(&service).await;
    assert!(after.runs.iter().any(|run| run.id == "run-9"));
    assert_eq!(after.revision, revision + 1);
    assert_eq!(committed.revision, revision + 1);
    let tip = head(&service).await;
    assert_eq!(tip.seq, committed.seq);
    let loaded = service
        .store()
        .load_draft(&created.id)
        .await
        .expect("load")
        .expect("present");
    assert_eq!(loaded.draft.status, DraftStatus::Merged);
    assert_eq!(loaded.draft.merged_seq, Some(committed.seq));
    assert_eq!(loaded.draft.version, 2);
    assert_eq!(
        loaded.draft.closed_by,
        Some(Actor::admin("root")),
        "the merge names its merger"
    );
    let events = service
        .store()
        .draft_events(&created.id)
        .await
        .expect("events");
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [DraftEventKind::Created, DraftEventKind::Merged]
    );
    // The record carries the merge surface and request.
    let record = service
        .store()
        .load_change(committed.seq)
        .await
        .expect("record")
        .expect("record");
    assert_eq!(record.origin.surface, Surface::DraftMerge);
    assert_eq!(record.origin.request_id.as_deref(), Some("merge-1"));
    // Merging again (another request id) reports the merged draft.
    assert!(matches!(
        service
            .store()
            .commit_merge(
                revision + 1,
                ChangeSet {
                    changes: vec![Change::PutRun(run("run-10", week, utc(30, 20, 0)))],
                },
                merge_meta("merge-2", "digest"),
                &created.id,
                2,
                None,
            )
            .await,
        Ok(MergeCommit::Stale(DraftStale::Moved {
            status: DraftStatus::Merged,
            merged_seq: Some(seq),
            ..
        })) if seq == committed.seq
    ));
}

async fn commit_merge_replays_request_ids<S: ScheduleStore + ChangeHistory + DraftStore>(store: S) {
    let service = service(store).await;
    let base = head(&service).await;
    let revision = snapshot(&service).await.revision;
    for id in ["draft-a", "draft-b"] {
        let mut new = draft(&base, revision);
        new.id = id.into();
        let DraftCreated::Created(_) = service.store().create_draft(new).await.expect("create")
        else {
            panic!("commit_merge_replay: not created");
        };
    }
    let week = utc(26, 16, 0);
    let changes = || ChangeSet {
        changes: vec![Change::PutRun(run("run-9", week, utc(29, 20, 0)))],
    };
    let MergeCommit::Committed(first) = service
        .store()
        .commit_merge(
            revision,
            changes(),
            merge_meta("merge-1", "digest"),
            "draft-a",
            1,
            None,
        )
        .await
        .expect("merge")
    else {
        panic!("commit_merge_replay: not committed");
    };
    // The exact retry replays the record and writes nothing new.
    let MergeCommit::Committed(replayed) = service
        .store()
        .commit_merge(
            revision + 1,
            changes(),
            merge_meta("merge-1", "digest"),
            "draft-b",
            1,
            None,
        )
        .await
        .expect("replay")
    else {
        panic!("commit_merge_replay: not replayed");
    };
    assert!(replayed.replayed);
    assert_eq!(replayed.seq, first.seq);
    assert_eq!(head(&service).await.seq, first.seq, "nothing was written");
    // Another digest for the same request id is a mismatch.
    assert!(matches!(
        service
            .store()
            .commit_merge(
                revision + 1,
                changes(),
                merge_meta("merge-1", "other"),
                "draft-b",
                1,
                None
            )
            .await,
        Err(StoreError::IdempotencyMismatch { .. })
    ));
}

async fn commit_merge_refuses_empty_stale_and_conflicted<
    S: ScheduleStore + ChangeHistory + DraftStore,
>(
    store: S,
) {
    let mut service = service(store).await;
    let base = head(&service).await;
    let revision = snapshot(&service).await.revision;
    let DraftCreated::Created(created) = service
        .store()
        .create_draft(draft(&base, revision))
        .await
        .expect("create")
    else {
        panic!("commit_merge_refusals: not created");
    };
    // An empty change set is refused.
    assert!(matches!(
        service
            .store()
            .commit_merge(
                revision,
                ChangeSet {
                    changes: Vec::new()
                },
                merge_meta("merge-1", "digest"),
                &created.id,
                1,
                None
            )
            .await,
        Err(StoreError::Constraint(_))
    ));
    // A missing draft reports itself.
    assert!(matches!(
        service
            .store()
            .commit_merge(
                revision,
                ChangeSet {
                    changes: vec![Change::PutRun(run("run-9", utc(26, 16, 0), utc(29, 20, 0)))],
                },
                merge_meta("merge-1", "digest"),
                "missing",
                1,
                None
            )
            .await,
        Ok(MergeCommit::Stale(DraftStale::Missing))
    ));
    // A draft at another version reports itself.
    assert!(matches!(
        service
            .store()
            .commit_merge(
                revision,
                ChangeSet {
                    changes: vec![Change::PutRun(run("run-9", utc(26, 16, 0), utc(29, 20, 0)))],
                },
                merge_meta("merge-1", "digest"),
                &created.id,
                7,
                None
            )
            .await,
        Ok(MergeCommit::Stale(DraftStale::Moved { version: 1, .. }))
    ));
    // A stale revision conflicts and writes nothing.
    service
        .as_origin(admin())
        .add_fixed_run(NewFixedRun {
            owner_pinned: false,
            owner_id: "1".into(),
            channel_id: None,
            bosses: vec!["HFA".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(20, 0, 0).expect("valid time"),
            participants: vec!["1".into()],
            note: None,
        })
        .await
        .expect("upstream");
    assert!(matches!(
        service
            .store()
            .commit_merge(
                revision,
                ChangeSet {
                    changes: vec![Change::PutRun(run("run-9", utc(26, 16, 0), utc(29, 20, 0)))],
                },
                merge_meta("merge-1", "digest"),
                &created.id,
                1,
                None
            )
            .await,
        Err(StoreError::Conflict { .. })
    ));
    assert_eq!(
        head(&service).await.seq,
        base.seq + 1,
        "only the upstream write landed"
    );
}

async fn expiry_closes_only_past_weeks<S: ScheduleStore + ChangeHistory + DraftStore>(store: S) {
    let service = service(store).await;
    let base = head(&service).await;
    let revision = snapshot(&service).await.revision;
    for id in ["draft-past", "draft-current"] {
        let mut new = draft(&base, revision);
        new.id = id.into();
        let DraftCreated::Created(_) = service.store().create_draft(new).await.expect("create")
        else {
            panic!("expiry: not created");
        };
    }
    // The scope arrives with the operations, in the same write.
    let scope = |week: Option<DateTime<Utc>>| DraftUpdate {
        draft_id: "draft-past".into(),
        expected_version: 1,
        actor: Actor::admin("root"),
        at: utc(27, 1, 30),
        change: DraftChange::ReplaceOps {
            ops: Vec::new(),
            event: DraftEventKind::OpAdded,
            ord: 0,
            expires_week: week,
        },
    };
    let DraftWrite::Written(_) = service
        .store()
        .update_draft(scope(Some(utc(19, 16, 0))))
        .await
        .expect("scope")
    else {
        panic!("expiry: scope not written");
    };
    let expired = service
        .store()
        .expire_drafts(
            utc(26, 16, 0),
            utc(27, 4, 0),
            &Actor::system("delivery"),
            Vec::new(),
        )
        .await
        .expect("expire");
    assert_eq!(expired, ["draft-past"]);
    let past = service
        .store()
        .load_draft("draft-past")
        .await
        .expect("load")
        .expect("present");
    assert_eq!(past.draft.status, DraftStatus::Expired);
    assert_eq!(past.draft.version, 2, "expiry does not bump the version");
    assert_eq!(
        past.draft.closed_by,
        Some(Actor::system("delivery")),
        "a closed draft names who closed it"
    );
    assert_eq!(past.draft.merged_seq, None, "only a merge names its record");
    let current = service
        .store()
        .load_draft("draft-current")
        .await
        .expect("load")
        .expect("present");
    assert_eq!(current.draft.status, DraftStatus::Open);
    assert_eq!(current.draft.scope, DraftScope::Weekly);
    assert_eq!(current.draft.closed_by, None);
    let events = service
        .store()
        .draft_events("draft-past")
        .await
        .expect("events");
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [
            DraftEventKind::Created,
            DraftEventKind::OpAdded,
            DraftEventKind::Expired
        ]
    );
    // Expiring again finds nothing new.
    assert!(
        service
            .store()
            .expire_drafts(
                utc(26, 16, 0),
                utc(27, 5, 0),
                &Actor::system("delivery"),
                Vec::new()
            )
            .await
            .expect("expire")
            .is_empty()
    );
}

async fn records_after_pages_the_chain<S: ScheduleStore + ChangeHistory + DraftStore>(store: S) {
    let mut service = service(store).await;
    let base = head(&service).await;
    for n in 0..3 {
        service
            .as_origin(admin())
            .add_fixed_run(NewFixedRun {
                owner_pinned: false,
                owner_id: format!("{n}"),
                channel_id: None,
                bosses: vec!["HFA".into()],
                weekday: Weekday::Sat,
                time: NaiveTime::from_hms_opt(20, 0, 0).expect("valid time"),
                participants: vec!["1".into()],
                note: None,
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
    assert!(
        service
            .store()
            .records_after(&tip)
            .await
            .expect("records")
            .is_empty()
    );
}

async fn snapshot_with_head_covers_the_snapshot<S: ScheduleStore + ChangeHistory + DraftStore>(
    store: S,
) {
    let mut service = service(store).await;
    service
        .as_origin(admin())
        .add_fixed_run(NewFixedRun {
            owner_pinned: false,
            owner_id: "1".into(),
            channel_id: None,
            bosses: vec!["HFA".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(20, 0, 0).expect("valid time"),
            participants: vec!["1".into()],
            note: None,
        })
        .await
        .expect("upstream");
    let (snapshot, head) = service.store().snapshot_with_head().await.expect("read");
    let tip = service.store().history_head().await.expect("head");
    assert_eq!(
        head, tip,
        "the head covers every record the snapshot reflects"
    );
    assert_eq!(snapshot.revision, tip_revision(&service, &head).await);
    assert_eq!(snapshot.fixed_runs.len(), 1);
}

async fn tip_revision<S: ScheduleStore + ChangeHistory>(
    service: &Service<S>,
    head: &ChangeRef,
) -> u64 {
    service
        .store()
        .load_change(head.seq)
        .await
        .expect("record")
        .expect("record")
        .revision
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
    let DraftWrite::Written(_) = store
        .update_draft(DraftUpdate {
            draft_id: id.into(),
            expected_version: 1,
            actor: Actor::member("1"),
            at,
            change: DraftChange::Close {
                status: DraftStatus::Withdrawn,
                reason: None,
                notices: Vec::new(),
            },
        })
        .await
        .expect("withdraw")
    else {
        panic!("request_limits: not withdrawn");
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
    for n in 0..3 {
        let created = store
            .create_draft(request(&base, &format!("q-{n}"), utc(27, 1, n)))
            .await
            .expect("submit");
        let DraftCreated::Created(draft) = created else {
            panic!("request_limits: {created:?}");
        };
        assert_eq!(draft.status, DraftStatus::Submitted, "request_limits");
    }
    let events = store.draft_events("q-0").await.expect("events");
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [DraftEventKind::Created, DraftEventKind::Submitted],
        "request_limits: submitted at creation"
    );
    // A fourth pending request is over the cap; nothing is written.
    assert_eq!(
        store
            .create_draft(request(&base, "q-3", utc(27, 2, 0)))
            .await
            .expect("submit"),
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
        let created = store
            .create_draft(request(&base, &format!("q-{n}"), utc(27, 4, n)))
            .await
            .expect("submit");
        assert!(
            matches!(created, DraftCreated::Created(_)),
            "request_limits: {created:?}"
        );
    }
    for n in 3..6 {
        withdraw(store, &format!("q-{n}"), utc(27, 5, 0)).await;
    }
    assert_eq!(
        store
            .create_draft(request(&base, "q-6", utc(27, 6, 0)))
            .await
            .expect("submit"),
        DraftCreated::Limited(RequestLimit::Rate { count: 6, max: 6 }),
        "request_limits: rolling window"
    );
    // A day after the first submissions the window has room again.
    let created = store
        .create_draft(request(&base, "q-7", utc(28, 1, 1)))
        .await
        .expect("submit");
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
    assert_eq!(
        store
            .list_drafts(Some(DraftStatus::Submitted))
            .await
            .expect("list")
            .len(),
        3,
        "concurrent: at the cap"
    );
}

async fn commit_merge_records_its_note<S: ScheduleStore + ChangeHistory + DraftStore>(store: S) {
    let service = service(store).await;
    let base = head(&service).await;
    let revision = snapshot(&service).await.revision;
    let DraftCreated::Created(created) = service
        .store()
        .create_draft(draft(&base, revision))
        .await
        .expect("create")
    else {
        panic!("merge_note: not created");
    };
    let MergeCommit::Committed(committed) = service
        .store()
        .commit_merge(
            revision,
            ChangeSet {
                changes: vec![Change::PutRun(run("run-9", utc(26, 16, 0), utc(29, 20, 0)))],
            },
            merge_meta("merge-1", "digest"),
            &created.id,
            1,
            Some("choices=update_all".into()),
        )
        .await
        .expect("merge")
    else {
        panic!("merge_note: not committed");
    };
    let events = service
        .store()
        .draft_events(&created.id)
        .await
        .expect("events");
    let merged = events.last().expect("merged event");
    assert_eq!(merged.kind, DraftEventKind::Merged, "merge_note");
    assert_eq!(
        merged.detail,
        Some(format!("{} choices=update_all", committed.seq)),
        "merge_note: the note follows the seq"
    );
}
