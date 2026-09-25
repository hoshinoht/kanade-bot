//! v5 attendance on the write path: status fully derived from standing
//! answers, the timing default and the unknown window (re-derived in the
//! same commit as the change), one source for the attendance rules across
//! reactions, draft merges and cherry-picks, and the same actions in
//! v4-compat mode (which must stay v4).

use chrono::{DateTime, FixedOffset, NaiveTime, TimeDelta, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::domain::attendance::{
    AnswerState, AssumedSource, AttendanceDefault, AttendancePolicy, StatusLabel, Tally,
    snapshot_states, status_label,
};
use kanade::domain::drafts::{DraftOp, Target};
use kanade::domain::history::{
    Actor, BlameTarget, ChangeHistory, Origin, PickMode, PickRefusal, Surface, blame,
};
use kanade::domain::ids::IdGenerator;
use kanade::domain::members::{Directory, Member};
use kanade::domain::schedule::{
    EMOJI_NO, EMOJI_YES, FixedEdit, NewFixedRun, ReminderPolicy, Run, RunStatus, ScheduleError,
    SchedulePolicy, ScheduleSnapshot, StatusChange,
};
use kanade::domain::scheduler::{
    DraftError, PickError, ScheduleStore, SchedulerError, SchedulerService, Scope,
};
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::common::TestClock;

#[derive(Default)]
struct CountingIds(u32);

impl IdGenerator for CountingIds {
    fn new_id(&mut self) -> String {
        self.0 += 1;
        format!("id-{:04}", self.0)
    }
}

struct Guild;

impl Directory for Guild {
    fn member(&self, user_id: &str) -> Option<Member> {
        ["1001", "1002", "1003"].contains(&user_id).then(|| Member {
            user_id: user_id.to_owned(),
            has_role: true,
            ..Member::default()
        })
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        channel_id == "222"
    }
}

type Service = SchedulerService<MemoryScheduleStore, CountingIds, TestClock>;

fn kl(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
}

fn policy(attendance: AttendancePolicy) -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
    .with_attendance(attendance)
}

fn admin() -> Origin {
    Origin::new(Actor::admin("root"), Surface::AdminPortal)
}

fn member(id: &str) -> Origin {
    Origin::new(Actor::member(id), Surface::Discord)
}

fn tick() -> Origin {
    Origin::new(Actor::system("delivery"), Surface::DeliveryTick)
}

struct Fixture {
    service: Service,
    clock: TestClock,
    attendance: AttendancePolicy,
    fixed: String,
    /// Runs by week: Sat 29 Aug, 5 Sep and 12 Sep, 20:00 in Kuala Lumpur.
    runs: [String; 3],
}

impl Fixture {
    fn run(&self) -> &str {
        &self.runs[0]
    }
}

/// Weekly Sat 20:00 with `party`, materialised on Thu 27 Aug.
async fn fixture_with(attendance: AttendancePolicy, party: &[&str]) -> Fixture {
    let clock = TestClock::new(kl(8, 27, 1, 0));
    let mut service = SchedulerService::new(
        MemoryScheduleStore::new(),
        CountingIds::default(),
        clock.clone(),
    )
    .with_attendance(attendance);
    let fixed = service
        .as_origin(admin())
        .add_fixed_run(NewFixedRun {
            owner_id: "1001".into(),
            channel_id: Some("222".into()),
            bosses: vec!["HFA".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(20, 0, 0).unwrap(),
            participants: party.iter().map(|user| (*user).to_owned()).collect(),
            note: None,
        })
        .await
        .unwrap();
    service
        .as_origin(admin())
        .materialise_weeks(&policy(attendance))
        .await
        .unwrap();
    let state = snapshot(&service).await;
    let run_at = |day: DateTime<FixedOffset>| {
        state
            .runs
            .iter()
            .find(|run| run.datetime == day.with_timezone(&Utc))
            .unwrap()
            .id
            .clone()
    };
    let runs = [
        run_at(kl(8, 29, 20, 0)),
        run_at(kl(9, 5, 20, 0)),
        run_at(kl(9, 12, 20, 0)),
    ];
    Fixture {
        service,
        clock,
        attendance,
        fixed,
        runs,
    }
}

async fn fixture(attendance: AttendancePolicy) -> Fixture {
    fixture_with(attendance, &["1001", "1002"]).await
}

async fn snapshot(service: &Service) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.unwrap()
}

async fn run_row(service: &Service, run: &str) -> Run {
    snapshot(service)
        .await
        .runs
        .into_iter()
        .find(|row| row.id == run)
        .unwrap()
}

async fn status(service: &Service, run: &str) -> RunStatus {
    run_row(service, run).await.status
}

async fn head(service: &Service) -> u64 {
    service.store().history_head().await.unwrap().seq
}

/// The run's tally and whether it reads "expected" (confirmed on assumed
/// answers), which must agree with its status.
async fn tally(f: &Fixture, run: &str) -> (Tally, bool) {
    let state = snapshot(&f.service).await;
    let row = state.runs.iter().find(|row| row.id == run).unwrap();
    let states = snapshot_states(&state, row, f.attendance.mode);
    let tally = Tally::of(states.iter().map(|(_, state)| state));
    let coming = tally.confirmed + tally.assumed == tally.total;
    assert_eq!(
        row.status == RunStatus::Confirmed,
        coming,
        "v5 status agrees with the tally: {tally}"
    );
    (
        tally,
        row.status == RunStatus::Confirmed && tally.assumed > 0,
    )
}

async fn react(f: &mut Fixture, run: usize, user: &str, emoji: &str, added: bool) -> RunStatus {
    let run = f.runs[run].clone();
    f.service
        .as_origin(member(user))
        .apply_reaction(&run, user, emoji, added)
        .await
        .unwrap()
        .new_status
}

async fn set_default(f: &mut Fixture, default: AttendanceDefault) {
    f.service
        .as_origin(admin())
        .set_attendance_default(&f.fixed, default)
        .await
        .unwrap();
}

async fn standing(f: &mut Fixture, user: &str, on: bool) {
    let fixed = f.fixed.clone();
    f.service
        .as_origin(member(user))
        .set_standing_answer(user, &fixed, on)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_timing_default_confirms_and_withdraws_in_its_own_commit() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let before = head(&f.service).await;
    set_default(&mut f, AttendanceDefault::AssumeComing).await;
    assert_eq!(head(&f.service).await, before + 1, "one commit");
    let run = f.run().to_owned();
    assert_eq!(status(&f.service, &run).await, RunStatus::Confirmed);
    assert_eq!(
        tally(&f, &run).await,
        (
            Tally {
                assumed: 2,
                total: 2,
                ..Tally::default()
            },
            true
        )
    );
    let state = snapshot(&f.service).await;
    let row = state.runs.iter().find(|row| row.id == run).unwrap();
    assert_eq!(
        snapshot_states(&state, row, f.attendance.mode),
        [
            (
                "1001".to_owned(),
                AnswerState::Assumed(AssumedSource::Default)
            ),
            (
                "1002".to_owned(),
                AnswerState::Assumed(AssumedSource::Default)
            ),
        ]
    );
    // Every live run of the timing followed, in that same commit.
    for run in &f.runs {
        assert_eq!(status(&f.service, run).await, RunStatus::Confirmed);
    }
    // The tick's recount finds nothing left to do.
    let recounted = f
        .service
        .as_origin(tick())
        .recount_attendance()
        .await
        .unwrap();
    assert!(recounted.is_empty());
    // An explicit reaction always wins over the default.
    assert_eq!(
        react(&mut f, 0, "1002", EMOJI_NO, true).await,
        RunStatus::AtRisk
    );
    assert_eq!(
        react(&mut f, 0, "1002", EMOJI_NO, false).await,
        RunStatus::Confirmed,
        "removing the ❌ reverts to assumed"
    );
    // Back to opt-in: no confirmation lingers ("Confirmed · 0/2" is gone).
    let before = head(&f.service).await;
    set_default(&mut f, AttendanceDefault::OptIn).await;
    assert_eq!(head(&f.service).await, before + 1, "one commit");
    for run in &f.runs {
        assert_eq!(status(&f.service, run).await, RunStatus::Planned);
    }
    assert_eq!(
        tally(&f, &run).await,
        (
            Tally {
                unknown: 2,
                total: 2,
                ..Tally::default()
            },
            false
        )
    );
}

#[tokio::test]
async fn standing_answers_confirm_and_withdraw_at_once() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let run = f.run().to_owned();
    assert_eq!(
        react(&mut f, 0, "1002", EMOJI_YES, true).await,
        RunStatus::Planned
    );
    // 1001's standing answer completes the party: confirmed immediately.
    let before = head(&f.service).await;
    standing(&mut f, "1001", true).await;
    assert_eq!(head(&f.service).await, before + 1, "one commit");
    assert_eq!(status(&f.service, &run).await, RunStatus::Confirmed);
    assert_eq!(
        tally(&f, &run).await,
        (
            Tally {
                confirmed: 1,
                assumed: 1,
                total: 2,
                ..Tally::default()
            },
            true
        )
    );
    // Clearing it before the window: planned at once.
    standing(&mut f, "1001", false).await;
    assert_eq!(status(&f.service, &run).await, RunStatus::Planned);
    tally(&f, &run).await;
    // Inside the window: set confirms, clear puts it at risk.
    f.clock.set(kl(8, 29, 8, 0));
    standing(&mut f, "1001", true).await;
    assert_eq!(status(&f.service, &run).await, RunStatus::Confirmed);
    standing(&mut f, "1001", false).await;
    assert_eq!(status(&f.service, &run).await, RunStatus::AtRisk);
    // Runs of later weeks are outside their windows: planned.
    assert_eq!(status(&f.service, &f.runs[1]).await, RunStatus::Planned);
}

#[tokio::test]
async fn a_standing_answer_and_one_reaction_confirm_a_run_in_v5_only() {
    for (attendance, expected) in [
        (AttendancePolicy::V5, RunStatus::Confirmed),
        (AttendancePolicy::V4_COMPAT, RunStatus::Planned),
    ] {
        let mut f = fixture(attendance).await;
        standing(&mut f, "1001", true).await;
        assert_eq!(
            react(&mut f, 0, "1002", EMOJI_YES, true).await,
            expected,
            "{attendance:?}"
        );
    }
}

#[tokio::test]
async fn unknown_answers_put_a_run_at_risk_once_its_window_opens() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let run = f.run().to_owned();
    assert_eq!(
        react(&mut f, 0, "1001", EMOJI_YES, true).await,
        RunStatus::Planned
    );
    let start = kl(8, 29, 20, 0);
    f.clock
        .set(start - TimeDelta::hours(12) - TimeDelta::minutes(1));
    let before = head(&f.service).await;
    let recount = f
        .service
        .as_origin(tick())
        .recount_attendance()
        .await
        .unwrap();
    assert!(recount.is_empty(), "not yet");
    assert_eq!(
        head(&f.service).await,
        before,
        "an idle recount records nothing"
    );
    f.clock.set(start - TimeDelta::hours(12));
    let recount = f
        .service
        .as_origin(tick())
        .recount_attendance()
        .await
        .unwrap();
    assert_eq!(recount, std::slice::from_ref(&run), "the window opened");
    assert_eq!(status(&f.service, &run).await, RunStatus::AtRisk);
    // A late ✅ from the unknown member confirms it again.
    assert_eq!(
        react(&mut f, 0, "1002", EMOJI_YES, true).await,
        RunStatus::Confirmed
    );
}

#[tokio::test]
async fn v4_compat_ignores_standing_answers_defaults_and_the_window() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    let run = f.run().to_owned();
    let before = head(&f.service).await;
    set_default(&mut f, AttendanceDefault::AssumeComing).await;
    standing(&mut f, "1001", true).await;
    assert_eq!(head(&f.service).await, before + 2);
    assert_eq!(
        status(&f.service, &run).await,
        RunStatus::Planned,
        "written, no effect"
    );
    f.clock.set(kl(8, 29, 19, 0));
    let before = head(&f.service).await;
    let recount = f
        .service
        .as_origin(tick())
        .recount_attendance()
        .await
        .unwrap();
    assert!(recount.is_empty());
    assert_eq!(head(&f.service).await, before);
    assert_eq!(
        react(&mut f, 0, "1002", EMOJI_YES, true).await,
        RunStatus::Planned
    );
    assert_eq!(
        react(&mut f, 0, "1001", EMOJI_YES, true).await,
        RunStatus::Confirmed
    );
    // v4's confirmation survives silence.
    assert_eq!(
        react(&mut f, 0, "1001", EMOJI_YES, false).await,
        RunStatus::Confirmed
    );
}

/// 1001 "always in", 1002 said ✅, 1003 has not answered on the first run.
async fn three_ways_fixture(attendance: AttendancePolicy) -> Fixture {
    let mut f = fixture_with(attendance, &["1001", "1002", "1003"]).await;
    standing(&mut f, "1001", true).await;
    react(&mut f, 0, "1002", EMOJI_YES, true).await;
    f
}

/// The same state reaches the same status by a reaction, a draft merge and
/// a cherry-picked answer: every path follows the scheduler's rules.
#[tokio::test]
async fn reactions_merges_and_picks_derive_status_from_one_source() {
    for (attendance, expected) in [
        (AttendancePolicy::V5, RunStatus::Confirmed),
        (AttendancePolicy::V4_COMPAT, RunStatus::Planned),
    ] {
        let schedule = policy(attendance);
        // A reaction: 1003 says yes.
        let mut f = three_ways_fixture(attendance).await;
        assert_eq!(react(&mut f, 0, "1003", EMOJI_YES, true).await, expected);
        // A draft merge: 1003 leaves the run.
        let mut f = three_ways_fixture(attendance).await;
        let run = f.run().to_owned();
        let draft = f.service.create_draft("root", "swap", None).await.unwrap();
        let staged = f
            .service
            .add_draft_op(
                "root",
                &draft.id,
                draft.version,
                DraftOp::SwapParticipants {
                    run: Target::Existing(run.clone()),
                    remove: vec!["1003".into()],
                    add: Vec::new(),
                    via_portal: true,
                },
                &schedule,
                &Guild,
            )
            .await
            .unwrap();
        f.service
            .merge_draft(
                "root",
                &draft.id,
                staged.draft.version,
                &schedule,
                &Guild,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            status(&f.service, &run).await,
            expected,
            "merge {attendance:?}"
        );
        // A cherry-pick of 1003's ✅ from the next week, recounted.
        let mut f = three_ways_fixture(attendance).await;
        let run = f.run().to_owned();
        react(&mut f, 1, "1003", EMOJI_YES, true).await;
        let picked = head(&f.service).await;
        let week = run_row(&f.service, &run).await.week_start;
        f.service
            .cherry_pick(
                "root",
                None,
                picked,
                week,
                PickMode::Strict,
                &schedule,
                &Guild,
            )
            .await
            .unwrap();
        assert_eq!(
            status(&f.service, &run).await,
            expected,
            "pick {attendance:?}"
        );
    }
}

#[tokio::test]
async fn a_policy_with_other_attendance_rules_is_refused() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let other = policy(AttendancePolicy::V4_COMPAT);
    let mismatch = ScheduleError::AttendanceMismatch {
        service: AttendancePolicy::V5,
        policy: AttendancePolicy::V4_COMPAT,
    };
    assert_eq!(
        f.service
            .as_origin(admin())
            .materialise_weeks(&other)
            .await
            .unwrap_err(),
        SchedulerError::Schedule(mismatch.clone())
    );
    let draft = f.service.create_draft("root", "x", None).await.unwrap();
    assert_eq!(
        f.service
            .merge_draft("root", &draft.id, draft.version, &other, &Guild, None)
            .await
            .unwrap_err(),
        DraftError::Schedule(mismatch.clone())
    );
    assert_eq!(
        f.service
            .cherry_pick(
                "root",
                None,
                1,
                run_row(&f.service, f.run()).await.week_start,
                PickMode::Strict,
                &other,
                &Guild
            )
            .await
            .unwrap_err(),
        PickError::Schedule(mismatch)
    );
}

#[tokio::test]
async fn tick_changes_cannot_be_cherry_picked() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let run = f.run().to_owned();
    react(&mut f, 1, "1001", EMOJI_YES, true).await;
    react(&mut f, 1, "1002", EMOJI_YES, true).await;
    // Next week's run goes at risk by the recount inside its window.
    react(&mut f, 1, "1002", EMOJI_YES, false).await;
    f.clock.set(kl(9, 5, 8, 0));
    let recounted = f
        .service
        .as_origin(tick())
        .recount_attendance()
        .await
        .unwrap();
    assert_eq!(recounted, std::slice::from_ref(&f.runs[1]));
    let seq = head(&f.service).await;
    f.clock.set(kl(8, 27, 2, 0));
    let week = run_row(&f.service, &run).await.week_start;
    assert!(matches!(
        f.service
            .cherry_pick(
                "root",
                None,
                seq,
                week,
                PickMode::Strict,
                &policy(AttendancePolicy::V5),
                &Guild
            )
            .await,
        Err(PickError::Refused(PickRefusal::UnsupportedPick { .. }))
    ));
}

async fn set_status(f: &mut Fixture, run: usize, status: RunStatus) {
    let run = f.runs[run].clone();
    let reminders = policy(f.attendance).reminders;
    f.service
        .as_origin(admin())
        .set_status(
            &run,
            StatusChange {
                status,
                announce: false,
                via_portal: true,
            },
            &reminders,
        )
        .await
        .unwrap();
}

async fn recount(f: &mut Fixture) -> Vec<String> {
    f.service
        .as_origin(tick())
        .recount_attendance()
        .await
        .unwrap()
}

async fn label(f: &Fixture, run: usize) -> Option<StatusLabel> {
    let state = snapshot(&f.service).await;
    let row = state.runs.iter().find(|row| row.id == f.runs[run]).unwrap();
    let states = snapshot_states(&state, row, f.attendance.mode);
    let tally = Tally::of(states.iter().map(|(_, state)| state));
    status_label(row.status, row.status_pin, &tally, f.attendance.mode)
}

#[tokio::test]
async fn a_hand_set_confirmed_holds_until_an_explicit_answer() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let run = f.run().to_owned();
    react(&mut f, 0, "1001", EMOJI_YES, true).await;
    set_status(&mut f, 0, RunStatus::Confirmed).await;
    let pinned = run_row(&f.service, &run).await.status_pin;
    assert_eq!(pinned.map(|pin| pin.status), Some(RunStatus::Confirmed));
    assert_eq!(
        label(&f, 0).await,
        Some(StatusLabel::SetByAdmin(RunStatus::Confirmed))
    );
    // 1002 is unknown: recounts before and inside the window keep it.
    assert!(recount(&mut f).await.is_empty());
    f.clock.set(kl(8, 29, 8, 0));
    assert!(
        recount(&mut f).await.is_empty(),
        "the window does not override"
    );
    assert_eq!(status(&f.service, &run).await, RunStatus::Confirmed);
    // An explicit ❌ ends the pin and re-derives in the same commit.
    let before = head(&f.service).await;
    assert_eq!(
        react(&mut f, 0, "1002", EMOJI_NO, true).await,
        RunStatus::AtRisk
    );
    assert_eq!(head(&f.service).await, before + 1);
    assert_eq!(run_row(&f.service, &run).await.status_pin, None);
    // Pinned again, then 1001 takes the ✅ back: unpinned and at risk.
    set_status(&mut f, 0, RunStatus::Confirmed).await;
    assert_eq!(status(&f.service, &run).await, RunStatus::Confirmed);
    let before = head(&f.service).await;
    assert_eq!(
        react(&mut f, 0, "1001", EMOJI_YES, false).await,
        RunStatus::AtRisk
    );
    assert_eq!(head(&f.service).await, before + 1);
    assert_eq!(run_row(&f.service, &run).await.status_pin, None);
}

#[tokio::test]
async fn a_hand_set_planned_on_an_assumed_run_survives_the_recount() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let run = f.run().to_owned();
    set_default(&mut f, AttendanceDefault::AssumeComing).await;
    assert_eq!(status(&f.service, &run).await, RunStatus::Confirmed);
    set_status(&mut f, 0, RunStatus::Planned).await;
    assert!(recount(&mut f).await.is_empty());
    assert_eq!(status(&f.service, &run).await, RunStatus::Planned);
    assert_eq!(
        label(&f, 0).await,
        Some(StatusLabel::SetByAdmin(RunStatus::Planned))
    );
    // A party change ends the pin and re-derives.
    f.service
        .as_origin(admin())
        .swap_participants(&run, &["1002".into()], &[], true, &Guild)
        .await
        .unwrap();
    let row = run_row(&f.service, &run).await;
    assert_eq!((row.status, row.status_pin), (RunStatus::Confirmed, None));
    // Leaving the live statuses ends it too.
    set_status(&mut f, 1, RunStatus::Confirmed).await;
    set_status(&mut f, 1, RunStatus::Cancelled).await;
    assert_eq!(run_row(&f.service, &f.runs[1]).await.status_pin, None);
}

#[tokio::test]
async fn a_cherry_picked_status_step_pins() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let run = f.run().to_owned();
    set_status(&mut f, 1, RunStatus::Confirmed).await;
    let picked = head(&f.service).await;
    let week = run_row(&f.service, &run).await.week_start;
    f.service
        .cherry_pick(
            "root",
            None,
            picked,
            week,
            PickMode::Strict,
            &policy(f.attendance),
            &Guild,
        )
        .await
        .unwrap();
    let row = run_row(&f.service, &run).await;
    assert_eq!(row.status, RunStatus::Confirmed);
    assert_eq!(
        row.status_pin.map(|pin| pin.status),
        Some(RunStatus::Confirmed)
    );
    assert!(recount(&mut f).await.is_empty(), "the pick holds");
}

#[tokio::test]
async fn a_started_run_is_frozen_in_v5_only() {
    for (attendance, after_swap) in [
        (AttendancePolicy::V5, RunStatus::Confirmed),
        (AttendancePolicy::V4_COMPAT, RunStatus::Planned),
    ] {
        let mut f = fixture(attendance).await;
        let run = f.run().to_owned();
        react(&mut f, 0, "1001", EMOJI_YES, true).await;
        react(&mut f, 0, "1002", EMOJI_YES, true).await;
        assert_eq!(status(&f.service, &run).await, RunStatus::Confirmed);
        f.clock.set(kl(8, 29, 20, 10));
        assert_eq!(
            react(&mut f, 0, "1001", EMOJI_YES, false).await,
            RunStatus::Confirmed,
            "{attendance:?}: taking a ✅ back after the start"
        );
        f.service
            .as_origin(admin())
            .swap_participants(&run, &[], &["1003".into()], true, &Guild)
            .await
            .unwrap();
        assert_eq!(
            status(&f.service, &run).await,
            after_swap,
            "{attendance:?}: swapping someone in after the start"
        );
        assert!(recount(&mut f).await.is_empty());
        set_status(&mut f, 0, RunStatus::Done).await;
        assert_eq!(status(&f.service, &run).await, RunStatus::Done, "manual");
    }
}

#[tokio::test]
async fn v4_compat_never_pins() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    set_status(&mut f, 0, RunStatus::Confirmed).await;
    let row = run_row(&f.service, f.run()).await;
    assert_eq!((row.status, row.status_pin), (RunStatus::Confirmed, None));
    assert_eq!(label(&f, 0).await, None);
}

/// A pinned confirmed run on incomplete answers (1002 unknown).
async fn pinned_on_incomplete_answers(f: &mut Fixture) -> String {
    let run = f.run().to_owned();
    react(f, 0, "1001", EMOJI_YES, true).await;
    set_status(f, 0, RunStatus::Confirmed).await;
    let row = run_row(&f.service, &run).await;
    assert_eq!(
        (row.status, row.status_pin.map(|pin| pin.status)),
        (RunStatus::Confirmed, Some(RunStatus::Confirmed))
    );
    run
}

async fn assert_unpinned_and_derived(f: &mut Fixture, run: &str) {
    let row = run_row(&f.service, run).await;
    assert_eq!((row.status, row.status_pin), (RunStatus::Planned, None));
    assert_eq!(label(f, 0).await, None);
    assert!(recount(f).await.is_empty(), "no pin brings confirmed back");
    assert_eq!(status(&f.service, run).await, RunStatus::Planned);
}

#[tokio::test]
async fn a_move_ends_the_pin_and_re_derives() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let run = pinned_on_incomplete_answers(&mut f).await;
    let before = head(&f.service).await;
    f.service
        .as_origin(admin())
        .amend_run(
            &run,
            kl(8, 29, 22, 0).with_timezone(&Utc),
            &policy(f.attendance),
        )
        .await
        .unwrap();
    assert_eq!(head(&f.service).await, before + 1, "one commit");
    assert_unpinned_and_derived(&mut f, &run).await;
}

#[tokio::test]
async fn a_reset_of_slot_only_ends_the_pin_and_re_derives() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let run = f.run().to_owned();
    f.service
        .as_origin(admin())
        .amend_run(
            &run,
            kl(8, 29, 22, 0).with_timezone(&Utc),
            &policy(f.attendance),
        )
        .await
        .unwrap();
    pinned_on_incomplete_answers(&mut f).await;
    let before = head(&f.service).await;
    let outcome = f
        .service
        .as_origin(admin())
        .reset_to_fixed(&run, &policy(f.attendance))
        .await
        .unwrap();
    assert_eq!(outcome.value.run.datetime, kl(8, 29, 20, 0));
    assert_eq!(head(&f.service).await, before + 1, "one commit");
    assert_unpinned_and_derived(&mut f, &run).await;
}

#[tokio::test]
async fn a_weekly_time_edit_moving_the_run_ends_the_pin() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let run = pinned_on_incomplete_answers(&mut f).await;
    let fixed = f.fixed.clone();
    let before = head(&f.service).await;
    f.service
        .as_origin(admin())
        .update_fixed(
            &fixed,
            FixedEdit {
                time: NaiveTime::from_hms_opt(21, 0, 0),
                ..FixedEdit::default()
            },
            &Guild,
            &policy(f.attendance),
        )
        .await
        .unwrap();
    assert_eq!(head(&f.service).await, before + 1, "one commit");
    assert_eq!(
        run_row(&f.service, &run).await.datetime,
        kl(8, 29, 21, 0).with_timezone(&Utc)
    );
    assert_unpinned_and_derived(&mut f, &run).await;
}

#[tokio::test]
async fn after_the_start_party_changes_keep_the_pin() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let run = pinned_on_incomplete_answers(&mut f).await;
    let pinned = run_row(&f.service, &run).await.status_pin;
    async fn pin_blame(service: &Service, run: &str) -> kanade::domain::history::Attribution {
        blame(service.store(), &BlameTarget::Run(run.to_owned()))
            .await
            .unwrap()
            .unwrap()
            .lines
            .into_iter()
            .find(|line| line.field == "status_pin")
            .and_then(|line| line.last)
            .unwrap()
    }
    let blamed = pin_blame(&f.service, &run).await;
    assert_eq!(blamed.actor, Actor::admin("root"));
    f.clock.set(kl(8, 29, 20, 10));
    f.service
        .as_origin(admin())
        .swap_participants(&run, &["1002".into()], &["1003".into()], true, &Guild)
        .await
        .unwrap();
    f.service
        .as_origin(admin())
        .change_fixed_party(
            &f.fixed,
            &[],
            &["1003".into()],
            &Guild,
            &policy(f.attendance),
        )
        .await
        .unwrap();
    let row = run_row(&f.service, &run).await;
    assert_eq!(row.participants, ["1001"]);
    assert_eq!((row.status, row.status_pin), (RunStatus::Confirmed, pinned));
    assert_eq!(
        label(&f, 0).await,
        Some(StatusLabel::SetByAdmin(RunStatus::Confirmed))
    );
    assert_eq!(pin_blame(&f.service, &run).await, blamed, "blame unchanged");
    assert!(recount(&mut f).await.is_empty());
    set_status(&mut f, 0, RunStatus::Done).await;
    let row = run_row(&f.service, &run).await;
    assert_eq!((row.status, row.status_pin), (RunStatus::Done, None));
}
