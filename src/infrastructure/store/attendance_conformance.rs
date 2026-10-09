//! v5 attendance writes every store must keep (standing answers, the
//! timing default, recorded attendance and the member history query),
//! driven through [`SchedulerService`] against a fresh store per check.
//! Failures panic with the check name.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};

use crate::domain::attendance::{AttendanceDefault, AttendancePolicy, AttendanceRefusal, Patterns};
use crate::domain::history::{
    ATTENDANCE_DEFAULT_FIELD, Actor, BlameIndex, BlameTarget, ChangeHistory, Expect, Origin,
    Precondition, RowValue, STATUS_PIN_FIELD, Surface, attended_field, blame, standing_field,
};
use crate::domain::ids::IdGenerator;
use crate::domain::members::{Member, Roster};
use crate::domain::schedule::{
    EMOJI_NO, FixedEdit, FixedRun, NewFixedRun, ReminderPolicy, RsvpSource, RsvpState, Run,
    RunStatus, ScheduleError, SchedulePolicy, StatusChange,
};
use crate::domain::scheduler::{
    AttendanceHistory, Clock, ScheduleStore, SchedulerError, SchedulerService, Scope,
};

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<S: ScheduleStore + ChangeHistory + BlameIndex + AttendanceHistory + Sync>(
    make: impl AsyncFn() -> S,
) {
    standing_answers_set_clear_and_blame(make().await).await;
    standing_answers_are_for_party_members_themselves(make().await).await;
    leaving_the_party_removes_the_standing_answer(make().await).await;
    the_attendance_default_is_admin_only_blamed_and_guarded(make().await).await;
    versioned_reads_carry_the_attendance_fields(make().await).await;
    recording_attendance_checks_permissions_and_is_recorded(make().await).await;
    member_patterns_and_the_standing_suggestion(make().await).await;
    recorded_attendance_follows_the_run(make().await).await;
    status_pins_are_recorded_blamed_and_guarded(make().await).await;
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
struct Now;

impl Clock for Now {
    fn now(&self) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 27, 1, 0, 0)
            .single()
            .expect("valid instant")
    }
}

type Service<S> = SchedulerService<S, Ids, Now>;

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
            has_role: true,
            ..Member::default()
        });
    }
    roster
}

/// A Saturday timing with party 1 and 2, materialised.
async fn fixture<S: ScheduleStore>(store: S) -> (Service<S>, String) {
    let mut service = SchedulerService::new(store, Ids::default(), Now);
    let fixed = service
        .as_origin(admin())
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

async fn standing_answers_set_clear_and_blame<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (mut service, fixed) = fixture(store).await;
    let row = service
        .as_origin(member("2"))
        .set_standing_answer("2", &fixed, true)
        .await
        .expect("standing: set");
    assert_eq!(standing_users(&row), ["2"], "standing");
    assert_eq!(row.standing[0].set_by, "member:2", "standing: who");
    let set = service.store().history_head().await.expect("head");
    assert_eq!(
        timing(&service, &fixed).await.expect("timing").standing,
        row.standing,
        "standing: stored"
    );
    assert_eq!(
        last_set(&service, &fixed, &standing_field("2")).await,
        Some((set.seq, Actor::member("2"))),
        "standing: blamed"
    );
    // Setting it again changes nothing and records nothing.
    service
        .as_origin(member("2"))
        .set_standing_answer("2", &fixed, true)
        .await
        .expect("standing: again");
    assert_eq!(
        service.store().history_head().await.expect("head"),
        set,
        "standing: idempotent"
    );
    service
        .as_origin(member("2"))
        .set_standing_answer("2", &fixed, false)
        .await
        .expect("standing: clear");
    let cleared = service.store().history_head().await.expect("head");
    assert!(
        timing(&service, &fixed)
            .await
            .expect("timing")
            .standing
            .is_empty(),
        "standing: cleared"
    );
    assert_eq!(
        last_set(&service, &fixed, &standing_field("2")).await,
        Some((cleared.seq, Actor::member("2"))),
        "standing: the clear is blamed"
    );
}

async fn standing_answers_are_for_party_members_themselves<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (mut service, fixed) = fixture(store).await;
    assert!(
        matches!(
            service
                .as_origin(member("1"))
                .set_standing_answer("2", &fixed, true)
                .await,
            Err(SchedulerError::Forbidden(_))
        ),
        "standing: not for someone else"
    );
    assert!(
        matches!(
            service
                .as_origin(member("3"))
                .set_standing_answer("3", &fixed, true)
                .await,
            Err(SchedulerError::Schedule(ScheduleError::NotInParty { .. }))
        ),
        "standing: only party members"
    );
    let row = service
        .as_origin(admin())
        .set_standing_answer("1", &fixed, true)
        .await
        .expect("standing: an admin for a member");
    assert_eq!(row.standing[0].set_by, "admin:root", "standing");
}

async fn leaving_the_party_removes_the_standing_answer<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (mut service, fixed) = fixture(store).await;
    for user in ["1", "2"] {
        service
            .as_origin(member(user))
            .set_standing_answer(user, &fixed, true)
            .await
            .expect("leave: set");
    }
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
    service
        .as_origin(member("2"))
        .set_standing_answer("2", &fixed, true)
        .await
        .expect("leave: set again");
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

async fn the_attendance_default_is_admin_only_blamed_and_guarded<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (mut service, fixed) = fixture(store).await;
    assert_eq!(
        timing(&service, &fixed)
            .await
            .expect("timing")
            .attendance_default,
        AttendanceDefault::OptIn,
        "default: opt-in to start"
    );
    assert!(
        matches!(
            service
                .as_origin(member("1"))
                .set_attendance_default(&fixed, AttendanceDefault::AssumeComing)
                .await,
            Err(SchedulerError::Forbidden(_))
        ),
        "default: admin only"
    );
    let target = BlameTarget::FixedRun(fixed.clone());
    let seen = service
        .store()
        .last_changes(&target)
        .await
        .expect("versions")
        .get(ATTENDANCE_DEFAULT_FIELD)
        .copied();
    assert_eq!(seen, None, "default: no version yet");
    let view = Expect::fields([Precondition::new(
        target.clone(),
        ATTENDANCE_DEFAULT_FIELD,
        seen,
    )]);
    service
        .as_origin(admin())
        .expecting(view.clone())
        .set_attendance_default(&fixed, AttendanceDefault::AssumeComing)
        .await
        .expect("default: set");
    let set = service.store().history_head().await.expect("head");
    assert_eq!(
        timing(&service, &fixed)
            .await
            .expect("timing")
            .attendance_default,
        AttendanceDefault::AssumeComing,
        "default: stored"
    );
    assert_eq!(
        last_set(&service, &fixed, ATTENDANCE_DEFAULT_FIELD).await,
        Some((set.seq, Actor::admin("root"))),
        "default: blamed"
    );
    // A second admin working from the same view is stale.
    assert!(
        matches!(
            service
                .as_origin(Origin::new(Actor::admin("other"), Surface::AdminPortal))
                .expecting(view)
                .set_attendance_default(&fixed, AttendanceDefault::OptIn)
                .await,
            Err(SchedulerError::StaleEdit { .. })
        ),
        "default: stale view"
    );
    // Preconditions on a standing answer work the same way.
    let standing = Expect::fields([Precondition::new(target, standing_field("2"), None)]);
    service
        .as_origin(member("2"))
        .expecting(standing.clone())
        .set_standing_answer("2", &fixed, true)
        .await
        .expect("default: standing with a fresh view");
    assert!(
        matches!(
            service
                .as_origin(member("2"))
                .expecting(standing)
                .set_standing_answer("2", &fixed, false)
                .await,
            Err(SchedulerError::StaleEdit { .. })
        ),
        "default: stale standing view"
    );
}

async fn versioned_reads_carry_the_attendance_fields<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (mut service, fixed) = fixture(store).await;
    service
        .as_origin(member("2"))
        .set_standing_answer("2", &fixed, true)
        .await
        .expect("versioned: standing");
    service
        .as_origin(admin())
        .set_attendance_default(&fixed, AttendanceDefault::AssumeComing)
        .await
        .expect("versioned: default");
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

fn set(users: &[&str]) -> BTreeSet<String> {
    users.iter().map(|user| (*user).to_owned()).collect()
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

async fn recording_attendance_checks_permissions_and_is_recorded<
    S: ScheduleStore + ChangeHistory + BlameIndex + Sync,
>(
    store: S,
) {
    let (mut service, _) = fixture(store).await;
    let run = runs(&service).await[0].id.clone();
    // 2 declined; the prefill is everyone not declined.
    service
        .as_origin(member("2"))
        .set_rsvp(&run, "2", RsvpState::No, RsvpSource::Chat)
        .await
        .expect("attendance: answer");
    assert!(
        matches!(
            service
                .as_origin(admin())
                .record_attendance(&run, &set(&["1"]))
                .await,
            Err(SchedulerError::Schedule(ScheduleError::Attendance(
                AttendanceRefusal::NotDone
            )))
        ),
        "attendance: only once done"
    );
    mark_done(&mut service, &run).await;
    let head = service.store().history_head().await.expect("head");
    assert!(
        matches!(
            service
                .as_origin(member("1"))
                .record_attendance(&run, &set(&["1", "2"]))
                .await,
            Err(SchedulerError::Forbidden(_))
        ),
        "attendance: a member cannot mark someone else"
    );
    assert!(
        matches!(
            service
                .as_origin(Origin::new(
                    Actor::system("delivery"),
                    Surface::DeliveryTick
                ))
                .record_attendance(&run, &set(&["1"]))
                .await,
            Err(SchedulerError::Forbidden(_))
        ),
        "attendance: not the system"
    );
    assert!(
        matches!(
            service
                .as_origin(admin())
                .record_attendance(&run, &set(&["1", "3"]))
                .await,
            Err(SchedulerError::Schedule(ScheduleError::Attendance(
                AttendanceRefusal::NotOnRun(_)
            )))
        ),
        "attendance: only people on the run"
    );
    assert_eq!(
        service.store().history_head().await.expect("head"),
        head,
        "attendance: refusals record nothing"
    );
    // The member confirms their own entry from the prefill ({1}): only
    // their entry is written.
    let state = service
        .as_origin(member("2"))
        .record_attendance(&run, &set(&["1", "2"]))
        .await
        .expect("attendance: own entry");
    assert_eq!(
        attendance_of(&state.run),
        [("2".to_owned(), true, "member:2".to_owned())],
        "attendance: a member writes only their own"
    );
    let own = service.store().history_head().await.expect("head");
    let target = BlameTarget::Run(run.clone());
    assert_eq!(
        run_blame(&service, &target, attended_field("2")).await,
        Some((own.seq, Actor::member("2"))),
        "attendance: blamed"
    );
    // The admin records the whole roster: 1 absent; 2 unchanged keeps the
    // member's entry.
    let seen = service
        .store()
        .last_changes(&target)
        .await
        .expect("versions")
        .get(&attended_field("1"))
        .copied();
    assert_eq!(seen, None, "attendance: 1 not recorded yet");
    let view = Expect::fields([Precondition::new(target.clone(), attended_field("1"), seen)]);
    let state = service
        .as_origin(admin())
        .expecting(view.clone())
        .record_attendance(&run, &set(&["2"]))
        .await
        .expect("attendance: admin");
    assert_eq!(
        attendance_of(&state.run),
        [
            ("1".to_owned(), false, "admin:root".to_owned()),
            ("2".to_owned(), true, "member:2".to_owned()),
        ],
        "attendance: admin roster"
    );
    let recorded = service.store().history_head().await.expect("head");
    assert_eq!(
        run_blame(&service, &target, attended_field("1")).await,
        Some((recorded.seq, Actor::admin("root"))),
        "attendance: admin blamed"
    );
    assert_eq!(
        run_blame(&service, &target, attended_field("2")).await,
        Some((own.seq, Actor::member("2"))),
        "attendance: an unchanged entry keeps its attribution"
    );
    // A second admin working from the same view is stale.
    assert!(
        matches!(
            service
                .as_origin(Origin::new(Actor::admin("other"), Surface::AdminPortal))
                .expecting(view)
                .record_attendance(&run, &set(&["1", "2"]))
                .await,
            Err(SchedulerError::StaleEdit { .. })
        ),
        "attendance: stale view"
    );
    // The stored row, its versions and the change record agree.
    let read = service
        .store()
        .read_versioned(std::slice::from_ref(&target))
        .await
        .expect("versioned");
    let Some(RowValue::Run(row)) = &read[0].row else {
        panic!("attendance: no run row");
    };
    assert_eq!(
        row.attendance, state.run.attendance,
        "attendance: versioned"
    );
    assert_eq!(
        read[0].versions.get(&attended_field("1")),
        Some(&recorded.seq),
        "attendance: versioned"
    );
    assert_eq!(
        runs(&service).await[0].attendance,
        state.run.attendance,
        "attendance: stored"
    );
    assert!(
        service
            .store()
            .verify_history()
            .await
            .expect("verify")
            .is_intact(),
        "attendance: the history verifies"
    );
}

async fn member_patterns_and_the_standing_suggestion<
    S: ScheduleStore + ChangeHistory + BlameIndex + AttendanceHistory + Sync,
>(
    store: S,
) {
    let (mut service, fixed) = fixture(store).await;
    let ids: Vec<String> = runs(&service).await.into_iter().map(|run| run.id).collect();
    // Member 1 said no on the oldest run and came anyway; member 2 never
    // answered and came to all three.
    service
        .as_origin(member("1"))
        .set_rsvp(&ids[0], "1", RsvpState::No, RsvpSource::Chat)
        .await
        .expect("patterns: answer");
    for id in &ids {
        mark_done(&mut service, id).await;
        service
            .as_origin(admin())
            .record_attendance(id, &set(&["1", "2"]))
            .await
            .expect("patterns: record");
    }
    // Only the member themselves or an administrator may read them.
    assert!(
        matches!(
            service.member_attendance(&Actor::member("1"), "2").await,
            Err(SchedulerError::Forbidden(_))
        ),
        "patterns: private"
    );
    let two = service
        .member_attendance(&Actor::member("2"), "2")
        .await
        .expect("patterns: own");
    let newest_first: Vec<String> = ids.iter().rev().cloned().collect();
    assert_eq!(
        two.history
            .iter()
            .map(|run| run.run_id.clone())
            .collect::<Vec<_>>(),
        newest_first,
        "patterns: newest first"
    );
    assert_eq!(
        two.patterns,
        Patterns {
            attended_without_answering: 3,
            answered_in_but_absent: 0,
            answered_out_but_attended: 0,
        },
        "patterns: 2"
    );
    assert_eq!(
        two.suggest_standing,
        std::slice::from_ref(&fixed),
        "patterns: suggested"
    );
    let one = service
        .member_attendance(&Actor::admin("root"), "1")
        .await
        .expect("patterns: admin");
    assert_eq!(
        one.patterns,
        Patterns {
            attended_without_answering: 2,
            answered_in_but_absent: 0,
            answered_out_but_attended: 1,
        },
        "patterns: 1"
    );
    assert!(
        one.suggest_standing.is_empty(),
        "patterns: 2 of 3 is not enough"
    );
    // A suggestion is never applied, and goes once the answer is set.
    assert!(
        timing(&service, &fixed)
            .await
            .expect("timing")
            .standing
            .is_empty(),
        "patterns: not applied"
    );
    service
        .as_origin(member("2"))
        .set_standing_answer("2", &fixed, true)
        .await
        .expect("patterns: standing");
    assert!(
        service
            .member_attendance(&Actor::member("2"), "2")
            .await
            .expect("patterns: again")
            .suggest_standing
            .is_empty(),
        "patterns: already set"
    );
    // At most the last runs per timing are read.
    let limited = service
        .store()
        .member_history("2", 2)
        .await
        .expect("history");
    assert_eq!(
        limited
            .iter()
            .map(|run| run.run_id.clone())
            .collect::<Vec<_>>(),
        newest_first[..2],
        "patterns: per-timing limit"
    );
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
    service
        .as_origin(admin())
        .record_attendance(&run, &set(&["1", "2"]))
        .await
        .expect("follows: record");
    let state = service
        .as_origin(admin())
        .swap_participants(&run, &["2".into()], &["3".into()], true, &roster())
        .await
        .expect("follows: swap on a done run")
        .value;
    assert_eq!(state.run.status, RunStatus::Done, "follows: still done");
    assert_eq!(
        attendance_of(&state.run),
        [("1".to_owned(), true, "admin:root".to_owned())],
        "follows: the leaver's entry is dropped"
    );
    let swapped = service.store().history_head().await.expect("head");
    let target = BlameTarget::Run(run.clone());
    assert_eq!(
        run_blame(&service, &target, attended_field("2")).await,
        Some((swapped.seq, Actor::admin("root"))),
        "follows: the drop is recorded with the swap"
    );
    // The newcomer can be recorded like anyone on the run.
    let state = service
        .as_origin(member("3"))
        .record_attendance(&run, &set(&["1", "3"]))
        .await
        .expect("follows: newcomer");
    assert_eq!(state.run.attendance.len(), 2, "follows: newcomer");
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
    let (service, _) = fixture(store).await;
    let mut service = service.with_attendance(AttendancePolicy::V5);
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
        Some((RunStatus::Confirmed, Now.now())),
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
