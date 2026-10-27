//! v5 attendance every store must keep (standing answers and the timing
//! default stored with their timing, recorded attendance stored with its
//! run, status pins), driven through [`SchedulerService`] against a fresh
//! store per check.
//! Failures panic with the check name.

use chrono::{DateTime, Utc, Weekday};

use super::conformance::{seed_attendance, seed_run_attendance};
use super::fixtures::{self, Ids, Stopped, admin, member, policy_in, utc2026};
use kanade_domain::attendance::AttendanceDefault;
use kanade_domain::history::{
    ATTENDANCE_DEFAULT_FIELD, Actor, BlameIndex, BlameTarget, ChangeHistory, Expect, Origin,
    Precondition, RowValue, STATUS_PIN_FIELD, Surface, attended_field, blame, standing_field,
};
use kanade_domain::members::{Member, Roster};
use kanade_domain::schedule::{
    EMOJI_NO, FixedEdit, FixedRun, Run, RunStatus, SchedulePolicy, StatusChange,
};
use kanade_domain::scheduler::{ScheduleStore, SchedulerError, SchedulerService, Scope};

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<S: ScheduleStore + ChangeHistory + BlameIndex + Sync>(
    make: impl AsyncFn() -> S,
) {
    leaving_the_party_removes_the_standing_answer(make().await).await;
    versioned_reads_carry_the_attendance_fields(make().await).await;
    recorded_attendance_follows_the_run(make().await).await;
    status_pins_are_recorded_blamed_and_guarded(make().await).await;
}

type Service<S> = SchedulerService<S, Ids, Stopped>;

fn policy() -> SchedulePolicy {
    policy_in(chrono_tz::UTC)
}

fn roster() -> Roster {
    let mut roster = Roster::new();
    for id in ["1", "2", "3"] {
        roster.upsert(Member {
            user_id: id.into(),
            has_role: true,
            ..Member::default()
        });
    }
    roster
}

/// Thursday 27 Aug 01:00, the service clock.
fn now() -> DateTime<Utc> {
    utc2026(8, 27, 1, 0)
}

/// A Saturday timing with party 1 and 2, materialised.
async fn fixture<S: ScheduleStore>(store: S) -> (Service<S>, String) {
    let mut service = SchedulerService::new(store, Ids::default(), Stopped(now()));
    let fixed = service
        .as_origin(admin())
        .add_fixed_run(fixtures::timing(Weekday::Sat))
        .await
        .expect("timing");
    service
        .as_origin(admin())
        .materialise_weeks(&policy())
        .await
        .expect("materialise");
    (service, fixed)
}

async fn timing<S: ScheduleStore>(service: &Service<S>, id: &str) -> Option<FixedRun> {
    service
        .store()
        .load(&Scope::Weeks(Vec::new()))
        .await
        .expect("load")
        .fixed_runs
        .into_iter()
        .find(|row| row.id == id)
}

fn standing_users(row: &FixedRun) -> Vec<String> {
    row.standing
        .iter()
        .map(|answer| answer.user_id.clone())
        .collect()
}

async fn last_set<S: ScheduleStore + ChangeHistory + BlameIndex>(
    service: &Service<S>,
    fixed: &str,
    field: &str,
) -> Option<(u64, Actor)> {
    blame(service.store(), &BlameTarget::FixedRun(fixed.to_owned()))
        .await
        .expect("blame")
        .and_then(|blamed| {
            blamed
                .lines
                .into_iter()
                .find(|line| line.field == field)
                .and_then(|line| line.last)
                .map(|last| (last.seq, last.actor))
        })
}

async fn leaving_the_party_removes_the_standing_answer<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (mut service, fixed) = fixture(store).await;
    seed_attendance(
        service.store(),
        &fixed,
        &["1", "2"],
        AttendanceDefault::OptIn,
    )
    .await;
    // A party delta (a request's leave) drops 2's answer in its commit.
    service
        .as_origin(admin())
        .change_fixed_party(&fixed, &[], &["2".into()], &roster(), &policy())
        .await
        .expect("leave: delta");
    let left = service.store().history_head().await.expect("head");
    assert_eq!(
        standing_users(&timing(&service, &fixed).await.expect("timing")),
        ["1"],
        "leave: delta"
    );
    assert_eq!(
        last_set(&service, &fixed, &standing_field("2")).await,
        Some((left.seq, Actor::admin("root"))),
        "leave: recorded with the party change"
    );
    // A party edit drops it too.
    service
        .as_origin(admin())
        .update_fixed(
            &fixed,
            FixedEdit {
                participants: Some(vec!["2".into(), "3".into()]),
                ..FixedEdit::default()
            },
            &roster(),
            &policy(),
        )
        .await
        .expect("leave: edit");
    assert!(
        timing(&service, &fixed)
            .await
            .expect("timing")
            .standing
            .is_empty(),
        "leave: edit"
    );
    // Retiring the timing takes its answers with it.
    seed_attendance(service.store(), &fixed, &["2"], AttendanceDefault::OptIn).await;
    let weeks: Vec<DateTime<Utc>> = Vec::new();
    service
        .as_origin(admin())
        .retire_fixed_run(&fixed, &weeks, &policy().reminders)
        .await
        .expect("leave: retire");
    assert!(timing(&service, &fixed).await.is_none(), "leave: retired");
    assert!(
        service
            .store()
            .verify_history()
            .await
            .expect("verify")
            .is_intact(),
        "leave: the history verifies"
    );
}

async fn versioned_reads_carry_the_attendance_fields<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (service, fixed) = fixture(store).await;
    seed_attendance(
        service.store(),
        &fixed,
        &["2"],
        AttendanceDefault::AssumeComing,
    )
    .await;
    let read = service
        .store()
        .read_versioned(&[BlameTarget::FixedRun(fixed.clone())])
        .await
        .expect("versioned");
    let Some(RowValue::FixedRun(row)) = &read[0].row else {
        panic!("versioned: no timing row");
    };
    assert_eq!(
        row.attendance_default,
        AttendanceDefault::AssumeComing,
        "versioned"
    );
    assert_eq!(standing_users(row), ["2"], "versioned");
    assert!(
        read[0].versions.contains_key(ATTENDANCE_DEFAULT_FIELD),
        "versioned"
    );
    assert!(
        read[0].versions.contains_key(&standing_field("2")),
        "versioned"
    );
}

/// The fixture's runs (three weeks of the timing), oldest first.
async fn runs<S: ScheduleStore>(service: &Service<S>) -> Vec<Run> {
    service.store().load(&Scope::All).await.expect("load").runs
}

async fn mark_done<S: ScheduleStore>(service: &mut Service<S>, run_id: &str) {
    service
        .as_origin(admin())
        .set_status(
            run_id,
            StatusChange {
                status: RunStatus::Done,
                announce: false,
                via_portal: false,
            },
            &policy().reminders,
        )
        .await
        .expect("done");
}

fn attendance_of(run: &Run) -> Vec<(String, bool, String)> {
    run.attendance
        .iter()
        .map(|record| {
            (
                record.user_id.clone(),
                record.attended,
                record.recorded_by.clone(),
            )
        })
        .collect()
}

async fn run_blame<S: ScheduleStore + ChangeHistory + BlameIndex>(
    service: &Service<S>,
    target: &BlameTarget,
    field: String,
) -> Option<(u64, Actor)> {
    blame(service.store(), target)
        .await
        .expect("blame")
        .expect("run")
        .lines
        .into_iter()
        .find(|line| line.field == field)
        .and_then(|line| line.last)
        .map(|last| (last.seq, last.actor))
}

/// Entries never outlive what they describe: a swap (allowed on a done
/// run) drops the leaver's entry in its commit, and reviving the run from
/// done clears them all.
async fn recorded_attendance_follows_the_run<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (mut service, _) = fixture(store).await;
    let run = runs(&service).await[0].id.clone();
    mark_done(&mut service, &run).await;
    seed_run_attendance(service.store(), &run, &[("1", true), ("2", true)]).await;
    let recorded = service.store().history_head().await.expect("head");
    let target = BlameTarget::Run(run.clone());
    assert_eq!(
        run_blame(&service, &target, attended_field("2")).await,
        Some((recorded.seq, Actor::admin("conformance"))),
        "follows: recorded entries are blamed"
    );
    let read = service
        .store()
        .read_versioned(std::slice::from_ref(&target))
        .await
        .expect("versioned");
    let Some(RowValue::Run(row)) = &read[0].row else {
        panic!("follows: no run row");
    };
    assert_eq!(row.attendance.len(), 2, "follows: versioned");
    assert_eq!(
        read[0].versions.get(&attended_field("1")),
        Some(&recorded.seq),
        "follows: versioned"
    );
    let state = service
        .as_origin(admin())
        .swap_participants(&run, &["2".into()], &["3".into()], true, &roster())
        .await
        .expect("follows: swap on a done run")
        .value;
    assert_eq!(state.run.status, RunStatus::Done, "follows: still done");
    assert_eq!(
        attendance_of(&state.run),
        [("1".to_owned(), true, "admin:conformance".to_owned())],
        "follows: the leaver's entry is dropped"
    );
    let swapped = service.store().history_head().await.expect("head");
    assert_eq!(
        run_blame(&service, &target, attended_field("2")).await,
        Some((swapped.seq, Actor::admin("root"))),
        "follows: the drop is recorded with the swap"
    );
    // Reviving the run clears what it recorded.
    service
        .as_origin(admin())
        .set_status(
            &run,
            StatusChange {
                status: RunStatus::Planned,
                announce: false,
                via_portal: false,
            },
            &policy().reminders,
        )
        .await
        .expect("follows: revive");
    assert!(
        runs(&service).await[0].attendance.is_empty(),
        "follows: revived runs record nothing"
    );
    assert!(
        service
            .store()
            .verify_history()
            .await
            .expect("verify")
            .is_intact(),
        "follows: the history verifies"
    );
}

/// v5: a hand-set status is pinned on the run with history and blame
/// (`status_pin`), guarded like any field, and an explicit answer ends it.
async fn status_pins_are_recorded_blamed_and_guarded<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (mut service, _) = fixture(store).await;
    let run = runs(&service).await[0].id.clone();
    let target = BlameTarget::Run(run.clone());
    let confirm = StatusChange {
        status: RunStatus::Confirmed,
        announce: false,
        via_portal: true,
    };
    let view = Expect::fields([Precondition::new(target.clone(), STATUS_PIN_FIELD, None)]);
    service
        .as_origin(admin())
        .expecting(view.clone())
        .set_status(&run, confirm, &policy().reminders)
        .await
        .expect("pin: set");
    let pinned = service.store().history_head().await.expect("head");
    let row = runs(&service).await[0].clone();
    assert_eq!(
        row.status_pin.map(|pin| (pin.status, pin.at)),
        Some((RunStatus::Confirmed, now())),
        "pin: stored"
    );
    assert_eq!(
        run_blame(&service, &target, STATUS_PIN_FIELD.to_owned()).await,
        Some((pinned.seq, Actor::admin("root"))),
        "pin: blamed"
    );
    let read = service
        .store()
        .read_versioned(std::slice::from_ref(&target))
        .await
        .expect("versioned");
    let Some(RowValue::Run(versioned)) = &read[0].row else {
        panic!("pin: no run row");
    };
    assert_eq!(versioned.status_pin, row.status_pin, "pin: versioned");
    assert_eq!(
        read[0].versions.get(STATUS_PIN_FIELD),
        Some(&pinned.seq),
        "pin: versioned"
    );
    // Another admin working from the unpinned view is stale.
    assert!(
        matches!(
            service
                .as_origin(Origin::new(Actor::admin("other"), Surface::AdminPortal))
                .expecting(view)
                .set_status(
                    &run,
                    StatusChange {
                        status: RunStatus::Planned,
                        ..confirm
                    },
                    &policy().reminders
                )
                .await,
            Err(SchedulerError::StaleEdit { .. })
        ),
        "pin: stale view"
    );
    // An explicit answer ends it in its own commit.
    service
        .as_origin(member("2"))
        .apply_reaction(&run, "2", EMOJI_NO, true)
        .await
        .expect("pin: answer");
    let answered = service.store().history_head().await.expect("head");
    let row = runs(&service).await[0].clone();
    assert_eq!(
        (row.status, row.status_pin),
        (RunStatus::AtRisk, None),
        "pin: ended"
    );
    assert_eq!(
        run_blame(&service, &target, STATUS_PIN_FIELD.to_owned()).await,
        Some((answered.seq, Actor::member("2"))),
        "pin: the end is blamed"
    );
    assert!(
        service
            .store()
            .verify_history()
            .await
            .expect("verify")
            .is_intact(),
        "pin: the history verifies"
    );
}
