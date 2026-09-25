//! The change history every store must keep, and reverts planned from it,
//! driven through [`SchedulerService`] against a fresh store per check.
//! Failures panic with the check name.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveTime, TimeDelta, TimeZone, Utc, Weekday};

use crate::domain::history::{
    Actor, ChangeFilter, ChangeHistory, ChangeMeta, ChangeQuery, ChangeRecord, GENESIS_PREV_HASH,
    HistoryRefusal, Origin, RevertMode, RevertOutcome, RowKey, Surface,
};
use crate::domain::history::{
    Blame, BlameIndex, BlameTarget, CheckpointCreated, CheckpointKind, Checkpoints, NewCheckpoint,
    Via, auto_checkpoint_name, blame,
};
use crate::domain::ids::IdGenerator;
use crate::domain::members::{Member, Roster};
use crate::domain::notify::due_reminders;
use crate::domain::schedule::{
    Change, ChangeSet, EMOJI_YES, FixedEdit, NewFixedRun, NewRun, NoticeChange, ReminderPolicy,
    RsvpSource, RsvpState, RunSource, RunStatus, SchedulePolicy, ScheduleSnapshot, StatusChange,
};
use crate::domain::scheduler::{
    Clock, Committed, ScheduleStore, SchedulerError, SchedulerService, Scope, StoreError,
};

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<S: ScheduleStore + ChangeHistory + BlameIndex + Checkpoints + Sync>(
    make: impl AsyncFn() -> S,
) {
    genesis_then_one_attributed_record_per_commit(make().await).await;
    chain_stays_intact_and_pages(make().await).await;
    lists_filter_by_week_actor_and_revision(make().await).await;
    reverts_amend_status_swap_and_fixed_edit(make().await).await;
    reverting_a_revert_reapplies_the_change(make().await).await;
    unknown_changes_are_refused(make().await).await;
    conflicting_revert_is_refused_unless_forced(make().await).await;
    week_restore_leaves_timings_and_other_weeks(make().await, RevertMode::Strict).await;
    week_restore_leaves_timings_and_other_weeks(make().await, RevertMode::Force).await;
    week_restores_to_a_point_leaving_other_weeks(make().await).await;
    spam_by_one_member_reverts_exactly(make().await).await;
    sent_and_held_reminders_survive_a_revert(make().await).await;
    due_unsent_reminders_survive_a_revert(make().await).await;
    repeated_requests_apply_once(make().await).await;
    exact_retries_are_already_applied(make().await).await;
    a_rollback_that_changes_nothing_is_silent(make().await).await;
    rollback_notices_go_to_each_channel(make().await).await;
    blame_names_the_last_change_per_field(make().await).await;
    checkpoints_are_immutable_named_heads(make().await).await;
    restoring_a_checkpoint_previews_then_applies(make().await).await;
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

impl TestClock {
    fn advance(&self, by: TimeDelta) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) += by;
    }
}

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

type Service<S> = SchedulerService<S, Ids, TestClock>;

fn utc(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .single()
        .expect("valid instant")
}

/// Boss weeks reset Thursday 00:00 UTC.
fn week(index: i64) -> DateTime<Utc> {
    utc(8, 27, 0, 0) + TimeDelta::weeks(index)
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

fn admin() -> Origin {
    Origin::new(Actor::admin("root"), Surface::AdminPortal)
}

fn member(id: &str) -> Origin {
    Origin::new(Actor::member(id), Surface::PublicPortal)
}

fn roster() -> Roster {
    let mut roster = Roster::new();
    for id in ["1", "2", "3"] {
        roster.upsert(Member {
            user_id: id.into(),
            display_name: Some(format!("m{id}")),
            has_role: true,
            ..Member::default()
        });
    }
    roster
}

fn status(status: RunStatus) -> StatusChange {
    StatusChange {
        status,
        announce: false,
        via_portal: true,
    }
}

/// Two weekly timings (Sat, Sun 20:00) materialised for three boss weeks;
/// the clock is Thursday 27 Aug 01:00, early in boss week 0.
struct Fixture<S> {
    service: Service<S>,
    clock: TestClock,
    fixed: [String; 2],
}

async fn fixture<S: ScheduleStore>(store: S) -> Fixture<S> {
    let clock = TestClock(Arc::new(Mutex::new(utc(8, 27, 1, 0))));
    let mut service = SchedulerService::new(store, Ids::default(), clock.clone());
    let mut fixed = Vec::new();
    for weekday in [Weekday::Sat, Weekday::Sun] {
        fixed.push(
            service
                .as_origin(admin())
                .add_fixed_run(NewFixedRun {
                    owner_id: "1".into(),
                    channel_id: Some("900".into()),
                    bosses: vec!["HFA".into()],
                    weekday,
                    time: NaiveTime::from_hms_opt(20, 0, 0).expect("valid time"),
                    participants: vec!["1".into(), "2".into()],
                    note: None,
                })
                .await
                .expect("fixed run"),
        );
    }
    service
        .as_origin(Origin::new(
            Actor::system("delivery"),
            Surface::DeliveryTick,
        ))
        .materialise_weeks(&policy())
        .await
        .expect("materialise");
    Fixture {
        service,
        clock,
        fixed: [fixed[0].clone(), fixed[1].clone()],
    }
}

async fn snapshot<S: ScheduleStore>(service: &Service<S>) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.expect("load")
}

/// The run of `fixed` in boss week `index`.
async fn run_of<S: ScheduleStore>(service: &Service<S>, fixed: &str, index: i64) -> String {
    snapshot(service)
        .await
        .runs
        .iter()
        .find(|run| run.fixed_run_id.as_deref() == Some(fixed) && run.week_start == week(index))
        .expect("materialised run")
        .id
        .clone()
}

async fn all_records<S: ChangeHistory>(store: &S) -> Vec<ChangeRecord> {
    let mut query = ChangeQuery::new(ChangeFilter::All);
    let mut records = Vec::new();
    loop {
        let page = store.list_changes(&query).await.expect("list");
        records.extend(page.records);
        match page.next_cursor {
            Some(cursor) => query.cursor = Some(cursor),
            None => return records,
        }
    }
}

async fn latest<S: ChangeHistory>(store: &S) -> ChangeRecord {
    all_records(store).await.pop().expect("genesis at least")
}

/// Runs, timings and RSVPs exactly, reminders by content.
fn comparable(state: &ScheduleSnapshot) -> impl PartialEq + std::fmt::Debug {
    let mut reminders: Vec<_> = state
        .reminders
        .iter()
        .map(|row| {
            (
                row.run_id.clone(),
                row.kind.clone(),
                row.fire_at,
                row.sent_at,
            )
        })
        .collect();
    reminders.sort();
    (
        state.fixed_runs.clone(),
        state.runs.clone(),
        state.rsvps.clone(),
        reminders,
    )
}

fn unique_request() -> Option<String> {
    Some(uuid::Uuid::new_v4().to_string())
}

async fn revert<S: ScheduleStore + ChangeHistory>(
    service: &mut Service<S>,
    seqs: &[u64],
    mode: RevertMode,
    held: &BTreeSet<String>,
) -> RevertOutcome {
    service
        .revert_changes(
            "root",
            unique_request(),
            seqs,
            mode,
            &policy().reminders,
            held,
        )
        .await
        .expect("revert runs")
}

async fn genesis_then_one_attributed_record_per_commit<S: ScheduleStore + ChangeHistory>(store: S) {
    let genesis = latest(&store).await;
    assert_eq!((genesis.seq, genesis.id.as_str()), (0, "genesis"));
    assert_eq!(genesis.prev_hash, GENESIS_PREV_HASH);
    assert!(genesis.rows.is_empty() && genesis.refs.is_empty());
    assert_eq!(
        store.history_head().await.expect("head"),
        genesis.reference()
    );

    let Fixture { mut service, .. } = fixture(store).await;
    let before = all_records(service.store()).await.len();
    let run = snapshot(&service).await.runs[0].clone();
    service
        .as_origin(member("2").with_request_id("req-42"))
        .set_rsvp(&run.id, "2", RsvpState::Yes, RsvpSource::Chat)
        .await
        .expect("rsvp");
    let record = latest(service.store()).await;
    assert_eq!(record.seq, u64::try_from(before).expect("fits"));
    assert_eq!(record.revision, snapshot(&service).await.revision);
    assert_eq!(record.origin, member("2").with_request_id("req-42"));
    assert_eq!(record.at, utc(8, 27, 1, 0), "the operation's clock reading");
    assert_eq!(record.weeks, [run.week_start]);
    let rsvp = record
        .rows
        .iter()
        .find(|row| {
            row.key
                == RowKey::Rsvp {
                    run_id: run.id.clone(),
                    user_id: "2".into(),
                }
        })
        .expect("the RSVP row is recorded");
    assert!(rsvp.before.is_none() && rsvp.after.is_some());

    let count = all_records(service.store()).await.len();
    service
        .as_origin(admin())
        .set_run_status(&run.id, run.status)
        .await
        .expect("no-op");
    assert_eq!(
        all_records(service.store()).await.len(),
        count,
        "an empty commit records nothing"
    );

    let cancel = StatusChange {
        status: RunStatus::Cancelled,
        announce: true,
        via_portal: true,
    };
    service
        .as_origin(admin())
        .set_status(&run.id, cancel, &policy().reminders)
        .await
        .expect("status");
    let record = latest(service.store()).await;
    assert_eq!(record.notices, ["notice.run.status.cancelled"]);
    assert!(
        service
            .store()
            .verify_history()
            .await
            .expect("verify")
            .is_intact(),
        "genesis_then_one_attributed_record_per_commit"
    );
}

async fn chain_stays_intact_and_pages<S: ScheduleStore + ChangeHistory + Sync>(store: S) {
    let Fixture { mut service, .. } = fixture(store).await;
    let run = snapshot(&service).await.runs[0].id.clone();
    for n in 0..40 {
        let state = if n % 2 == 0 {
            RsvpState::Yes
        } else {
            RsvpState::No
        };
        service
            .as_origin(member("1"))
            .set_rsvp(&run, "1", state, RsvpSource::Reaction)
            .await
            .expect("rsvp");
    }
    let records = all_records(service.store()).await;
    let verification = service.store().verify_history().await.expect("verify");
    assert!(verification.is_intact(), "chain_stays_intact_and_pages");
    assert_eq!(
        verification.records,
        u64::try_from(records.len()).expect("fits")
    );
    let head = records.last().expect("records").reference();
    assert_eq!(verification.head.as_ref(), Some(&head));
    assert_eq!(service.store().history_head().await.expect("head"), head);
    assert!(
        service
            .store()
            .contains_anchor(&head)
            .await
            .expect("anchor")
    );
    let forged = crate::domain::history::ChangeRef {
        seq: head.seq,
        hash: "0".repeat(64),
    };
    assert!(
        !service
            .store()
            .contains_anchor(&forged)
            .await
            .expect("anchor")
    );
    for (seq, record) in records.iter().enumerate() {
        assert_eq!(record.seq, u64::try_from(seq).expect("fits"));
    }

    for newest_first in [false, true] {
        let mut query = ChangeQuery {
            limit: 7,
            newest_first,
            ..ChangeQuery::new(ChangeFilter::All)
        };
        let mut paged = Vec::new();
        loop {
            let page = service.store().list_changes(&query).await.expect("page");
            assert!(page.records.len() <= 7);
            paged.extend(page.records);
            match page.next_cursor {
                Some(cursor) => query.cursor = Some(cursor),
                None => break,
            }
        }
        let mut expected = records.clone();
        if newest_first {
            expected.reverse();
        }
        assert_eq!(paged, expected, "newest_first={newest_first}");
    }
    assert_eq!(
        service.store().load_change(3).await.expect("load"),
        Some(records[3].clone())
    );
}

async fn lists_filter_by_week_actor_and_revision<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service, fixed, ..
    } = fixture(store).await;
    let first = run_of(&service, &fixed[0], 0).await;
    let second = run_of(&service, &fixed[0], 1).await;
    let mark = snapshot(&service).await.revision;
    service
        .as_origin(member("3"))
        .set_rsvp(&second, "3", RsvpState::Maybe, RsvpSource::Chat)
        .await
        .expect("rsvp");
    service
        .as_origin(member("2"))
        .set_rsvp(&first, "2", RsvpState::No, RsvpSource::Chat)
        .await
        .expect("rsvp");
    let list = |filter| {
        let query = ChangeQuery::new(filter);
        let store = service.store();
        async move { store.list_changes(&query).await.expect("list").records }
    };
    let week_one = list(ChangeFilter::Week(week(1))).await;
    assert!(week_one.iter().all(|record| record.touches_week(week(1))));
    assert_eq!(week_one.last().map(|r| &r.origin), Some(&member("3")));
    let by_member = list(ChangeFilter::Actor(Actor::member("2"))).await;
    assert_eq!(
        by_member.len(),
        1,
        "lists_filter_by_week_actor_and_revision"
    );
    assert_eq!(by_member[0].weeks, [week(0)]);
    let recent = list(ChangeFilter::Revisions {
        from: mark + 1,
        to: u64::MAX >> 1,
    })
    .await;
    assert_eq!(recent.len(), 2);
}

async fn reverts_amend_status_swap_and_fixed_edit<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service, fixed, ..
    } = fixture(store).await;
    let run = run_of(&service, &fixed[0], 0).await;
    let none = BTreeSet::new();
    for step in ["amend", "status", "swap", "fixed"] {
        let before = snapshot(&service).await;
        let act = service.as_origin(member("2"));
        match step {
            "amend" => {
                act.amend_run(&run, utc(8, 29, 22, 0), &policy())
                    .await
                    .expect("amend");
            }
            "status" => {
                act.set_status(&run, status(RunStatus::Confirmed), &policy().reminders)
                    .await
                    .expect("status");
            }
            "swap" => {
                act.swap_participants(&run, &["2".into()], &["3".into()], true, &roster())
                    .await
                    .expect("swap");
            }
            _ => {
                let edit = FixedEdit {
                    bosses: None,
                    weekday: None,
                    time: Some(NaiveTime::from_hms_opt(21, 15, 0).expect("valid time")),
                    participants: None,
                    channel_id: None,
                    note: Some("moved".into()),
                };
                act.update_fixed(&fixed[1], edit, &roster(), &policy())
                    .await
                    .expect("fixed edit");
            }
        }
        assert_ne!(
            comparable(&snapshot(&service).await),
            comparable(&before),
            "{step} changed"
        );
        let record = latest(service.store()).await;
        let outcome = revert(&mut service, &[record.seq], RevertMode::Strict, &none).await;
        let RevertOutcome::Reverted { seqs, notices, .. } = &outcome else {
            panic!("{step}: {outcome:?}");
        };
        assert_eq!(*seqs, [record.seq]);
        assert_eq!(notices.len(), 1, "{step}: one channel, one notice");
        assert_eq!(notices[0].effect_kind(), "notice.rollback.reverted");
        assert_eq!(
            comparable(&snapshot(&service).await),
            comparable(&before),
            "reverts_amend_status_swap_and_fixed_edit: {step}"
        );
        let revert_record = latest(service.store()).await;
        assert_eq!(revert_record.origin.surface, Surface::Rollback);
        assert_eq!(revert_record.origin.actor, Actor::admin("root"));
        assert_eq!(revert_record.refs, [record.reference()]);
        assert_eq!(revert_record.notices, ["notice.rollback.reverted"]);
    }
    assert!(
        service
            .store()
            .verify_history()
            .await
            .expect("verify")
            .is_intact()
    );
}

async fn reverting_a_revert_reapplies_the_change<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service, fixed, ..
    } = fixture(store).await;
    let run = run_of(&service, &fixed[0], 0).await;
    service
        .as_origin(member("2"))
        .amend_run(&run, utc(8, 29, 22, 0), &policy())
        .await
        .expect("amend");
    let amended = snapshot(&service).await;
    let amend = latest(service.store()).await;
    let none = BTreeSet::new();
    revert(&mut service, &[amend.seq], RevertMode::Strict, &none).await;
    let undo = latest(service.store()).await;
    assert_ne!(comparable(&snapshot(&service).await), comparable(&amended));
    let outcome = revert(&mut service, &[undo.seq], RevertMode::Strict, &none).await;
    assert!(
        matches!(outcome, RevertOutcome::Reverted { .. }),
        "{outcome:?}"
    );
    assert_eq!(
        comparable(&snapshot(&service).await),
        comparable(&amended),
        "reverting_a_revert_reapplies_the_change"
    );
    assert_eq!(latest(service.store()).await.refs, [undo.reference()]);
}

async fn unknown_changes_are_refused<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture { mut service, .. } = fixture(store).await;
    let revision = snapshot(&service).await.revision;
    for seq in [0, 9_999] {
        let refused = service
            .revert_changes(
                "root",
                unique_request(),
                &[seq],
                RevertMode::Force,
                &policy().reminders,
                &BTreeSet::new(),
            )
            .await;
        assert_eq!(
            refused,
            Err(SchedulerError::History(HistoryRefusal::UnknownChange(seq))),
            "unknown_changes_are_refused"
        );
    }
    let nothing = service
        .revert_by_actor(
            "root",
            unique_request(),
            &Actor::member("nobody"),
            utc(1, 1, 0, 0),
            RevertMode::Strict,
            &policy().reminders,
            &BTreeSet::new(),
        )
        .await;
    assert_eq!(
        nothing,
        Err(SchedulerError::History(HistoryRefusal::NothingToRevert))
    );
    assert_eq!(snapshot(&service).await.revision, revision);
}

async fn conflicting_revert_is_refused_unless_forced<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service, fixed, ..
    } = fixture(store).await;
    let run = run_of(&service, &fixed[0], 0).await;
    let original = snapshot(&service).await;
    service
        .as_origin(member("2"))
        .amend_run(&run, utc(8, 29, 22, 0), &policy())
        .await
        .expect("amend");
    let amend = latest(service.store()).await;
    service
        .as_origin(admin())
        .set_status(&run, status(RunStatus::Confirmed), &policy().reminders)
        .await
        .expect("status");
    let revision = snapshot(&service).await.revision;
    let none = BTreeSet::new();
    let refused = revert(&mut service, &[amend.seq], RevertMode::Strict, &none).await;
    let RevertOutcome::Conflicts(conflicts) = refused else {
        panic!("conflicting_revert_is_refused_unless_forced: {refused:?}");
    };
    assert_eq!(conflicts[0].key, RowKey::Run(run.clone()));
    assert_eq!(conflicts[0].seq, amend.seq);
    assert_ne!(conflicts[0].expected, conflicts[0].found);
    assert_eq!(
        snapshot(&service).await.revision,
        revision,
        "nothing written"
    );

    let forced = revert(&mut service, &[amend.seq], RevertMode::Force, &none).await;
    assert!(
        matches!(&forced, RevertOutcome::Reverted { overridden, .. } if !overridden.is_empty()),
        "{forced:?}"
    );
    let after = snapshot(&service).await;
    let restored = after.runs.iter().find(|r| r.id == run).expect("run");
    let was = original.runs.iter().find(|r| r.id == run).expect("run");
    assert_eq!(restored, was, "forced: current goes back to before");
}

async fn week_restore_leaves_timings_and_other_weeks<S: ScheduleStore + ChangeHistory>(
    store: S,
    mode: RevertMode,
) {
    let Fixture {
        mut service, fixed, ..
    } = fixture(store).await;
    let point = snapshot(&service).await;
    let runs = [
        run_of(&service, &fixed[0], 0).await,
        run_of(&service, &fixed[0], 1).await,
        run_of(&service, &fixed[0], 2).await,
    ];
    let edit = FixedEdit {
        bosses: None,
        weekday: None,
        time: Some(NaiveTime::from_hms_opt(21, 15, 0).expect("valid time")),
        participants: None,
        channel_id: None,
        note: None,
    };
    service
        .as_origin(admin())
        .update_fixed(&fixed[0], edit, &roster(), &policy())
        .await
        .expect("fixed edit");
    service
        .as_origin(admin())
        .set_status(&runs[1], status(RunStatus::Confirmed), &policy().reminders)
        .await
        .expect("later week-1 edit");
    let edited = snapshot(&service).await;
    let outcome = service
        .restore_week_to(
            "root",
            unique_request(),
            week(0),
            point.revision,
            mode,
            &policy().reminders,
            &BTreeSet::new(),
        )
        .await
        .expect("restore");
    let RevertOutcome::Reverted { skipped, .. } = &outcome else {
        panic!("week_restore_leaves_timings_and_other_weeks {mode:?}: {outcome:?}");
    };
    let skipped: BTreeSet<RowKey> = skipped.iter().map(|row| row.key.clone()).collect();
    assert!(
        skipped.contains(&RowKey::FixedRun(fixed[0].clone())),
        "{skipped:?}"
    );
    assert!(skipped.contains(&RowKey::Run(runs[1].clone())));
    assert!(skipped.contains(&RowKey::Run(runs[2].clone())));
    let after = snapshot(&service).await;
    let find = |state: &ScheduleSnapshot, id: &str| {
        state
            .runs
            .iter()
            .find(|run| run.id == id)
            .cloned()
            .expect("run")
    };
    assert_eq!(find(&after, &runs[0]), find(&point, &runs[0]), "{mode:?}");
    for other in &runs[1..] {
        assert_eq!(
            find(&after, other),
            find(&edited, other),
            "{mode:?}: {other}"
        );
    }
    assert_eq!(
        after.fixed_runs, edited.fixed_runs,
        "the timing keeps its edit"
    );
}

async fn week_restores_to_a_point_leaving_other_weeks<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service,
        clock,
        fixed,
    } = fixture(store).await;
    let point = snapshot(&service).await;
    let this_week = run_of(&service, &fixed[0], 0).await;
    let this_sunday = run_of(&service, &fixed[1], 0).await;
    let next_week = run_of(&service, &fixed[0], 1).await;
    service
        .as_origin(member("2"))
        .amend_run(&this_week, utc(8, 29, 21, 0), &policy())
        .await
        .expect("amend");
    clock.advance(TimeDelta::minutes(5));
    service
        .as_origin(member("2"))
        .set_rsvp(&next_week, "2", RsvpState::No, RsvpSource::Chat)
        .await
        .expect("other week");
    service
        .as_origin(member("1"))
        .set_rsvp(&this_sunday, "1", RsvpState::Yes, RsvpSource::Chat)
        .await
        .expect("rsvp");
    service
        .as_origin(admin())
        .set_status(
            &this_sunday,
            status(RunStatus::Cancelled),
            &policy().reminders,
        )
        .await
        .expect("status");
    let outcome = service
        .restore_week_to(
            "root",
            unique_request(),
            week(0),
            point.revision,
            RevertMode::Strict,
            &policy().reminders,
            &BTreeSet::new(),
        )
        .await
        .expect("restore");
    let RevertOutcome::Reverted { seqs, .. } = &outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(seqs.len(), 3, "only this week's changes");
    assert!(
        seqs.windows(2).all(|pair| pair[0] > pair[1]),
        "newest first"
    );
    let now = snapshot(&service).await;
    let in_week = |state: &ScheduleSnapshot| {
        let runs: Vec<_> = state
            .runs
            .iter()
            .filter(|run| run.week_start == week(0))
            .cloned()
            .collect();
        let ids: BTreeSet<_> = runs.iter().map(|run| run.id.clone()).collect();
        let rsvps: Vec<_> = state
            .rsvps
            .iter()
            .filter(|row| ids.contains(&row.run_id))
            .cloned()
            .collect();
        (runs, rsvps)
    };
    assert_eq!(
        in_week(&now),
        in_week(&point),
        "week_restores_to_a_point_leaving_other_weeks"
    );
    assert!(
        now.rsvps
            .iter()
            .any(|row| row.run_id == next_week && row.state == RsvpState::No),
        "the other week keeps its change"
    );
}

async fn spam_by_one_member_reverts_exactly<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service,
        clock,
        fixed,
    } = fixture(store).await;
    let run = run_of(&service, &fixed[0], 0).await;
    let other = run_of(&service, &fixed[1], 0).await;
    let before = snapshot(&service).await;
    let since = clock.now();
    for n in 0..30 {
        clock.advance(TimeDelta::minutes(1));
        let to = utc(8, 29, 10, 0) + TimeDelta::minutes(10 * n);
        service
            .as_origin(member("2"))
            .amend_run(&run, to, &policy())
            .await
            .expect("spam move");
        if n % 3 == 0 {
            let state = if n % 2 == 0 {
                RsvpState::No
            } else {
                RsvpState::Yes
            };
            service
                .as_origin(member("2"))
                .set_rsvp(&other, "2", state, RsvpSource::Chat)
                .await
                .expect("spam rsvp");
        }
    }
    let next_week = run_of(&service, &fixed[0], 1).await;
    service
        .as_origin(member("1"))
        .set_rsvp(&next_week, "1", RsvpState::Yes, RsvpSource::Chat)
        .await
        .expect("bystander");
    let outcome = service
        .revert_by_actor(
            "root",
            unique_request(),
            &Actor::member("2"),
            since,
            RevertMode::Strict,
            &policy().reminders,
            &BTreeSet::new(),
        )
        .await
        .expect("revert");
    let RevertOutcome::Reverted { seqs, notices, .. } = &outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(seqs.len(), 40);
    assert_eq!(notices.len(), 1, "both runs share one home channel");
    assert_eq!(notices[0].effect_context()[0], "reverted:40");
    let after = snapshot(&service).await;
    let mut without_bystander = after.clone();
    without_bystander
        .rsvps
        .retain(|row| row.run_id != next_week);
    assert_eq!(
        comparable(&without_bystander),
        comparable(&before),
        "spam_by_one_member_reverts_exactly"
    );
    assert!(after.rsvps.iter().any(|row| row.run_id == next_week));
    let mut kinds = BTreeSet::new();
    for row in after.reminders.iter().filter(|row| row.run_id == run) {
        assert!(kinds.insert(row.kind.clone()), "no duplicate reminder");
        assert!(row.sent_at.is_none(), "future reminders stay unsent");
    }
}

async fn sent_and_held_reminders_survive_a_revert<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service, fixed, ..
    } = fixture(store).await;
    let run = run_of(&service, &fixed[0], 0).await;
    let sunday = run_of(&service, &fixed[1], 0).await;
    service
        .as_origin(member("2"))
        .amend_run(&run, utc(8, 29, 22, 0), &policy())
        .await
        .expect("amend");
    let amend = latest(service.store()).await;
    service
        .as_origin(member("2"))
        .amend_run(&sunday, utc(8, 30, 22, 0), &policy())
        .await
        .expect("amend");
    let sunday_amend = latest(service.store()).await;
    let state = snapshot(&service).await;
    let day_of = |run_id: &str| {
        state
            .reminders
            .iter()
            .find(|row| row.run_id == run_id && row.kind == "day_of")
            .expect("day_of")
            .id
            .clone()
    };
    let (sent, held) = (day_of(&run), day_of(&sunday));
    service
        .as_origin(Origin::new(
            Actor::system("delivery"),
            Surface::DeliveryTick,
        ))
        .mark_reminder_sent(&sent, Some("5001"))
        .await
        .expect("sent");
    let held_set = BTreeSet::from([held.clone()]);
    let outcome = revert(
        &mut service,
        &[amend.seq, sunday_amend.seq],
        RevertMode::Strict,
        &held_set,
    )
    .await;
    assert!(
        matches!(outcome, RevertOutcome::Reverted { .. }),
        "{outcome:?}"
    );
    let after = snapshot(&service).await;
    let rows = |run_id: &str| -> Vec<_> {
        after
            .reminders
            .iter()
            .filter(|row| row.run_id == run_id && row.kind == "day_of")
            .cloned()
            .collect()
    };
    let sent_rows = rows(&run);
    assert_eq!(
        sent_rows.len(),
        1,
        "sent_and_held_reminders_survive_a_revert"
    );
    assert_eq!(sent_rows[0].id, sent, "the sent reminder is kept");
    assert_eq!(sent_rows[0].message_id.as_deref(), Some("5001"));
    let held_rows = rows(&sunday);
    assert_eq!(held_rows.len(), 1);
    assert_eq!(held_rows[0].id, held, "a journal-held reminder is kept");
}

async fn due_unsent_reminders_survive_a_revert<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service,
        clock,
        fixed,
    } = fixture(store).await;
    let run = run_of(&service, &fixed[0], 0).await;
    // The day-of ping (Sat 09:00) is due but the tick has not sent it yet.
    clock.advance(TimeDelta::hours(56));
    let due = |state: &ScheduleSnapshot| -> Vec<String> {
        due_reminders(&state.reminders, utc(8, 29, 9, 0))
            .into_iter()
            .filter(|row| row.run_id == run)
            .map(|row| row.id.clone())
            .collect()
    };
    let before = due(&snapshot(&service).await);
    assert_eq!(before.len(), 1, "the day-of ping is due");
    let since = clock.now();
    for state in [RsvpState::No, RsvpState::Yes, RsvpState::Maybe] {
        service
            .as_origin(member("2"))
            .set_rsvp(&run, "2", state, RsvpSource::Chat)
            .await
            .expect("rsvp spam");
    }
    // Both participants confirm: the second reaction flips the run to
    // `confirmed` (no reminder rebuild), so reverting it changes the run's
    // status and reconciles its reminders.
    for (user, origin) in [("1", member("1")), ("2", member("2"))] {
        service
            .as_origin(origin)
            .apply_reaction(&run, user, EMOJI_YES, true)
            .await
            .expect("reaction");
    }
    assert_eq!(
        snapshot(&service)
            .await
            .runs
            .iter()
            .find(|row| row.id == run)
            .map(|row| row.status),
        Some(RunStatus::Confirmed)
    );
    service
        .revert_by_actor(
            "root",
            unique_request(),
            &Actor::member("2"),
            since,
            RevertMode::Strict,
            &policy().reminders,
            &BTreeSet::new(),
        )
        .await
        .expect("revert");
    assert_eq!(
        due(&snapshot(&service).await),
        before,
        "due_unsent_reminders_survive_a_revert: same row, still unsent"
    );
}

async fn repeated_requests_apply_once<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service, fixed, ..
    } = fixture(store).await;
    let run = run_of(&service, &fixed[0], 0).await;
    let request = member("2").with_request_id("move-1");
    service
        .as_origin(request.clone())
        .amend_run(&run, utc(8, 29, 22, 0), &policy())
        .await
        .expect("first attempt");
    let first = latest(service.store()).await;
    let state = snapshot(&service).await;
    let already = Err(SchedulerError::AlreadyApplied {
        seq: first.seq,
        revision: first.revision,
    });
    // The caller never saw the answer and retries exactly.
    let retry = service
        .as_origin(request.clone())
        .amend_run(&run, utc(8, 29, 22, 0), &policy())
        .await;
    assert_eq!(retry, already, "repeated_requests_apply_once");
    let other = service
        .as_origin(request.clone())
        .amend_run(&run, utc(8, 29, 23, 0), &policy())
        .await;
    assert_eq!(
        other,
        Err(SchedulerError::IdempotencyMismatch { seq: first.seq }),
        "the same id for a different request is refused"
    );
    assert_eq!(snapshot(&service).await, state, "nothing applied again");

    // At the store: a stale-revision retry of an ambiguous commit replays;
    // a different request under the same id is refused.
    let recorded = service
        .store()
        .recorded_request(&request.actor, "move-1")
        .await
        .expect("lookup")
        .expect("recorded");
    assert_eq!(recorded.committed.seq, first.seq);
    let stale = |digest: Option<String>| ChangeMeta {
        origin: request.clone(),
        at: utc(8, 27, 2, 0),
        notices: Vec::new(),
        refs: Vec::new(),
        request_digest: digest,
        expect: Default::default(),
    };
    let changes = || ChangeSet {
        changes: vec![Change::DeleteRsvp {
            run_id: run.clone(),
            user_id: "2".into(),
        }],
    };
    let replay = service
        .store()
        .commit(0, changes(), stale(recorded.digest.clone()))
        .await;
    assert_eq!(
        replay,
        Ok(Some(Committed {
            seq: first.seq,
            revision: first.revision,
            replayed: true,
        }))
    );
    let mismatch = service
        .store()
        .commit(0, changes(), stale(Some("other".into())))
        .await;
    assert_eq!(
        mismatch,
        Err(StoreError::IdempotencyMismatch { seq: first.seq })
    );
    let per_actor = service
        .as_origin(member("3").with_request_id("move-1"))
        .set_rsvp(&run, "3", RsvpState::Yes, RsvpSource::Chat)
        .await;
    assert_eq!(per_actor, Ok(()), "request ids are per actor");
}

async fn exact_retries_are_already_applied<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service, fixed, ..
    } = fixture(store).await;
    let run = run_of(&service, &fixed[0], 0).await;
    let sunday = run_of(&service, &fixed[1], 0).await;

    // A remove-only swap would fail if re-planned ("not on the run").
    let swap = member("1").with_request_id("swap-1");
    service
        .as_origin(swap.clone())
        .swap_participants(&run, &["2".into()], &[], true, &roster())
        .await
        .expect("swap");
    let swapped = latest(service.store()).await;
    let retry = service
        .as_origin(swap)
        .swap_participants(&run, &["2".into()], &[], true, &roster())
        .await;
    assert_eq!(
        retry,
        Err(SchedulerError::AlreadyApplied {
            seq: swapped.seq,
            revision: swapped.revision,
        }),
        "exact_retries_are_already_applied: swap"
    );

    service
        .as_origin(member("2"))
        .amend_run(&sunday, utc(8, 30, 22, 0), &policy())
        .await
        .expect("amend");
    let amend = latest(service.store()).await;
    for (mode, request, target) in [
        (RevertMode::Strict, "rv-strict", amend.seq),
        (RevertMode::Force, "rv-force", swapped.seq),
    ] {
        let first = service
            .revert_changes(
                "root",
                Some(request.into()),
                &[target],
                mode,
                &policy().reminders,
                &BTreeSet::new(),
            )
            .await
            .expect("revert");
        assert!(matches!(first, RevertOutcome::Reverted { .. }), "{first:?}");
        let undo = latest(service.store()).await;
        let count = all_records(service.store()).await.len();
        let retry = service
            .revert_changes(
                "root",
                Some(request.into()),
                &[target],
                mode,
                &policy().reminders,
                &BTreeSet::new(),
            )
            .await;
        assert_eq!(
            retry,
            Err(SchedulerError::AlreadyApplied {
                seq: undo.seq,
                revision: undo.revision,
            }),
            "exact_retries_are_already_applied: {mode:?}"
        );
        assert_eq!(
            all_records(service.store()).await.len(),
            count,
            "no second record or notice"
        );
    }
}

async fn a_rollback_that_changes_nothing_is_silent<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service, fixed, ..
    } = fixture(store).await;
    let run = run_of(&service, &fixed[0], 0).await;
    service
        .as_origin(member("2"))
        .set_rsvp(&run, "2", RsvpState::No, RsvpSource::Chat)
        .await
        .expect("rsvp");
    let rsvp = latest(service.store()).await;
    let none = BTreeSet::new();
    revert(&mut service, &[rsvp.seq], RevertMode::Strict, &none).await;
    let count = all_records(service.store()).await.len();
    let again = revert(&mut service, &[rsvp.seq], RevertMode::Force, &none).await;
    assert!(
        matches!(&again, RevertOutcome::Unchanged { seqs, .. } if *seqs == [rsvp.seq]),
        "a_rollback_that_changes_nothing_is_silent: {again:?}"
    );
    assert_eq!(all_records(service.store()).await.len(), count, "no record");
}

async fn rollback_notices_go_to_each_channel<S: ScheduleStore + ChangeHistory>(store: S) {
    let Fixture {
        mut service,
        clock,
        fixed,
    } = fixture(store).await;
    let home = [
        run_of(&service, &fixed[0], 0).await,
        run_of(&service, &fixed[1], 0).await,
    ];
    let mut elsewhere = Vec::new();
    for channel in [Some("901"), None] {
        let new = NewRun {
            fixed_run_id: None,
            channel_id: channel.map(str::to_owned),
            week_start: week(0),
            datetime: utc(8, 29, 12, 0),
            bosses: vec!["HFA".into()],
            participants: vec!["1".into(), "2".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        };
        elsewhere.push(
            service
                .as_origin(admin())
                .create_run(new)
                .await
                .expect("run"),
        );
    }
    clock.advance(TimeDelta::minutes(1));
    let since = clock.now();
    for run in home.iter().chain(&elsewhere) {
        service
            .as_origin(member("2"))
            .set_rsvp(run, "2", RsvpState::No, RsvpSource::Chat)
            .await
            .expect("rsvp");
    }
    let outcome = service
        .revert_by_actor(
            "root",
            unique_request(),
            &Actor::member("2"),
            since,
            RevertMode::Strict,
            &policy().reminders,
            &BTreeSet::new(),
        )
        .await
        .expect("revert");
    let RevertOutcome::Reverted { notices, .. } = &outcome else {
        panic!("{outcome:?}");
    };
    let mut by_channel: Vec<(Option<String>, Vec<String>)> = notices
        .iter()
        .map(|notice| match &notice.change {
            NoticeChange::Rollback { run_ids, .. } => (notice.channel_id.clone(), run_ids.clone()),
            other => panic!("{other:?}"),
        })
        .collect();
    by_channel.sort();
    let mut expected_home = home.to_vec();
    expected_home.sort();
    assert_eq!(
        by_channel,
        [
            (None, vec![elsewhere[1].clone()]),
            (Some("900".into()), expected_home),
            (Some("901".into()), vec![elsewhere[0].clone()]),
        ],
        "rollback_notices_go_to_each_channel"
    );
    assert_eq!(latest(service.store()).await.notices.len(), 3);
}

fn line<'a>(blame: &'a Blame, field: &str) -> Option<&'a crate::domain::history::Attribution> {
    blame
        .lines
        .iter()
        .find(|line| line.field == field)
        .unwrap_or_else(|| panic!("no {field} line"))
        .last
        .as_ref()
}

async fn blame_names_the_last_change_per_field<S: ScheduleStore + ChangeHistory + BlameIndex>(
    store: S,
) {
    let Fixture {
        mut service, fixed, ..
    } = fixture(store).await;
    let run = run_of(&service, &fixed[0], 0).await;
    let target = BlameTarget::Run(run.clone());
    let materialised = latest(service.store()).await;
    let fresh = blame(service.store(), &target)
        .await
        .expect("blame")
        .expect("run");
    for field in ["slot", "bosses", "participants", "channel", "status"] {
        let last = line(&fresh, field).expect("set by materialising");
        assert_eq!(last.seq, materialised.seq, "{field}");
        assert_eq!(last.actor, Actor::system("delivery"));
        assert_eq!(last.surface, Surface::DeliveryTick);
        assert_eq!(last.via, Via::Direct);
    }

    service
        .as_origin(member("2"))
        .amend_run(&run, utc(8, 29, 22, 0), &policy())
        .await
        .expect("amend");
    let amend = latest(service.store()).await;
    service
        .as_origin(member("1"))
        .set_rsvp(&run, "1", RsvpState::Yes, RsvpSource::Chat)
        .await
        .expect("rsvp");
    let rsvp = latest(service.store()).await;
    service
        .as_origin(admin())
        .swap_participants(&run, &["2".into()], &["3".into()], true, &roster())
        .await
        .expect("swap");
    let swap = latest(service.store()).await;
    let now = blame(service.store(), &target)
        .await
        .expect("blame")
        .expect("run");
    assert_eq!(
        line(&now, "slot").map(|l| l.seq),
        Some(amend.seq),
        "blame_names_the_last_change_per_field"
    );
    assert_eq!(line(&now, "bosses").map(|l| l.seq), Some(materialised.seq));
    assert_eq!(line(&now, "participants").map(|l| l.seq), Some(swap.seq));
    assert_eq!(line(&now, "rsvp:1").map(|l| l.seq), Some(rsvp.seq));
    assert_eq!(
        line(&now, "rsvp:1").map(|l| l.actor.clone()),
        Some(Actor::member("1"))
    );

    revert(
        &mut service,
        &[amend.seq],
        RevertMode::Force,
        &BTreeSet::new(),
    )
    .await;
    let undo = latest(service.store()).await;
    let reverted = blame(service.store(), &target)
        .await
        .expect("blame")
        .expect("run");
    let slot = line(&reverted, "slot").expect("slot");
    assert_eq!(slot.seq, undo.seq, "the rollback set the slot back");
    assert_eq!(slot.surface, Surface::Rollback);
    assert_eq!(
        slot.via,
        Via::Rollback {
            undid: vec![amend.reference()],
            restored_to: None,
        }
    );
    // Force puts the whole recorded row back, so the later swap's line-up
    // is overridden too and the rollback now owns that field.
    assert_eq!(
        line(&reverted, "participants").map(|l| l.seq),
        Some(undo.seq)
    );

    let edit = FixedEdit {
        bosses: None,
        weekday: None,
        time: Some(NaiveTime::from_hms_opt(21, 15, 0).expect("valid time")),
        participants: None,
        channel_id: None,
        note: None,
    };
    let created = all_records(service.store())
        .await
        .into_iter()
        .find(|record| record.seq > 0)
        .expect("first change");
    service
        .as_origin(admin())
        .update_fixed(&fixed[1], edit, &roster(), &policy())
        .await
        .expect("fixed edit");
    let edited = latest(service.store()).await;
    let timing = blame(service.store(), &BlameTarget::FixedRun(fixed[1].clone()))
        .await
        .expect("blame")
        .expect("timing");
    assert_eq!(line(&timing, "time").map(|l| l.seq), Some(edited.seq));
    assert_eq!(
        line(&timing, "day").map(|l| l.actor.clone()),
        Some(Actor::admin("root"))
    );
    assert!(line(&timing, "day").is_some_and(|l| l.seq < edited.seq && l.seq >= created.seq));
    assert_eq!(
        blame(service.store(), &BlameTarget::Run("absent".into()))
            .await
            .expect("blame"),
        None
    );
}

async fn checkpoints_are_immutable_named_heads<S: ScheduleStore + ChangeHistory + Checkpoints>(
    store: S,
) {
    let Fixture { service, .. } = fixture(store).await;
    let head = service.store().history_head().await.expect("head");
    let named = service
        .create_checkpoint("root", "before the raid", week(0), &policy())
        .await
        .expect("checkpoint");
    assert_eq!(named.head, head, "checkpoints_are_immutable_named_heads");
    assert_eq!(named.kind, CheckpointKind::Admin);
    assert_eq!(named.created_by, Actor::admin("root"));
    assert_eq!(named.created_at, utc(8, 27, 1, 0));
    for (name, why) in [
        ("before the raid", "taken"),
        (" padded", "outer spaces"),
        ("", "empty"),
        ("week 2026-08-27 start", "reserved"),
        ("tab\there", "control character"),
    ] {
        let refused = service
            .create_checkpoint("root", name, week(0), &policy())
            .await;
        assert!(
            matches!(
                refused,
                Err(SchedulerError::Store(StoreError::Constraint(_)))
            ),
            "{why}: {refused:?}"
        );
    }
    let auto = |name: &str| NewCheckpoint {
        name: name.into(),
        kind: CheckpointKind::Auto,
        week: week(0),
        created_at: utc(8, 27, 0, 0),
        created_by: Actor::system("delivery"),
    };
    let first = service
        .store()
        .create_checkpoint(auto(&auto_checkpoint_name(week(0).date_naive())))
        .await
        .expect("auto");
    assert!(matches!(first, CheckpointCreated::Created(_)));
    let again = service
        .store()
        .create_checkpoint(auto("week 2026-08-28 start"))
        .await
        .expect("auto again");
    assert_eq!(
        again,
        CheckpointCreated::Existing(first.checkpoint().clone())
    );
    let same_name = service
        .store()
        .create_checkpoint(NewCheckpoint {
            week: week(0) + TimeDelta::hours(8),
            ..auto(&auto_checkpoint_name(week(0).date_naive()))
        })
        .await
        .expect("auto collision");
    assert_eq!(
        same_name,
        CheckpointCreated::Existing(first.checkpoint().clone()),
        "the same local week under another reset instant is the same checkpoint"
    );
    let midweek = service
        .create_checkpoint("root", "midweek", week(0) + TimeDelta::days(2), &policy())
        .await
        .expect("normalised");
    assert_eq!(midweek.week, week(0), "normalised to the boss-week start");
    let listed = service
        .store()
        .list_checkpoints(Some(week(0)))
        .await
        .expect("list");
    assert_eq!(listed, [named.clone(), first.checkpoint().clone(), midweek]);
    assert!(
        service
            .store()
            .list_checkpoints(Some(week(1)))
            .await
            .expect("list")
            .is_empty()
    );
    assert_eq!(
        service
            .store()
            .load_checkpoint("before the raid")
            .await
            .expect("load"),
        Some(named)
    );
}

async fn restoring_a_checkpoint_previews_then_applies<
    S: ScheduleStore + ChangeHistory + BlameIndex + Checkpoints,
>(
    store: S,
) {
    let Fixture {
        mut service, fixed, ..
    } = fixture(store).await;
    let run = run_of(&service, &fixed[0], 0).await;
    let next_week = run_of(&service, &fixed[0], 1).await;
    let checkpoint = service
        .create_checkpoint("root", "clean week", week(0), &policy())
        .await
        .expect("checkpoint");
    let clean = snapshot(&service).await;
    service
        .as_origin(member("2"))
        .amend_run(&run, utc(8, 29, 22, 0), &policy())
        .await
        .expect("amend");
    service
        .as_origin(member("2"))
        .set_rsvp(&next_week, "2", RsvpState::No, RsvpSource::Chat)
        .await
        .expect("other week");
    let changed = snapshot(&service).await;
    let count = all_records(service.store()).await.len();

    let preview = service
        .preview_checkpoint_restore(
            "clean week",
            RevertMode::Strict,
            &policy().reminders,
            &BTreeSet::new(),
        )
        .await
        .expect("preview");
    assert!(
        matches!(&preview, RevertOutcome::Reverted { seqs, .. } if seqs.len() == 1),
        "restoring_a_checkpoint_previews_then_applies: {preview:?}"
    );
    assert_eq!(
        snapshot(&service).await,
        changed,
        "a preview changes nothing"
    );
    assert_eq!(
        all_records(service.store()).await.len(),
        count,
        "and records nothing"
    );

    service
        .restore_to_checkpoint(
            "root",
            unique_request(),
            "clean week",
            RevertMode::Strict,
            &policy().reminders,
            &BTreeSet::new(),
        )
        .await
        .expect("restore");
    let after = snapshot(&service).await;
    let find =
        |state: &ScheduleSnapshot, id: &str| state.runs.iter().find(|row| row.id == id).cloned();
    assert_eq!(find(&after, &run), find(&clean, &run));
    assert!(
        after
            .rsvps
            .iter()
            .any(|row| row.run_id == next_week && row.user_id == "2"),
        "other weeks keep their changes"
    );
    let record = latest(service.store()).await;
    assert_eq!(
        record.refs.last(),
        Some(&checkpoint.head),
        "{:?}",
        record.refs
    );
    let blamed = blame(service.store(), &BlameTarget::Run(run.clone()))
        .await
        .expect("blame")
        .expect("run");
    let slot = line(&blamed, "slot").expect("slot");
    assert!(
        matches!(&slot.via, Via::Rollback { undid, restored_to: Some(head) }
            if *head == checkpoint.head && !undid.contains(head) && undid.len() == 1),
        "the checkpoint head is not an undone change: {:?}",
        slot.via
    );

    service
        .create_checkpoint("root", "nothing since", week(0), &policy())
        .await
        .expect("checkpoint");
    let nothing = service
        .preview_checkpoint_restore(
            "nothing since",
            RevertMode::Strict,
            &policy().reminders,
            &BTreeSet::new(),
        )
        .await;
    assert_eq!(
        nothing,
        Ok(RevertOutcome::Unchanged {
            seqs: Vec::new(),
            skipped: Vec::new(),
        }),
        "nothing after the checkpoint: unchanged, not an error"
    );

    let unknown = service
        .restore_to_checkpoint(
            "root",
            unique_request(),
            "no such checkpoint",
            RevertMode::Strict,
            &policy().reminders,
            &BTreeSet::new(),
        )
        .await;
    assert_eq!(
        unknown,
        Err(SchedulerError::History(HistoryRefusal::UnknownCheckpoint(
            "no such checkpoint".into()
        )))
    );
}
