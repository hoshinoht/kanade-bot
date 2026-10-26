//! Administrator drafts over a temp SQLite store: staging, preview,
//! rebase, merge, expiry and their errors. Nothing here touches the network,
//! and previews never write.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, FixedOffset, TimeDelta, TimeZone, Utc, Weekday};
use kanade::infrastructure::store::SqliteStore;
use kanade_domain::drafts::{
    DraftCreated, DraftEvent, DraftEventKind, DraftKind, DraftOp, DraftScope, DraftStatus,
    DraftStore, DraftUpdate, DraftWrite, Field, LoadedDraft, MergeAnalysis, MergeCommit,
    MergeConflict, NewDraft, ReplayError, Settle, StagedOp, StoredDraft, Target,
};
use kanade_domain::history::{
    Actor, BlameTarget, ChangeFilter, ChangeHistory, ChangeMeta, ChangePage, ChangeQuery,
    ChangeRecord, ChangeRef, CheckedChange, HistoryVerification, Origin, Surface, blame,
};
use kanade_domain::ids::RandomIds;
use kanade_domain::notify::{DeliveryJournal, DeliverySettings, plan_notice};
use kanade_domain::schedule::{
    Change, ChangeSet, DRAFT_MERGED, FixedEdit, FixedEditChoices, NewFixedRun, Notice,
    NoticeChange, RsvpSource, RsvpState, RunStatus, ScheduleSnapshot, StatusChange,
};
use kanade_domain::scheduler::{
    Committed, DraftError, DraftResult, MergeOutcome, RecordedRequest, ScheduleStore,
    SchedulerService, Scope, StoreError,
};

use crate::common::TestClock;
use crate::support::{
    AnyChannel, Guild, TwoTimings as Fixture, amend, by_tests, existing, hm, kalos_run, kl, now,
    policy, rsvp, snapshot, swap, two_timings as fixture, utc, weekly,
};

type Svc<S> = SchedulerService<S, RandomIds, TestClock>;
type Service = Svc<SqliteStore>;

async fn head_seq(service: &Service) -> u64 {
    service.store().history_head().await.unwrap().seq
}

async fn journal_is_empty(service: &Service) {
    let view = service.store().load_view().await.unwrap();
    assert!(view.targets().is_empty(), "the journal holds no claims");
}

async fn loaded(service: &Service, id: &str) -> LoadedDraft {
    service.store().load_draft(id).await.unwrap().unwrap()
}

fn fixed_note(fixed: &str, note: &str) -> DraftOp {
    DraftOp::ApplyFixedEdit {
        fixed: existing(fixed),
        edit: FixedEdit {
            note: Some(note.to_owned()),
            ..FixedEdit::default()
        },
        choices: FixedEditChoices::UpdateAll,
    }
}

fn kalos_timing() -> DraftOp {
    DraftOp::AddFixedRun(NewFixedRun {
        bosses: vec!["Kalos".into()],
        ..weekly("222", Weekday::Fri, hm(22, 0), &["1001"])
    })
}

// The admin calls, as `root` with the shared policy and guild.

async fn add<S: ScheduleStore + DraftStore>(
    service: &mut Svc<S>,
    id: &str,
    version: u64,
    op: DraftOp,
) -> DraftResult<LoadedDraft> {
    let policy = policy();
    service
        .add_draft_op("root", id, version, op, &policy, &Guild)
        .await
}

async fn edit(
    service: &mut Service,
    id: &str,
    version: u64,
    ord: usize,
    op: DraftOp,
) -> DraftResult<LoadedDraft> {
    let policy = policy();
    service
        .edit_draft_op("root", id, version, ord, op, &policy, &Guild)
        .await
}

async fn remove(
    service: &mut Service,
    id: &str,
    version: u64,
    ord: usize,
) -> DraftResult<LoadedDraft> {
    let policy = policy();
    service
        .remove_draft_op("root", id, version, ord, &policy, &Guild)
        .await
}

async fn preview(service: &Service, id: &str) -> DraftResult<MergeAnalysis> {
    service.preview_draft(id, &policy(), &Guild).await
}

async fn rebase(service: &mut Service, id: &str, version: u64) -> DraftResult<StoredDraft> {
    let policy = policy();
    service
        .rebase_draft("root", id, version, &policy, &Guild)
        .await
}

async fn merge_as<S: ScheduleStore + DraftStore>(
    service: &mut Svc<S>,
    id: &str,
    version: u64,
    request_id: Option<&str>,
) -> DraftResult<MergeOutcome> {
    let (policy, request_id) = (policy(), request_id.map(Into::into));
    service
        .merge_draft("root", id, version, &policy, &Guild, request_id)
        .await
}

async fn merge(service: &mut Service, id: &str, version: u64) -> DraftResult<MergeOutcome> {
    merge_as(service, id, version, None).await
}

/// Another writer moves `run` outside the draft.
async fn upstream_amend(service: &mut Service, run: &str, to: DateTime<FixedOffset>) {
    let policy = policy();
    let amended = by_tests(service).amend_run(run, utc(to), &policy);
    amended.await.unwrap();
}

async fn stage(service: &mut Service, ops: &[DraftOp]) -> String {
    let draft = service.create_draft("root", "retime", None).await.unwrap();
    let mut version = draft.version;
    for op in ops {
        version = add(service, &draft.id, version, op.clone())
            .await
            .unwrap()
            .draft
            .version;
    }
    draft.id
}

fn merged_channels(outcome: &MergeOutcome) -> BTreeSet<Option<String>> {
    outcome
        .notices
        .iter()
        .map(|notice| notice.channel_id.clone())
        .collect()
}

/// The run and timing ids a merge summary lists.
fn merged_ids(notice: &Notice) -> (&[String], &[String]) {
    match &notice.change {
        NoticeChange::Merged {
            run_ids, fixed_ids, ..
        } => (run_ids, fixed_ids),
        other => panic!("not a merge notice: {other:?}"),
    }
}

// ---- staging writes draft rows but not schedule, history or journal --------

#[tokio::test]
async fn staging_and_rebase_write_draft_rows_but_not_schedule_or_journal() {
    let mut f = fixture().await;
    let seq = head_seq(&f.service).await;
    let revision = snapshot(&f.service).await.revision;
    journal_is_empty(&f.service).await;

    let create = Some("create-1".to_owned());
    let draft = f
        .service
        .create_draft("root", "retime", create.clone())
        .await
        .unwrap();
    assert_eq!(draft.version, 1);
    // An exact create retries the draft.
    let replayed = f.service.create_draft("root", "retime", create.clone());
    assert_eq!(replayed.await.unwrap().id, draft.id);
    assert!(matches!(
        f.service.create_draft("root", "other", create).await,
        Err(DraftError::RequestMismatch { .. })
    ));
    assert!(matches!(
        f.service.create_draft("root", "", None).await,
        Err(DraftError::InvalidTitle)
    ));

    let run = f.runs[0][0].clone();
    let loaded = add(&mut f.service, &draft.id, 1, amend(&run, kl(1, 20, 0)));
    assert_eq!(loaded.await.unwrap().draft.version, 2);
    let loaded = edit(&mut f.service, &draft.id, 2, 0, amend(&run, kl(2, 20, 0)));
    assert_eq!(loaded.await.unwrap().draft.version, 3);
    // A preview-id target is refused.
    let op = rsvp("preview-1", "1001", RsvpState::Yes);
    assert!(matches!(
        add(&mut f.service, &draft.id, 3, op).await,
        Err(DraftError::ReplayFailed { ord: 1, .. })
    ));
    let preview = preview(&f.service, &draft.id).await.unwrap();
    assert!(preview.is_clean(), "{:?}", preview.conflicts);
    let rebased = rebase(&mut f.service, &draft.id, 3).await.unwrap();
    assert_eq!(rebased.version, 4);
    let discarded = f
        .service
        .discard_draft("root", &draft.id, 4, Some("stale".into()))
        .await
        .unwrap();
    assert_eq!(discarded.status, DraftStatus::Discarded);
    assert!(matches!(
        add(&mut f.service, &draft.id, 4, amend(&run, kl(1, 20, 0))).await,
        Err(DraftError::Stale { .. })
    ));

    assert_eq!(
        head_seq(&f.service).await,
        seq,
        "no change record was appended"
    );
    assert_eq!(snapshot(&f.service).await.revision, revision);
    journal_is_empty(&f.service).await;
    let events = f.service.store().draft_events(&draft.id).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [
            DraftEventKind::Created,
            DraftEventKind::OpAdded,
            DraftEventKind::OpEdited,
            DraftEventKind::Rebased,
            DraftEventKind::Discarded,
        ]
    );
}

// ---- merge ------------------------------------------------------------------

#[tokio::test]
async fn merge_posts_one_notice_per_channel_and_advances_history() {
    let mut f = fixture().await;
    let base_seq = head_seq(&f.service).await;
    let ops = [
        amend(&f.runs[0][0], kl(1, 20, 0)),
        swap(&f.runs[1][2], &["1002"], &["1003"]),
    ];
    let id = stage(&mut f.service, &ops).await;
    let base = loaded(&f.service, &id).await.draft.base;
    let outcome = merge_as(&mut f.service, &id, 3, Some("merge-a"))
        .await
        .unwrap();
    assert_eq!(outcome.seq, base_seq + 1);
    assert_eq!(outcome.notices.len(), 2, "one summary notice per channel");
    assert_eq!(
        merged_channels(&outcome),
        BTreeSet::from([Some("222".into()), Some("333".into())])
    );
    for notice in &outcome.notices {
        assert_eq!(notice.effect_kind(), DRAFT_MERGED);
        let settings = DeliverySettings {
            post_channel_id: None,
            quiet_mode: false,
        };
        let intent = plan_notice(notice, &Guild, &AnyChannel, settings).expect("plannable");
        assert_eq!(Some(intent.channel_id.clone()), notice.channel_id);
    }
    assert!(!outcome.weeks.is_empty());

    let store = f.service.store();
    let record = store.load_change(outcome.seq).await.unwrap().unwrap();
    assert_eq!(record.origin.surface, Surface::DraftMerge);
    assert_eq!(record.origin.request_id.as_deref(), Some("merge-a"));
    assert_eq!(record.refs, [base]);
    assert!(
        store.verify_history().await.unwrap().is_intact(),
        "the merged record verifies"
    );
    // Blame names the merge for the fields it set.
    let blamed = blame(store, &BlameTarget::Run(f.runs[0][0].clone()))
        .await
        .unwrap()
        .expect("blamed");
    let slot = blamed.lines.iter().find(|line| line.field == "slot");
    let slot = slot.expect("slot").last.as_ref().expect("attributed");
    assert_eq!(slot.seq, outcome.seq);
    let loaded = loaded(&f.service, &id).await;
    assert_eq!(loaded.draft.status, DraftStatus::Merged);
    assert_eq!(loaded.draft.merged_seq, Some(outcome.seq));
}

#[tokio::test]
async fn merge_notices_name_rsvp_runs_and_both_sides_of_a_move() {
    let mut f = fixture().await;
    // An RSVP-only change names its run's channel with the run listed.
    let id = stage(
        &mut f.service,
        &[rsvp(&f.runs[1][2], "1001", RsvpState::Yes)],
    )
    .await;
    let outcome = merge(&mut f.service, &id, 2).await.unwrap();
    assert_eq!(outcome.notices.len(), 1);
    assert_eq!(outcome.notices[0].channel_id.as_deref(), Some("333"));
    let (run_ids, fixed_ids) = merged_ids(&outcome.notices[0]);
    assert_eq!(run_ids, &[f.runs[1][2].clone()]);
    assert!(fixed_ids.is_empty());

    // Moving a run to another channel notifies both channels.
    let id = stage(
        &mut f.service,
        &[DraftOp::ApplyFixedEdit {
            fixed: existing(&f.fixed[0]),
            edit: FixedEdit {
                channel_id: Some("333".into()),
                ..FixedEdit::default()
            },
            choices: FixedEditChoices::UpdateAll,
        }],
    )
    .await;
    let outcome = merge(&mut f.service, &id, 2).await.unwrap();
    assert_eq!(
        merged_channels(&outcome),
        BTreeSet::from([Some("222".into()), Some("333".into())])
    );
    for notice in &outcome.notices {
        let (run_ids, _) = merged_ids(notice);
        assert!(
            run_ids.contains(&f.runs[0][0]),
            "the moved run is named in both summaries: {run_ids:?}"
        );
    }
}

#[tokio::test]
async fn a_merge_that_changes_nothing_is_no_effect() {
    let mut f = fixture().await;
    // A swap that changes nobody is a quiet no-op that still stages.
    let id = stage(&mut f.service, &[swap(&f.runs[0][0], &[], &[])]).await;
    let preview = preview(&f.service, &id).await.unwrap();
    assert!(preview.is_clean(), "{:?}", preview.conflicts);
    let seq = head_seq(&f.service).await;
    assert!(matches!(
        merge(&mut f.service, &id, 2).await,
        Err(DraftError::NoEffect)
    ));
    assert_eq!(head_seq(&f.service).await, seq, "nothing was written");
}

#[tokio::test]
async fn request_drafts_are_refused_by_the_admin_methods() {
    fn refused<T>(result: DraftResult<T>) -> bool {
        matches!(result, Err(DraftError::RequestDraft))
    }
    let mut f = fixture().await;
    // Member requests arrive through S3; stage one directly at the store.
    let base = f.service.store().history_head().await.unwrap();
    let revision = snapshot(&f.service).await.revision;
    let created = f
        .service
        .store()
        .create_draft(NewDraft {
            id: "draft-req".into(),
            kind: DraftKind::Request,
            title: "member request".into(),
            author: Actor::member("1001"),
            base,
            base_revision: revision,
            request_type: Some("swap".into()),
            subject: Some(f.runs[0][0].clone()),
            at: now(),
            request: None,
            submit: None,
        })
        .await
        .unwrap();
    let DraftCreated::Created(draft) = created else {
        panic!("not created")
    };
    let (id, run) = (draft.id.as_str(), f.runs[0][0].clone());
    assert!(refused(
        add(&mut f.service, id, 1, amend(&run, kl(1, 20, 0))).await
    ));
    assert!(refused(preview(&f.service, id).await));
    assert!(refused(rebase(&mut f.service, id, 1).await));
    assert!(refused(f.service.discard_draft("root", id, 1, None).await));
    assert!(refused(merge(&mut f.service, id, 1).await));
}

#[tokio::test]
async fn a_stale_base_conflicts_and_writes_nothing() {
    let mut f = fixture().await;
    let seq = head_seq(&f.service).await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    // Upstream moves the same run elsewhere.
    upstream_amend(&mut f.service, &f.runs[0][0], kl(2, 20, 0)).await;
    assert!(!preview(&f.service, &id).await.unwrap().conflicts.is_empty());
    assert!(matches!(
        merge(&mut f.service, &id, 2).await,
        Err(DraftError::Conflicts(_))
    ));
    assert_eq!(
        head_seq(&f.service).await,
        seq + 1,
        "only the upstream write landed"
    );
    // A non-overlapping upstream change still merges clean.
    let clean = stage(
        &mut f.service,
        &[rsvp(&f.runs[1][2], "1001", RsvpState::Yes)],
    )
    .await;
    let outcome = merge(&mut f.service, &clean, 2).await.unwrap();
    assert_eq!(outcome.seq, seq + 2);
}

#[tokio::test]
async fn a_second_merger_gets_already_merged_and_a_retry_replays() {
    let mut f = fixture().await;
    let id = stage(&mut f.service, &[fixed_note(&f.fixed[0], "draft")]).await;
    let first = merge_as(&mut f.service, &id, 2, Some("merge-a"))
        .await
        .unwrap();
    assert!(matches!(
        merge_as(&mut f.service, &id, 3, Some("merge-b")).await,
        Err(DraftError::AlreadyMerged { seq }) if seq == first.seq
    ));
    // The exact retry (same arguments) replays instead.
    assert!(matches!(
        merge_as(&mut f.service, &id, 2, Some("merge-a")).await,
        Err(DraftError::AlreadyApplied { seq, .. }) if seq == first.seq
    ));
    // A reused merge id for another draft is a mismatch.
    let other = stage(&mut f.service, &[fixed_note(&f.fixed[1], "other")]).await;
    assert!(matches!(
        merge_as(&mut f.service, &other, 2, Some("merge-a")).await,
        Err(DraftError::IdempotencyMismatch { seq }) if seq == first.seq
    ));
    // A new request id for the merged draft reports the merge, not a replay.
    assert!(matches!(
        merge(&mut f.service, &id, 3).await,
        Err(DraftError::AlreadyMerged { seq }) if seq == first.seq
    ));
}

#[tokio::test]
async fn a_history_gap_is_refused() {
    let mut f = fixture().await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    by_tests(&mut f.service)
        .set_rsvp(&f.runs[1][2], "1001", RsvpState::No, RsvpSource::Chat)
        .await
        .unwrap();
    let tip = f.service.store().history_head().await.unwrap();
    f.service
        .store()
        .temp_dir()
        .raw(&format!(
            "DROP TRIGGER change_log_no_update;
             UPDATE change_log SET body = replace(body, '\"notices\":[]', '\"notices\":[\"forged\"]')
             WHERE seq = {};",
            tip.seq
        ))
        .await
        .expect("tamper");
    assert!(matches!(
        preview(&f.service, &id).await,
        Err(DraftError::HistoryGap(_))
    ));
    assert!(matches!(
        merge(&mut f.service, &id, 2).await,
        Err(DraftError::HistoryGap(_))
    ));
}

// ---- op edits -----------------------------------------------------------------

#[tokio::test]
async fn removing_an_operation_renumbers_or_refuses() {
    let mut f = fixture().await;
    let week = utc(kl(27, 0, 0));
    // [add timing, create its run, answer on it]: removing the creation
    // leaves the answer dangling.
    let answer = DraftOp::SetRsvp {
        run: Target::Created(1),
        user_id: "1001".into(),
        state: RsvpState::Yes,
        source: RsvpSource::Chat,
    };
    let ops = [
        kalos_timing(),
        kalos_run(Some(Target::Created(0)), week),
        answer,
    ];
    let id = stage(&mut f.service, &ops).await;
    assert!(matches!(
        remove(&mut f.service, &id, 4, 0).await,
        Err(DraftError::EditRefused(_))
    ));
    // [amend an existing run, add a timing, attach to it]: removing the
    // amendment renumbers the attachment from 1 to 0.
    let ops = [
        amend(&f.runs[0][0], kl(1, 20, 0)),
        kalos_timing(),
        kalos_run(Some(Target::Created(1)), week),
    ];
    let id = stage(&mut f.service, &ops).await;
    let loaded = remove(&mut f.service, &id, 4, 0).await.unwrap();
    assert_eq!(loaded.ops.len(), 2);
    assert!(matches!(
        loaded.ops[1].op,
        DraftOp::CreateRun {
            fixed: Some(Target::Created(0)),
            ..
        }
    ));
    // Editing the creation into a non-creating op is refused as well.
    let version = loaded.draft.version;
    let op = swap(&f.runs[0][2], &[], &["1003"]);
    assert!(matches!(
        edit(&mut f.service, &id, version, 0, op).await,
        Err(DraftError::EditRefused(_))
    ));
    assert!(matches!(
        remove(&mut f.service, &id, version, 9).await,
        Err(DraftError::EditRefused(_))
    ));
}

// ---- rebase -------------------------------------------------------------------

#[tokio::test]
async fn rebase_is_refused_while_conflicted_then_succeeds() {
    let mut f = fixture().await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    upstream_amend(&mut f.service, &f.runs[0][0], kl(2, 20, 0)).await;
    assert!(matches!(
        rebase(&mut f.service, &id, 2).await,
        Err(DraftError::Conflicts(_))
    ));
    // Drop the conflicting operation, then rebase onto the head.
    let loaded = remove(&mut f.service, &id, 2, 0).await.unwrap();
    assert!(loaded.ops.is_empty());
    let rebased = rebase(&mut f.service, &id, 3).await.unwrap();
    assert_eq!(rebased.version, 4);
    assert_eq!(
        rebased.base,
        f.service.store().history_head().await.unwrap()
    );
    assert!(matches!(
        merge(&mut f.service, &id, 4).await,
        Err(DraftError::Empty)
    ));
}

/// Settling a moved base: a field both sides moved is taken (the operation
/// goes), an upstream cancellation is kept (the run is revived first); the
/// rest of the draft keeps its edits, and only then does the on-head merge
/// land.
#[tokio::test]
async fn a_settling_rebase_keeps_the_edits_and_lands_on_the_head() {
    let mut f = fixture().await;
    let policy = policy();
    let ops = [
        amend(&f.runs[0][0], kl(1, 20, 0)),
        swap(&f.runs[1][0], &[], &["1003"]),
    ];
    let id = stage(&mut f.service, &ops).await;
    // Another admin moves the same run and cancels the other one.
    upstream_amend(&mut f.service, &f.runs[0][0], kl(2, 20, 0)).await;
    let cancel = StatusChange {
        status: RunStatus::Cancelled,
        announce: false,
        via_portal: false,
    };
    by_tests(&mut f.service)
        .set_status(&f.runs[1][0], cancel, &policy.reminders)
        .await
        .unwrap();
    assert!(matches!(
        f.service
            .merge_draft_on_head("root", &id, 3, &policy, &Guild, None)
            .await,
        Err(DraftError::BaseMoved { .. })
    ));
    let preview = f.service.draft_preview(&id, &policy, &Guild).await.unwrap();
    let keys: BTreeMap<String, Vec<usize>> = preview
        .analysis
        .conflicts
        .iter()
        .map(MergeConflict::key)
        .zip(preview.involved.iter().cloned())
        .collect();
    let slot = format!("both/run:{}/slot", f.runs[0][0]);
    let cancelled = format!("removed/run:{}", f.runs[1][0]);
    assert_eq!(
        keys,
        BTreeMap::from([(slot.clone(), vec![0]), (cancelled.clone(), vec![1])]),
        "each conflict names the operation behind it"
    );
    // Unsettled conflicts refuse and write nothing.
    let partial = BTreeMap::from([(slot.clone(), Settle::Take)]);
    assert!(matches!(
        f.service
            .rebase_draft_settling("root", &id, 3, &partial, &policy, &Guild)
            .await,
        Err(DraftError::Conflicts(left)) if left.len() == 1
    ));
    let settle = BTreeMap::from([(slot, Settle::Take), (cancelled, Settle::Keep)]);
    let rebased = f
        .service
        .rebase_draft_settling("root", &id, 3, &settle, &policy, &Guild)
        .await
        .unwrap();
    assert_eq!(rebased.version, 4);
    assert_eq!(
        rebased.base,
        f.service.store().history_head().await.unwrap()
    );
    assert!(matches!(
        loaded(&f.service, &id).await.ops.as_slice(),
        [
            StagedOp {
                ord: 0,
                op: DraftOp::SetStatus { .. },
                ..
            },
            StagedOp {
                ord: 1,
                op: DraftOp::SwapParticipants { .. },
                ..
            },
        ]
    ));
    f.service
        .merge_draft_on_head("root", &id, 4, &policy, &Guild, None)
        .await
        .unwrap();
    let state = snapshot(&f.service).await;
    let run = |id: &str| state.runs.iter().find(|run| run.id == id).unwrap().clone();
    assert_eq!(
        run(&f.runs[0][0]).datetime,
        utc(kl(2, 20, 0)),
        "upstream's time"
    );
    let revived = run(&f.runs[1][0]);
    assert_eq!(revived.status, RunStatus::Planned);
    assert!(revived.participants.contains(&"1003".to_owned()));
}

// ---- expiry -------------------------------------------------------------------

async fn scope_of(service: &Service, id: &str) -> DraftScope {
    loaded(service, id).await.draft.scope
}

fn week_of(state: &ScheduleSnapshot, id: &str) -> DateTime<Utc> {
    state
        .runs
        .iter()
        .find(|run| run.id == id)
        .unwrap()
        .week_start
}

#[tokio::test]
async fn the_scope_is_derived_from_the_staged_run_ops() {
    let mut f = fixture().await;
    let draft = f
        .service
        .create_draft("root", "retime", None)
        .await
        .unwrap();
    assert_eq!(
        scope_of(&f.service, &draft.id).await,
        DraftScope::Weekly,
        "a fresh draft has no expiry"
    );
    let week = week_of(&snapshot(&f.service).await, &f.runs[0][0]);
    // Staging a run op sets the earliest touched week.
    let op = amend(&f.runs[0][0], kl(1, 20, 0));
    let loaded = add(&mut f.service, &draft.id, 1, op).await.unwrap();
    assert_eq!(loaded.draft.scope, DraftScope::Week(week));
    // A fixed-only op leaves the scope alone.
    let op = fixed_note(&f.fixed[0], "prog");
    let loaded = add(&mut f.service, &draft.id, 2, op).await.unwrap();
    assert_eq!(loaded.draft.scope, DraftScope::Week(week));
    // Removing the only run op clears the expiry.
    let loaded = remove(&mut f.service, &draft.id, 3, 0).await.unwrap();
    assert_eq!(loaded.draft.scope, DraftScope::Weekly);
    // The fixed-only draft merges after any number of resets.
    f.service.clock().set(kl(20, 1, 0));
    let outcome = merge(&mut f.service, &draft.id, loaded.draft.version)
        .await
        .unwrap();
    // The merge also commits the weeks materialised since, but only the
    // edited timing's channel hears about it: the other timing's new runs
    // are routine and quiet.
    assert_eq!(outcome.notices.len(), 1);
    assert_eq!(outcome.notices[0].channel_id.as_deref(), Some("222"));
    let state = snapshot(&f.service).await;
    let timing_of = |id: &str| {
        let run = state.runs.iter().find(|run| run.id == id);
        run.and_then(|run| run.fixed_run_id.clone())
    };
    let (run_ids, fixed_ids) = merged_ids(&outcome.notices[0]);
    assert_eq!(fixed_ids, &[f.fixed[0].clone()]);
    // Like a plain timing edit, the edited timing's channel is told;
    // its newly materialised runs are listed with it.
    assert!(
        run_ids
            .iter()
            .any(|id| !f.runs[0].contains(id) && timing_of(id).as_ref() == Some(&f.fixed[0])),
        "{run_ids:?}"
    );
    assert!(
        run_ids
            .iter()
            .all(|id| timing_of(id).as_ref() == Some(&f.fixed[0])),
        "{run_ids:?}"
    );
}

#[tokio::test]
async fn edits_and_rebases_re_derive_the_scope() {
    let mut f = fixture().await;
    let state = snapshot(&f.service).await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    assert_eq!(
        scope_of(&f.service, &id).await,
        DraftScope::Week(week_of(&state, &f.runs[0][0]))
    );
    // Editing the only run op onto a later week's run moves the expiry.
    let op = rsvp(&f.runs[1][2], "1001", RsvpState::Yes);
    let edited = edit(&mut f.service, &id, 2, 0, op).await.unwrap();
    assert_eq!(
        edited.draft.scope,
        DraftScope::Week(week_of(&state, &f.runs[1][2]))
    );
    // Upstream moves that run a week later; rebasing re-derives on the head.
    upstream_amend(&mut f.service, &f.runs[1][2], kl(22, 21, 30)).await;
    let moved = snapshot(&f.service).await;
    assert!(week_of(&moved, &f.runs[1][2]) > week_of(&state, &f.runs[1][2]));
    let rebased = rebase(&mut f.service, &id, 3).await.unwrap();
    assert_eq!(
        rebased.scope,
        DraftScope::Week(week_of(&moved, &f.runs[1][2]))
    );
}

#[tokio::test]
async fn a_staged_run_must_sit_in_its_slots_week() {
    let mut f = fixture().await;
    let draft = f
        .service
        .create_draft("root", "new run", None)
        .await
        .unwrap();
    let right = utc(kl(27, 0, 0));
    let wrong = right + TimeDelta::days(7);
    assert!(matches!(
        add(&mut f.service, &draft.id, 1, kalos_run(None, wrong)).await,
        Err(DraftError::ReplayFailed {
            ord: 0,
            error: ReplayError::WeekMismatch { week_start, slot_week },
        }) if slot_week == right && week_start == wrong
    ));
    let loaded = add(&mut f.service, &draft.id, 1, kalos_run(None, right));
    assert_eq!(loaded.await.unwrap().draft.scope, DraftScope::Week(right));
}

#[tokio::test]
async fn a_run_op_draft_expires_after_the_reset_and_cannot_merge() {
    let mut f = fixture().await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    // Still its week: the preview is clean.
    let clean = preview(&f.service, &id).await.unwrap();
    assert!(clean.is_clean(), "{:?}", clean.conflicts);
    // Past the reset the tick expiry closes it.
    f.service.clock().set(kl(3, 1, 0));
    let expired = f.service.expire_due_drafts(&policy()).await.unwrap();
    assert_eq!(expired.ids, std::slice::from_ref(&id));
    assert!(expired.notices.is_empty(), "an admin draft notifies nobody");
    let loaded = loaded(&f.service, &id).await;
    assert_eq!(loaded.draft.status, DraftStatus::Expired);
    assert_eq!(loaded.draft.version, 2, "expiry does not bump the version");
    assert!(matches!(
        preview(&f.service, &id).await,
        Err(DraftError::Expired)
    ));
    assert!(matches!(
        merge(&mut f.service, &id, 2).await,
        Err(DraftError::Expired)
    ));
}

#[tokio::test]
async fn merge_marks_an_expired_draft_when_the_tick_did_not() {
    let mut f = fixture().await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    // Straight past the reset without expiring first.
    f.service.clock().set(kl(3, 1, 0));
    assert!(matches!(
        merge(&mut f.service, &id, 2).await,
        Err(DraftError::Expired)
    ));
    let loaded = loaded(&f.service, &id).await;
    assert_eq!(loaded.draft.status, DraftStatus::Expired);
    // Closed by the system expiry actor, as the tick would.
    let system = Actor::system("delivery");
    assert_eq!(loaded.draft.closed_by.as_ref(), Some(&system));
    let events = f.service.store().draft_events(&id).await.unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.kind, DraftEventKind::Expired);
    assert_eq!(last.actor, system);
    // Idempotent: merging again reports expiry, not staleness.
    assert!(matches!(
        merge(&mut f.service, &id, 2).await,
        Err(DraftError::Expired)
    ));
    // So does previewing.
    assert!(matches!(
        preview(&f.service, &id).await,
        Err(DraftError::Expired)
    ));
}

#[tokio::test]
async fn an_upstream_move_into_a_past_week_expires_the_draft_at_merge() {
    let mut f = fixture().await;
    let id = stage(
        &mut f.service,
        &[rsvp(&f.runs[1][2], "1001", RsvpState::Yes)],
    )
    .await;
    // The stored scope is the run's week; upstream then moves the run into
    // last week (26 Aug 20:00 +08:00, before the 27 Aug reset).
    let past = FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 8, 26, 20, 0, 0)
        .unwrap();
    upstream_amend(&mut f.service, &f.runs[1][2], past).await;
    let seq = head_seq(&f.service).await;
    assert!(matches!(
        scope_of(&f.service, &id).await,
        DraftScope::Week(week) if week > utc(past)
    ));
    assert!(matches!(
        preview(&f.service, &id).await,
        Err(DraftError::Expired)
    ));
    assert!(matches!(
        merge(&mut f.service, &id, 2).await,
        Err(DraftError::Expired)
    ));
    assert_eq!(head_seq(&f.service).await, seq, "nothing was merged");
    let loaded = loaded(&f.service, &id).await;
    assert_eq!(loaded.draft.status, DraftStatus::Expired);
    assert_eq!(loaded.draft.closed_by, Some(Actor::system("delivery")));
}

// ---- revision race --------------------------------------------------------------

/// A store failing the first merge commit with a revision conflict.
struct Racy<'a> {
    inner: &'a SqliteStore,
    attempts: AtomicUsize,
    race: Race,
}

enum Race {
    /// A bare conflict: the retry re-analyses the same schedule and commits.
    FakeConflict,
    /// A conflicting upstream commit lands first: the retry re-analyses and
    /// reports the conflict instead of committing.
    RealConflict { run: String, to: DateTime<Utc> },
    /// The tick expires the draft at the reset edge, between the merge's
    /// expiry check and its commit.
    Expire,
}

impl ScheduleStore for Racy<'_> {
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
        self.inner.commit(expected_revision, changes, meta).await
    }
}

impl ChangeHistory for Racy<'_> {
    async fn load_change(&self, seq: u64) -> Result<Option<ChangeRecord>, StoreError> {
        self.inner.load_change(seq).await
    }

    async fn load_checked(&self, seq: u64) -> Result<CheckedChange, StoreError> {
        self.inner.load_checked(seq).await
    }

    async fn list_changes(&self, query: &ChangeQuery) -> Result<ChangePage, StoreError> {
        self.inner.list_changes(query).await
    }

    async fn count_changes(&self, filter: &ChangeFilter) -> Result<u64, StoreError> {
        self.inner.count_changes(filter).await
    }

    async fn verify_history(&self) -> Result<HistoryVerification, StoreError> {
        self.inner.verify_history().await
    }

    async fn history_head(&self) -> Result<ChangeRef, StoreError> {
        self.inner.history_head().await
    }
}

impl DraftStore for Racy<'_> {
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

    async fn list_drafts(
        &self,
        status: Option<DraftStatus>,
    ) -> Result<Vec<StoredDraft>, StoreError> {
        self.inner.list_drafts(status).await
    }

    async fn recorded_draft_request(
        &self,
        author: &Actor,
        request_id: &str,
    ) -> Result<Option<(String, StoredDraft)>, StoreError> {
        self.inner.recorded_draft_request(author, request_id).await
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
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            match &self.race {
                Race::FakeConflict => {
                    return Err(StoreError::Conflict {
                        expected: expected_revision,
                        found: expected_revision + 1,
                    });
                }
                Race::RealConflict { run, to } => {
                    // A conflicting upstream commit lands between the merge's
                    // analysis and its commit.
                    let snapshot = self.inner.load(&Scope::All).await?;
                    let mut moved = snapshot
                        .runs
                        .iter()
                        .find(|row| row.id == *run)
                        .expect("run")
                        .clone();
                    moved.datetime = *to;
                    let upstream = ChangeMeta {
                        origin: Origin::new(Actor::member("2"), Surface::Discord),
                        at: now(),
                        notices: Vec::new(),
                        refs: Vec::new(),
                        request_digest: None,
                        expect: Default::default(),
                        outbox: Vec::new(),
                    };
                    let changes = ChangeSet {
                        changes: vec![Change::PutRun(moved)],
                    };
                    self.inner
                        .commit(snapshot.revision, changes, upstream)
                        .await
                        .expect("upstream commit");
                }
                Race::Expire => {
                    let far = now() + TimeDelta::days(365);
                    let actor = Actor::system("delivery");
                    self.inner
                        .expire_drafts(far, now(), &actor, Vec::new())
                        .await
                        .expect("expire");
                }
            }
        }
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
        notices: Vec<(String, Notice)>,
    ) -> Result<Vec<String>, StoreError> {
        self.inner.expire_drafts(week, at, actor, notices).await
    }
}

/// Stage one amendment through a `race` store over the fixture's and merge
/// it: the merge result, the commit attempts and the head before staging.
async fn race_merge(f: &Fixture, race: Race) -> (DraftResult<MergeOutcome>, usize, ChangeRef) {
    let racy = Racy {
        inner: f.service.store(),
        attempts: AtomicUsize::new(0),
        race,
    };
    let mut service = SchedulerService::new(racy, RandomIds, TestClock::new(kl(27, 1, 0)));
    let base = service.store().history_head().await.unwrap();
    let draft = service.create_draft("root", "retime", None).await.unwrap();
    assert_eq!(draft.base, base);
    let op = amend(&f.runs[0][0], kl(1, 20, 0));
    let loaded = add(&mut service, &draft.id, 1, op).await.unwrap();
    let merged = merge_as(&mut service, &draft.id, loaded.draft.version, None).await;
    (
        merged,
        service.store().attempts.load(Ordering::SeqCst),
        base,
    )
}

#[tokio::test]
async fn a_revision_race_is_re_analysed_and_retried() {
    let f = fixture().await;
    let (merged, attempts, base) = race_merge(&f, Race::FakeConflict).await;
    let outcome = merged.unwrap();
    assert_eq!(outcome.notices.len(), 1);
    assert_eq!(outcome.seq, base.seq + 1);
    // One failed commit plus the retry.
    assert_eq!(attempts, 2);
    let tip = f.service.store().history_head().await.unwrap();
    assert_eq!(tip.seq, outcome.seq);
}

#[tokio::test]
async fn a_conflicting_race_is_re_analysed_into_conflicts() {
    let f = fixture().await;
    let race = Race::RealConflict {
        run: f.runs[0][0].clone(),
        to: utc(kl(2, 20, 0)),
    };
    let (merged, attempts, base) = race_merge(&f, race).await;
    // The first commit raced a real conflicting upstream move; the retry
    // re-analysed and reported the conflict instead of committing.
    assert!(matches!(
        merged,
        Err(DraftError::Conflicts(conflicts))
            if conflicts.iter().any(|conflict| matches!(
                conflict,
                MergeConflict::BothChanged { field: Field::Slot, .. }
            ))
    ));
    // The merge committed nothing itself; only the upstream commit landed.
    assert_eq!(attempts, 1);
    let tip = f.service.store().history_head().await.unwrap();
    assert_eq!(tip.seq, base.seq + 1, "only the upstream commit landed");
}

#[tokio::test]
async fn a_draft_expired_between_check_and_commit_reports_expired() {
    let f = fixture().await;
    let (merged, attempts, base) = race_merge(&f, Race::Expire).await;
    assert!(matches!(merged, Err(DraftError::Expired)));
    assert_eq!(attempts, 1);
    let tip = f.service.store().history_head().await.unwrap();
    assert_eq!(tip, base, "nothing was merged");
}
