//! Edit preconditions every store must enforce inside the commit
//! transaction, driven through [`SchedulerService`] by two editors sharing a
//! fresh store per check. Failures panic with the check name.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveTime, TimeDelta, TimeZone, Utc, Weekday};

use crate::domain::history::{
    Actor, BlameIndex, BlameTarget, ChangeHistory, ChangeMeta, ChangeRef, EDIT_OVERRIDE, Expect,
    Origin, Precondition, PreconditionError, RowValue, Surface, Via, blame, rsvp_field,
};
use crate::domain::ids::IdGenerator;
use crate::domain::schedule::{
    Change, ChangeSet, FixedEdit, NewFixedRun, ReminderPolicy, RsvpSource, RsvpState, RunStatus,
    SchedulePolicy, ScheduleSnapshot, StatusChange,
};
use crate::domain::scheduler::{
    Clock, Committed, RecordedRequest, ScheduleStore, SchedulerError, SchedulerService, Scope,
    StoreError,
};

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<S: ScheduleStore + ChangeHistory + BlameIndex + Sync>(
    make: impl AsyncFn() -> S,
) {
    two_editors_on_one_field_second_is_stale(make().await).await;
    different_fields_of_one_run_both_apply(make().await).await;
    an_upstream_commit_between_plan_and_commit_is_stale(make().await).await;
    an_admin_override_records_and_blames_the_overridden(make().await).await;
    overrides_are_refused_for_members_and_unknown_changes(make().await).await;
    members_own_rsvp_and_move_carry_preconditions(make().await).await;
    retired_timings_and_cancelled_runs_are_stale(make().await).await;
    retries_replay_and_mismatch_by_expectation(make().await).await;
    unknown_fields_are_refused_by_service_and_store(make().await).await;
    no_op_edits_report_only_deleted_targets(make().await).await;
    versioned_reads_carry_rows_and_versions(make().await).await;
    overrides_must_change_their_field_and_be_admin(make().await).await;
    never_existing_targets_are_unknown(make().await).await;
    expectations_survive_a_revision_conflict(make().await).await;
    cleared_rsvp_keeps_blame_version(make().await).await;
}

async fn cleared_rsvp_keeps_blame_version<S: ScheduleStore + ChangeHistory + BlameIndex + Sync>(
    store: S,
) {
    let (_fixed, run) = seed(&store).await;
    let target = BlameTarget::Run(run.clone());
    editor(&store)
        .as_origin(admin("answer"))
        .set_rsvp(&run, "1", RsvpState::Yes, RsvpSource::Chat)
        .await
        .expect("answer");
    editor(&store)
        .as_origin(admin("clear"))
        .portal_answer(&run, "1", None)
        .await
        .expect("clear");
    let version = store
        .read_versioned(std::slice::from_ref(&target))
        .await
        .expect("versioned")
        .pop()
        .expect("run");
    assert!(
        version
            .answers
            .iter()
            .all(|row| !matches!(row, RowValue::Rsvp(rsvp) if rsvp.user_id == "1"))
    );
    assert!(
        version.versions.contains_key(&rsvp_field("1")),
        "clear is blamed"
    );
    let field = rsvp_field("1");
    let last = store.last_changes(&target).await.expect("last changes");
    assert_eq!(
        last.get(&field),
        version.versions.get(&field),
        "last_changes and read_versioned retain the same clear version"
    );
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

#[derive(Clone, Copy)]
struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        utc(27, 1)
    }
}

fn utc(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0)
        .single()
        .expect("valid instant")
}

fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::UTC,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).expect("valid time"),
            countdowns: vec![60],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

fn admin(id: &str) -> Origin {
    Origin::new(Actor::admin(id), Surface::AdminPortal)
}

fn member(id: &str) -> Origin {
    Origin::new(Actor::member(id), Surface::PublicPortal)
}

fn status(status: RunStatus) -> StatusChange {
    StatusChange {
        status,
        announce: false,
        via_portal: true,
    }
}

/// An editor: its own service (ids, clock) over the shared store.
fn editor<S: ScheduleStore + Sync>(store: &S) -> SchedulerService<&S, Ids, FixedClock> {
    SchedulerService::new(store, Ids::default(), FixedClock)
}

/// One weekly timing (Sat 20:00, channel 900) with its runs materialised;
/// returns the timing and the first run.
async fn seed<S: ScheduleStore + Sync>(store: &S) -> (String, String) {
    let mut service = editor(store);
    let fixed = service
        .as_origin(admin("seed"))
        .add_fixed_run(NewFixedRun {
            owner_pinned: false,
            owner_id: "1".into(),
            channel_id: Some("900".into()),
            bosses: vec!["HFA".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(20, 0, 0).expect("valid time"),
            participants: vec!["1".into(), "2".into()],
            note: None,
        })
        .await
        .expect("fixed run");
    service
        .as_origin(Origin::new(
            Actor::system("delivery"),
            Surface::DeliveryTick,
        ))
        .materialise_weeks(&policy())
        .await
        .expect("materialise");
    let run = snapshot(store)
        .await
        .runs
        .iter()
        .filter(|run| run.fixed_run_id.as_deref() == Some(fixed.as_str()))
        .min_by_key(|run| run.datetime)
        .expect("materialised run")
        .id
        .clone();
    (fixed, run)
}

async fn snapshot<S: ScheduleStore>(store: &S) -> ScheduleSnapshot {
    store.load(&Scope::All).await.expect("load")
}

async fn head<S: ChangeHistory>(store: &S) -> ChangeRef {
    store.history_head().await.expect("head")
}

/// What a screen shows as a field's version: its last change from blame.
async fn seen<S: BlameIndex>(store: &S, target: &BlameTarget, field: &str) -> Option<u64> {
    store
        .last_changes(target)
        .await
        .expect("last changes")
        .get(field)
        .copied()
}

async fn expect_seen<S: BlameIndex>(store: &S, target: &BlameTarget, field: &str) -> Precondition {
    Precondition::new(target.clone(), field, seen(store, target, field).await)
}

fn stale_fields(result: Result<impl std::fmt::Debug, SchedulerError>, check: &str) -> Vec<String> {
    match result {
        Err(SchedulerError::StaleEdit { conflicts }) => conflicts
            .into_iter()
            .map(|conflict| conflict.field)
            .collect(),
        other => panic!("{check}: expected StaleEdit, got {other:?}"),
    }
}

async fn two_editors_on_one_field_second_is_stale<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (_fixed, run) = seed(&store).await;
    let target = BlameTarget::Run(run.clone());
    // Both screens were read at the same version.
    let view = expect_seen(&store, &target, "slot").await;
    let mut first = editor(&store);
    first
        .as_origin(admin("a"))
        .expecting(Expect::fields([view.clone()]))
        .amend_run(&run, utc(29, 21), &policy())
        .await
        .expect("two_editors: first edit applies");
    let tip = head(&store).await;
    let mut second = editor(&store);
    let result = second
        .as_origin(admin("b"))
        .expecting(Expect::fields([view.clone()]))
        .amend_run(&run, utc(29, 22), &policy())
        .await;
    let Err(SchedulerError::StaleEdit { conflicts }) = result else {
        panic!("two_editors: expected StaleEdit, got {result:?}");
    };
    assert_eq!(conflicts.len(), 1, "two_editors");
    let conflict = &conflicts[0];
    assert_eq!(conflict.target, target, "two_editors");
    assert_eq!(conflict.field, "slot", "two_editors");
    assert_eq!(conflict.expected, view.seen, "two_editors");
    let current = conflict.current.as_ref().expect("two_editors: current");
    assert_eq!(current.change, tip, "two_editors: the newer change");
    assert_eq!(current.actor, Actor::admin("a"), "two_editors");
    assert!(!conflict.target_deleted, "two_editors");
    assert!(
        matches!(&conflict.current_value, Some(RowValue::Run(row)) if row.datetime == utc(29, 21)),
        "two_editors: the value now"
    );
    assert_eq!(head(&store).await, tip, "two_editors: nothing written");
}

async fn different_fields_of_one_run_both_apply<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (_fixed, run) = seed(&store).await;
    let target = BlameTarget::Run(run.clone());
    let slot = expect_seen(&store, &target, "slot").await;
    let state = expect_seen(&store, &target, "status").await;
    editor(&store)
        .as_origin(admin("a"))
        .expecting(Expect::fields([slot]))
        .amend_run(&run, utc(29, 21), &policy())
        .await
        .expect("different_fields: slot");
    editor(&store)
        .as_origin(admin("b"))
        .expecting(Expect::fields([state]))
        .set_status(&run, status(RunStatus::Confirmed), &policy().reminders)
        .await
        .expect("different_fields: status merges with the slot change");
    let row = snapshot(&store)
        .await
        .runs
        .into_iter()
        .find(|row| row.id == run)
        .expect("run");
    assert_eq!(
        (row.datetime, row.status),
        (utc(29, 21), RunStatus::Confirmed),
        "different_fields: both applied"
    );
}

/// Lands a newer slot change on `run` right before the first commit it
/// forwards (after the editor planned).
struct Upstream<'a, S> {
    inner: &'a S,
    run: String,
    fired: AtomicBool,
}

impl<S: ScheduleStore + Sync> ScheduleStore for Upstream<'_, S> {
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
        if !self.fired.swap(true, Ordering::SeqCst) {
            let snapshot = self.inner.load(&Scope::All).await?;
            let mut moved = snapshot
                .runs
                .iter()
                .find(|row| row.id == self.run)
                .expect("upstream: run")
                .clone();
            moved.datetime += TimeDelta::hours(3);
            let upstream = ChangeMeta {
                origin: admin("upstream"),
                at: utc(27, 1),
                notices: Vec::new(),
                refs: Vec::new(),
                request_digest: None,
                expect: Expect::default(),
                outbox: Vec::new(),
            };
            self.inner
                .commit(
                    snapshot.revision,
                    ChangeSet {
                        changes: vec![Change::PutRun(moved)],
                    },
                    upstream,
                )
                .await?;
        }
        self.inner.commit(expected_revision, changes, meta).await
    }
}

async fn an_upstream_commit_between_plan_and_commit_is_stale<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (_fixed, run) = seed(&store).await;
    let view = expect_seen(&store, &BlameTarget::Run(run.clone()), "slot").await;
    let racing = Upstream {
        inner: &store,
        run: run.clone(),
        fired: AtomicBool::new(false),
    };
    let mut service = SchedulerService::new(&racing, Ids::default(), FixedClock);
    let result = service
        .as_origin(admin("b"))
        .expecting(Expect::fields([view]))
        .amend_run(&run, utc(29, 22), &policy())
        .await;
    assert_eq!(stale_fields(result, "upstream_race"), ["slot"]);
    let row = snapshot(&store)
        .await
        .runs
        .into_iter()
        .find(|row| row.id == run)
        .expect("run");
    assert_ne!(row.datetime, utc(29, 22), "upstream_race: no silent win");
}

async fn an_admin_override_records_and_blames_the_overridden<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (_fixed, run) = seed(&store).await;
    let target = BlameTarget::Run(run.clone());
    let view = expect_seen(&store, &target, "slot").await;
    editor(&store)
        .as_origin(admin("a"))
        .amend_run(&run, utc(29, 21), &policy())
        .await
        .expect("override: first edit");
    let theirs = head(&store).await;
    let mut mine = editor(&store);
    let result = mine
        .as_origin(admin("b"))
        .expecting(Expect::fields([view]))
        .amend_run(&run, utc(29, 22), &policy())
        .await;
    let Err(SchedulerError::StaleEdit { conflicts }) = result else {
        panic!("override: expected StaleEdit, got {result:?}");
    };
    // "Apply mine anyway": resubmit at the current versions, naming them.
    let overridden = conflicts[0]
        .current
        .clone()
        .expect("override: current")
        .change;
    assert_eq!(overridden, theirs, "override");
    let resubmit = Expect::fields([Precondition::new(
        target.clone(),
        "slot",
        Some(overridden.seq),
    )])
    .overriding([overridden.clone()]);
    mine.as_origin(admin("b"))
        .expecting(resubmit)
        .amend_run(&run, utc(29, 22), &policy())
        .await
        .expect("override: applies");
    let tip = head(&store).await;
    let record = store
        .load_change(tip.seq)
        .await
        .expect("record")
        .expect("record");
    assert_eq!(
        record.refs,
        std::slice::from_ref(&overridden),
        "override: refs"
    );
    assert!(
        record.notices.iter().any(|kind| kind == EDIT_OVERRIDE),
        "override: marked"
    );
    let blamed = blame(&store, &target).await.expect("blame").expect("run");
    let line = blamed
        .lines
        .iter()
        .find(|line| line.field == "slot")
        .and_then(|line| line.last.clone())
        .expect("override: slot blamed");
    assert_eq!(line.seq, tip.seq, "override");
    assert_eq!(line.actor, Actor::admin("b"), "override");
    assert_eq!(
        line.via,
        Via::Override {
            picked: None,
            overridden: vec![overridden]
        },
        "override: via"
    );
}

async fn overrides_are_refused_for_members_and_unknown_changes<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (_fixed, run) = seed(&store).await;
    let target = BlameTarget::Run(run.clone());
    let answer = rsvp_field("1");
    editor(&store)
        .as_origin(admin("a"))
        .set_rsvp(&run, "1", RsvpState::Yes, RsvpSource::Chat)
        .await
        .expect("refusals: upstream answer");
    let theirs = head(&store).await;
    let overriding = || {
        Expect::fields([Precondition::new(
            target.clone(),
            answer.clone(),
            Some(theirs.seq),
        )])
        .overriding([theirs.clone()])
    };
    for origin in [
        member("1"),
        Origin::new(Actor::system("delivery"), Surface::DeliveryTick),
    ] {
        let result = editor(&store)
            .as_origin(origin)
            .expecting(overriding())
            .set_rsvp(&run, "1", RsvpState::No, RsvpSource::Chat)
            .await;
        assert!(
            matches!(
                result,
                Err(SchedulerError::Precondition(
                    PreconditionError::OverrideNotAdmin
                ))
            ),
            "refusals: {result:?}"
        );
    }
    // An override must name a recorded change with its hash (store check).
    let mut forged = theirs.clone();
    forged.hash = "f".repeat(64);
    let result = editor(&store)
        .as_origin(admin("b"))
        .expecting(
            Expect::fields([Precondition::new(target.clone(), answer, Some(theirs.seq))])
                .overriding([forged]),
        )
        .set_rsvp(&run, "1", RsvpState::No, RsvpSource::Chat)
        .await;
    assert!(
        matches!(
            result,
            Err(SchedulerError::Precondition(PreconditionError::UnknownOverride { seq }))
                if seq == theirs.seq
        ),
        "refusals: {result:?}"
    );
    assert_eq!(head(&store).await, theirs, "refusals: nothing written");
}

async fn members_own_rsvp_and_move_carry_preconditions<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (_fixed, run) = seed(&store).await;
    let target = BlameTarget::Run(run.clone());
    // No answer yet: the screen saw no change for it.
    let unanswered = expect_seen(&store, &target, &rsvp_field("1")).await;
    assert_eq!(unanswered.seen, None, "member");
    editor(&store)
        .as_origin(member("1"))
        .expecting(Expect::fields([unanswered.clone()]))
        .set_rsvp(&run, "1", RsvpState::Yes, RsvpSource::Chat)
        .await
        .expect("member: first answer");
    // A second tab still showing "no answer" is stale.
    let result = editor(&store)
        .as_origin(member("1"))
        .expecting(Expect::fields([unanswered]))
        .set_rsvp(&run, "1", RsvpState::No, RsvpSource::Chat)
        .await;
    assert_eq!(stale_fields(result, "member_rsvp"), [rsvp_field("1")]);
    // Moving their own run this boss week, from a fresh view.
    let slot = expect_seen(&store, &target, "slot").await;
    editor(&store)
        .as_origin(member("1"))
        .expecting(Expect::fields([slot.clone()]))
        .amend_run(&run, utc(29, 21), &policy())
        .await
        .expect("member: move");
    let result = editor(&store)
        .as_origin(member("1"))
        .expecting(Expect::fields([slot]))
        .amend_run(&run, utc(29, 22), &policy())
        .await;
    assert_eq!(stale_fields(result, "member_move"), ["slot"]);
}

async fn retired_timings_and_cancelled_runs_are_stale<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (fixed, run) = seed(&store).await;
    let timing = BlameTarget::FixedRun(fixed.clone());
    let target = BlameTarget::Run(run.clone());
    let note = expect_seen(&store, &timing, "note").await;
    let state = expect_seen(&store, &target, "status").await;
    let slot = expect_seen(&store, &target, "slot").await;
    let weeks: Vec<DateTime<Utc>> = {
        let mut weeks: Vec<_> = snapshot(&store)
            .await
            .runs
            .iter()
            .map(|row| row.week_start)
            .collect();
        weeks.dedup();
        weeks
    };
    editor(&store)
        .as_origin(admin("a"))
        .retire_fixed_run(&fixed, &weeks, &policy().reminders)
        .await
        .expect("retired: retire");
    let tip = head(&store).await;
    // Editing the deleted timing reports its row gone, not a rule refusal.
    let result = editor(&store)
        .as_origin(admin("b"))
        .expecting(Expect::fields([note]))
        .update_fixed(
            &fixed,
            FixedEdit {
                note: Some("late".into()),
                ..FixedEdit::default()
            },
            &crate::domain::members::Roster::new(),
            &policy(),
        )
        .await;
    let Err(SchedulerError::StaleEdit { conflicts }) = result else {
        panic!("retired: expected StaleEdit, got {result:?}");
    };
    assert!(conflicts[0].target_deleted, "retired: target gone");
    assert_eq!(conflicts[0].current_value, None, "retired");
    // The cancelled run: a status edit from before is stale, with the value.
    let result = editor(&store)
        .as_origin(admin("b"))
        .expecting(Expect::fields([state]))
        .set_status(&run, status(RunStatus::Confirmed), &policy().reminders)
        .await;
    let Err(SchedulerError::StaleEdit { conflicts }) = result else {
        panic!("cancelled: expected StaleEdit, got {result:?}");
    };
    assert!(
        matches!(&conflicts[0].current_value, Some(RowValue::Run(row)) if row.status == RunStatus::Cancelled),
        "cancelled: {conflicts:?}"
    );
    assert_eq!(head(&store).await, tip, "retired: nothing written");
    // Only declared fields are checked: a slot-only edit still applies.
    editor(&store)
        .as_origin(admin("b"))
        .expecting(Expect::fields([slot]))
        .amend_run(&run, utc(29, 21), &policy())
        .await
        .expect("cancelled: undeclared status is not checked");
}

async fn retries_replay_and_mismatch_by_expectation<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (_fixed, run) = seed(&store).await;
    let target = BlameTarget::Run(run.clone());
    let view = expect_seen(&store, &target, "slot").await;
    let edit = |expect: Expect| {
        let store = &store;
        let run = run.clone();
        async move {
            editor(store)
                .as_origin(admin("b").with_request_id("edit-1"))
                .expecting(expect)
                .amend_run(&run, utc(29, 21), &policy())
                .await
        }
    };
    edit(Expect::fields([view.clone()]))
        .await
        .expect("retries: applies");
    let tip = head(&store).await;
    assert!(
        matches!(
            edit(Expect::fields([view.clone()])).await,
            Err(SchedulerError::AlreadyApplied { seq, .. }) if seq == tip.seq
        ),
        "retries: exact retry"
    );
    // Two fields declared in either order are the same request.
    let status_view = expect_seen(&store, &target, "status").await;
    let tip_before = head(&store).await;
    let pair = |first: Precondition, second: Precondition| async {
        editor(&store)
            .as_origin(admin("b").with_request_id("edit-2"))
            .expecting(Expect::fields([first, second]))
            .set_status(&run, status(RunStatus::Confirmed), &policy().reminders)
            .await
    };
    let slot_now = expect_seen(&store, &target, "slot").await;
    pair(slot_now.clone(), status_view.clone())
        .await
        .expect("retries: two fields");
    let paired = head(&store).await;
    assert_eq!(paired.seq, tip_before.seq + 1, "retries");
    assert!(
        matches!(
            pair(status_view, slot_now).await,
            Err(SchedulerError::AlreadyApplied { seq, .. }) if seq == paired.seq
        ),
        "retries: reordered fields replay"
    );
    let other = Precondition::new(target, "slot", Some(paired.seq));
    assert!(
        matches!(
            edit(Expect::fields([other])).await,
            Err(SchedulerError::IdempotencyMismatch { seq }) if seq == tip.seq
        ),
        "retries: other expectations"
    );
    // The same arguments without expectations are another request too.
    assert!(
        matches!(
            edit(Expect::default()).await,
            Err(SchedulerError::IdempotencyMismatch { .. })
        ),
        "retries: no expectations"
    );
    assert_eq!(head(&store).await, paired, "retries: each applied once");
}

async fn unknown_fields_are_refused_by_service_and_store<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (_fixed, run) = seed(&store).await;
    let bad = Expect::fields([Precondition::new(
        BlameTarget::Run(run.clone()),
        "reminders",
        None,
    )]);
    let result = editor(&store)
        .as_origin(admin("b"))
        .expecting(bad.clone())
        .set_status(&run, status(RunStatus::Confirmed), &policy().reminders)
        .await;
    assert!(
        matches!(
            result,
            Err(SchedulerError::Precondition(
                PreconditionError::UnknownField { .. }
            ))
        ),
        "unknown_field: service {result:?}"
    );
    let revision = snapshot(&store).await.revision;
    let meta = ChangeMeta {
        origin: admin("b"),
        at: utc(27, 1),
        notices: Vec::new(),
        refs: Vec::new(),
        request_digest: None,
        expect: bad,
        outbox: Vec::new(),
    };
    let result = store.commit(revision, ChangeSet::default(), meta).await;
    assert!(
        matches!(
            result,
            Err(StoreError::Precondition(
                PreconditionError::UnknownField { .. }
            ))
        ),
        "unknown_field: store {result:?}"
    );
}

async fn no_op_edits_report_only_deleted_targets<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (fixed, run) = seed(&store).await;
    let timing = BlameTarget::FixedRun(fixed.clone());
    let target = BlameTarget::Run(run.clone());
    let note = expect_seen(&store, &timing, "note").await;
    let slot = expect_seen(&store, &target, "slot").await;
    // Another editor moves the run; a same-value status edit declaring the
    // stale slot still succeeds (nothing to write, nothing lost).
    editor(&store)
        .as_origin(admin("a"))
        .amend_run(&run, utc(29, 21), &policy())
        .await
        .expect("no_op: move");
    let tip = head(&store).await;
    editor(&store)
        .as_origin(admin("b"))
        .expecting(Expect::fields([slot]))
        .set_status(&run, status(RunStatus::Planned), &policy().reminders)
        .await
        .expect("no_op: same value");
    assert_eq!(head(&store).await, tip, "no_op: nothing written");
    // A no-op on a retired timing reports it gone instead of Ok(0).
    let weeks: Vec<DateTime<Utc>> = Vec::new();
    editor(&store)
        .as_origin(admin("a"))
        .retire_fixed_run(&fixed, &weeks, &policy().reminders)
        .await
        .expect("no_op: retire");
    let result = editor(&store)
        .as_origin(admin("b"))
        .expecting(Expect::fields([note]))
        .edit_fixed_run(
            &fixed,
            crate::domain::schedule::FixedRunPatch {
                note: Some("late".into()),
                ..Default::default()
            },
            &[crate::domain::schedule::FixedField::Note],
            &weeks,
            &policy().reminders,
        )
        .await;
    let Err(SchedulerError::StaleEdit { conflicts }) = result else {
        panic!("no_op: expected StaleEdit, got {result:?}");
    };
    assert!(
        conflicts.iter().all(|conflict| conflict.target_deleted),
        "no_op: only deleted targets"
    );
}

async fn versioned_reads_carry_rows_and_versions<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (fixed, run) = seed(&store).await;
    editor(&store)
        .as_origin(member("1"))
        .set_rsvp(&run, "1", RsvpState::Yes, RsvpSource::Chat)
        .await
        .expect("versioned: answer");
    let targets = [
        BlameTarget::Run(run.clone()),
        BlameTarget::FixedRun(fixed.clone()),
        BlameTarget::Run("ghost".into()),
    ];
    let read = store.read_versioned(&targets).await.expect("versioned");
    assert_eq!(read.len(), 3, "versioned");
    for (versioned, target) in read.iter().zip(&targets) {
        assert_eq!(&versioned.target, target, "versioned");
        assert_eq!(
            versioned.versions,
            store.last_changes(target).await.expect("last changes"),
            "versioned: same index as blame"
        );
    }
    assert!(
        matches!(&read[0].row, Some(RowValue::Run(row)) if row.id == run),
        "versioned: run row"
    );
    assert!(
        matches!(read[0].answers.as_slice(), [RowValue::Rsvp(answer)] if answer.user_id == "1"),
        "versioned: answers"
    );
    assert_eq!(
        read[0].versions.get(&rsvp_field("1")).copied(),
        Some(head(&store).await.seq),
        "versioned: answer version"
    );
    assert!(
        matches!(&read[1].row, Some(RowValue::FixedRun(row)) if row.id == fixed),
        "versioned: timing row"
    );
    assert!(read[1].answers.is_empty(), "versioned");
    assert_eq!(read[2].row, None, "versioned: ghost");
    assert!(read[2].versions.is_empty(), "versioned: ghost");
}

async fn overrides_must_change_their_field_and_be_admin<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (_fixed, run) = seed(&store).await;
    let target = BlameTarget::Run(run.clone());
    editor(&store)
        .as_origin(admin("a"))
        .amend_run(&run, utc(29, 21), &policy())
        .await
        .expect("override_scope: theirs");
    let theirs = head(&store).await;
    let slot = Precondition::new(target.clone(), "slot", Some(theirs.seq));
    // Overriding the slot change with an edit that only sets the status.
    let result = editor(&store)
        .as_origin(admin("b"))
        .expecting(Expect::fields([slot.clone()]).overriding([theirs.clone()]))
        .set_status(&run, status(RunStatus::Confirmed), &policy().reminders)
        .await;
    assert!(
        matches!(
            result,
            Err(SchedulerError::Precondition(PreconditionError::OverrideUnchanged { seq }))
                if seq == theirs.seq
        ),
        "override_scope: {result:?}"
    );
    // Overriding with an edit that changes nothing.
    let result = editor(&store)
        .as_origin(admin("b"))
        .expecting(Expect::fields([slot.clone()]).overriding([theirs.clone()]))
        .amend_run(&run, utc(29, 21), &policy())
        .await;
    assert!(
        matches!(
            result,
            Err(SchedulerError::Precondition(
                PreconditionError::OverrideUnchanged { .. }
            ))
        ),
        "override_scope: no-op {result:?}"
    );
    // The store refuses a non-admin override itself.
    let revision = snapshot(&store).await.revision;
    let meta = ChangeMeta {
        origin: member("1"),
        at: utc(27, 1),
        notices: Vec::new(),
        refs: vec![theirs.clone()],
        request_digest: None,
        expect: Expect::fields([slot]).overriding([theirs.clone()]),
        outbox: Vec::new(),
    };
    let result = store.commit(revision, ChangeSet::default(), meta).await;
    assert!(
        matches!(
            result,
            Err(StoreError::Precondition(
                PreconditionError::OverrideNotAdmin
            ))
        ),
        "override_scope: store {result:?}"
    );
    assert_eq!(
        head(&store).await,
        theirs,
        "override_scope: nothing written"
    );
}

async fn never_existing_targets_are_unknown<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    seed(&store).await;
    let ghost = Precondition::new(BlameTarget::Run("ghost".into()), "status", None);
    let result = editor(&store)
        .as_origin(admin("b"))
        .expecting(Expect::fields([ghost.clone()]))
        .set_status("ghost", status(RunStatus::Confirmed), &policy().reminders)
        .await;
    assert!(
        matches!(
            result,
            Err(SchedulerError::Precondition(
                PreconditionError::UnknownTarget { .. }
            ))
        ),
        "unknown_target: refused plan {result:?}"
    );
    // v4 treats a status write to an absent run as a no-op: still unknown.
    let result = editor(&store)
        .as_origin(admin("b"))
        .expecting(Expect::fields([ghost]))
        .set_run_status("ghost", RunStatus::Done)
        .await;
    assert!(
        matches!(
            result,
            Err(SchedulerError::Precondition(
                PreconditionError::UnknownTarget { .. }
            ))
        ),
        "unknown_target: no-op {result:?}"
    );
}

/// Fails the first commit with a revision conflict caused by an unrelated
/// upstream commit, recording every commit's expectations.
struct ConflictOnce<'a, S> {
    inner: &'a S,
    other_run: String,
    seen: Mutex<Vec<Expect>>,
}

impl<S: ScheduleStore + Sync> ScheduleStore for ConflictOnce<'_, S> {
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
        let first = {
            let mut seen = self
                .seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            seen.push(meta.expect.clone());
            seen.len() == 1
        };
        if first {
            let snapshot = self.inner.load(&Scope::All).await?;
            let mut other = snapshot
                .runs
                .iter()
                .find(|row| row.id == self.other_run)
                .expect("conflict_once: other run")
                .clone();
            other.status = RunStatus::Confirmed;
            let upstream = ChangeMeta {
                origin: admin("upstream"),
                at: utc(27, 1),
                notices: Vec::new(),
                refs: Vec::new(),
                request_digest: None,
                expect: Expect::default(),
                outbox: Vec::new(),
            };
            self.inner
                .commit(
                    snapshot.revision,
                    ChangeSet {
                        changes: vec![Change::PutRun(other)],
                    },
                    upstream,
                )
                .await?;
        }
        self.inner.commit(expected_revision, changes, meta).await
    }
}

async fn expectations_survive_a_revision_conflict<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (_fixed, run) = seed(&store).await;
    let other_run = snapshot(&store)
        .await
        .runs
        .iter()
        .map(|row| row.id.clone())
        .find(|id| *id != run)
        .expect("conflict: a second run");
    let view = expect_seen(&store, &BlameTarget::Run(run.clone()), "slot").await;
    let racing = ConflictOnce {
        inner: &store,
        other_run,
        seen: Mutex::new(Vec::new()),
    };
    let mut service = SchedulerService::new(&racing, Ids::default(), FixedClock);
    service
        .as_origin(admin("b"))
        .expecting(Expect::fields([view.clone()]))
        .amend_run(&run, utc(29, 21), &policy())
        .await
        .expect("conflict: the re-plan applies");
    let seen = racing
        .seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(seen.len(), 2, "conflict: one conflict, one retry");
    assert!(
        seen.iter()
            .all(|expect| *expect == Expect::fields([view.clone()])),
        "conflict: the retry kept the same expectations: {seen:?}"
    );
}
