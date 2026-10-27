//! v5 attendance on the write path: status fully derived from standing
//! answers, the timing default and the unknown window, under the service's
//! one attendance policy.

use chrono::{DateTime, FixedOffset, NaiveTime, TimeDelta, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade_domain::attendance::{
    AttendanceDefault, AttendancePolicy, StatusLabel, Tally, snapshot_states, status_label,
};
use kanade_domain::drafts::{ProposalSource, StoredDraft};
use kanade_domain::history::{
    Actor, Attribution, BlameTarget, ChangeHistory, Origin, Surface, blame,
};
use kanade_domain::members::{Directory, Member};
use kanade_domain::proposals::{Approver, ChangeKind, ProposedChange};
use kanade_domain::requests::{NoFreezes, RequestSpec, Subject};
use kanade_domain::schedule::{
    EMOJI_NO, EMOJI_YES, FixedEdit, ReminderPolicy, RsvpSource, RsvpState, Run, RunStatus,
    ScheduleError, SchedulePolicy, StatusChange,
};
use kanade_domain::scheduler::{
    Approved, DraftError, ProposalError, ProposalRequest, RequestError, SchedulerError,
    SchedulerService, Supersede,
};
use kanade_store::SqliteStore;

use crate::common::TestClock;
use crate::support::{CountingIds, hm, snapshot, weekly};

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

type Service = SchedulerService<SqliteStore, CountingIds, TestClock>;

fn kl(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
}

fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: Kuala_Lumpur,
            ping_time: hm(9, 0),
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
    Origin::new(Actor::member(id), Surface::Discord)
}

fn tick() -> Origin {
    Origin::new(Actor::system("delivery"), Surface::DeliveryTick)
}

struct Fixture {
    service: Service,
    clock: TestClock,
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
async fn fixture_with(party: &[&str]) -> Fixture {
    let clock = TestClock::new(kl(8, 27, 1, 0));
    let mut service = SchedulerService::new(
        SqliteStore::open_temp().await,
        CountingIds::default(),
        clock.clone(),
    );
    let fixed = service
        .as_origin(admin())
        .add_fixed_run(weekly("222", Weekday::Sat, hm(20, 0), party))
        .await
        .unwrap();
    service
        .as_origin(admin())
        .materialise_weeks(&policy())
        .await
        .unwrap();
    let state = snapshot(&service).await;
    let run_at = |day: DateTime<FixedOffset>| {
        let run = state.runs.iter().find(|run| run.datetime == day);
        run.unwrap().id.clone()
    };
    let runs = [
        run_at(kl(8, 29, 20, 0)),
        run_at(kl(9, 5, 20, 0)),
        run_at(kl(9, 12, 20, 0)),
    ];
    Fixture {
        service,
        clock,
        fixed,
        runs,
    }
}

async fn fixture() -> Fixture {
    fixture_with(&["1001", "1002"]).await
}

async fn run_row(service: &Service, run: &str) -> Run {
    let state = snapshot(service).await;
    state.runs.into_iter().find(|row| row.id == run).unwrap()
}

async fn status(service: &Service, run: &str) -> RunStatus {
    run_row(service, run).await.status
}

async fn head(service: &Service) -> u64 {
    service.store().history_head().await.unwrap().seq
}

async fn recount(f: &mut Fixture) -> Vec<String> {
    f.service
        .as_origin(tick())
        .recount_attendance()
        .await
        .unwrap()
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

/// Stores the timing's standing answers and default (no write path sets
/// them any more), then recounts as the tick does.
async fn seed(f: &mut Fixture, standing: &[&str], default: AttendanceDefault) {
    kanade_store::conformance::seed_attendance(f.service.store(), &f.fixed, standing, default)
        .await;
    recount(f).await;
}

#[tokio::test]
async fn a_standing_answer_and_one_reaction_confirm_a_run() {
    let mut f = fixture().await;
    seed(&mut f, &["1001"], AttendanceDefault::OptIn).await;
    assert_eq!(
        react(&mut f, 0, "1002", EMOJI_YES, true).await,
        RunStatus::Confirmed
    );
}

#[tokio::test]
async fn unknown_answers_put_a_run_at_risk_once_its_window_opens() {
    let mut f = fixture().await;
    let run = f.run().to_owned();
    assert_eq!(
        react(&mut f, 0, "1001", EMOJI_YES, true).await,
        RunStatus::Planned
    );
    let start = kl(8, 29, 20, 0);
    f.clock
        .set(start - TimeDelta::hours(12) - TimeDelta::minutes(1));
    let before = head(&f.service).await;
    assert!(recount(&mut f).await.is_empty(), "not yet");
    assert_eq!(
        head(&f.service).await,
        before,
        "an idle recount records nothing"
    );
    f.clock.set(start - TimeDelta::hours(12));
    assert_eq!(
        recount(&mut f).await,
        std::slice::from_ref(&run),
        "the window opened"
    );
    assert_eq!(status(&f.service, &run).await, RunStatus::AtRisk);
    // A late ✅ from the unknown member confirms it again.
    assert_eq!(
        react(&mut f, 0, "1002", EMOJI_YES, true).await,
        RunStatus::Confirmed
    );
}

/// 1001 "always in", 1002 said ✅, 1003 has not answered on the first run.
async fn three_ways_fixture() -> Fixture {
    let mut f = fixture_with(&["1001", "1002", "1003"]).await;
    seed(&mut f, &["1001"], AttendanceDefault::OptIn).await;
    react(&mut f, 0, "1002", EMOJI_YES, true).await;
    f
}

/// `member` asks to leave `run`.
async fn leave_request(
    f: &mut Fixture,
    member: &str,
    run: &str,
    schedule: &SchedulePolicy,
) -> StoredDraft {
    let spec = RequestSpec::Leave(Subject::Run(run.to_owned()));
    let submitted = f.service.submit_request(
        member,
        "can't make it",
        spec,
        None,
        schedule,
        &Guild,
        &NoFreezes,
    );
    submitted.await.unwrap()
}

/// An administrator approves request `id` as reviewed at version 1.
async fn approve(
    f: &mut Fixture,
    id: &str,
    schedule: &SchedulePolicy,
) -> Result<Approved, RequestError> {
    let admin = Actor::admin("root");
    f.service
        .approve_request(&admin, id, 1, None, schedule, &Guild, &NoFreezes)
        .await
}

/// The same state reaches the same status by a reaction and by an approved
/// member request's merge: every path follows the scheduler's rules.
#[tokio::test]
async fn reactions_and_request_merges_derive_status_from_one_source() {
    let schedule = policy();
    // A reaction: 1003 says yes.
    let mut f = three_ways_fixture().await;
    assert_eq!(
        react(&mut f, 0, "1003", EMOJI_YES, true).await,
        RunStatus::Confirmed
    );
    // A request merge: 1003 leaves the run.
    let mut f = three_ways_fixture().await;
    let run = f.run().to_owned();
    let request = leave_request(&mut f, "1003", &run, &schedule).await;
    approve(&mut f, &request.id, &schedule).await.unwrap();
    assert_eq!(
        status(&f.service, &run).await,
        RunStatus::Confirmed,
        "merge"
    );
}

#[tokio::test]
async fn a_policy_with_other_attendance_rules_is_refused() {
    let mut f = fixture().await;
    let narrower = AttendancePolicy {
        unknown_window: TimeDelta::hours(6),
    };
    let other = policy().with_attendance(narrower);
    let mismatch = ScheduleError::AttendanceMismatch {
        service: AttendancePolicy::V5,
        policy: narrower,
    };
    assert_eq!(
        f.service
            .as_origin(admin())
            .materialise_weeks(&other)
            .await
            .unwrap_err(),
        SchedulerError::Schedule(mismatch.clone())
    );
    // A member request's merge checks it too.
    let run = f.run().to_owned();
    let request = leave_request(&mut f, "1002", &run, &policy()).await;
    let refused = approve(&mut f, &request.id, &other).await;
    assert!(
        matches!(&refused, Err(RequestError::Draft(DraftError::Schedule(error))) if *error == mismatch),
        "{refused:?}"
    );
    // And a proposal's.
    let proposed = f
        .service
        .propose(
            ProposalRequest {
                change: ProposedChange {
                    run_id: Some(run.clone()),
                    channel_id: Some("222".into()),
                    ..ProposedChange::new(ChangeKind::Cancel)
                },
                source: ProposalSource::Extraction,
                source_id: "log-1".into(),
                supersede: Supersede::Older,
            },
            &policy(),
            &Guild,
        )
        .await
        .unwrap();
    let approver = Approver {
        user_id: "1001".into(),
        has_role: true,
        is_admin: true,
        via_portal: true,
    };
    let refused = f
        .service
        .approve_proposal(&proposed.proposal.id, &approver, &other, &Guild)
        .await;
    assert!(
        matches!(&refused, Err(ProposalError::Draft(DraftError::Schedule(error))) if *error == mismatch),
        "{refused:?}"
    );
}

async fn set_status(f: &mut Fixture, run: usize, status: RunStatus) {
    let run = f.runs[run].clone();
    let reminders = policy().reminders;
    let change = StatusChange {
        status,
        announce: false,
        via_portal: true,
    };
    f.service
        .as_origin(admin())
        .set_status(&run, change, &reminders)
        .await
        .unwrap();
}

async fn label(f: &Fixture, run: usize) -> Option<StatusLabel> {
    let state = snapshot(&f.service).await;
    let row = state.runs.iter().find(|row| row.id == f.runs[run]).unwrap();
    let states = snapshot_states(&state, row);
    let tally = Tally::of(states.iter().map(|(_, state)| state));
    status_label(row.status, row.status_pin, &tally)
}

#[tokio::test]
async fn a_hand_set_confirmed_holds_until_an_explicit_answer() {
    let mut f = fixture().await;
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

async fn portal_answer(f: &mut Fixture, run: usize, user: &str, answer: Option<RsvpState>) {
    let run = f.runs[run].clone();
    f.service
        .as_origin(admin())
        .portal_answer(&run, user, answer)
        .await
        .unwrap();
}

#[tokio::test]
async fn portal_answers_are_chat_recount_and_keep_a_hand_set_status() {
    let mut f = fixture().await;
    let run = f.run().to_owned();
    set_status(&mut f, 0, RunStatus::Confirmed).await;
    portal_answer(&mut f, 0, "1002", Some(RsvpState::No)).await;
    let row = run_row(&f.service, &run).await;
    assert_eq!(row.status, RunStatus::Confirmed, "the pin holds");
    assert!(
        row.status_pin.is_some(),
        "a portal answer never ends the pin"
    );
    let state = snapshot(&f.service).await;
    let rsvp = state
        .rsvps
        .iter()
        .find(|rsvp| rsvp.run_id == run && rsvp.user_id == "1002")
        .unwrap();
    assert_eq!((rsvp.state, rsvp.source), (RsvpState::No, RsvpSource::Chat));

    // A maybe can be set and cleared like any answer.
    portal_answer(&mut f, 0, "1001", Some(RsvpState::Maybe)).await;
    portal_answer(&mut f, 0, "1001", None).await;
    let state = snapshot(&f.service).await;
    assert!(
        !state
            .rsvps
            .iter()
            .any(|rsvp| rsvp.run_id == run && rsvp.user_id == "1001")
    );

    let refused = f
        .service
        .as_origin(admin())
        .portal_answer(&run, "9999", Some(RsvpState::Yes))
        .await;
    assert!(matches!(
        refused,
        Err(SchedulerError::Schedule(ScheduleError::NotOnRun(_)))
    ));
}

#[tokio::test]
async fn portal_answers_recount_an_unpinned_run() {
    let mut f = fixture().await;
    let run = f.run().to_owned();
    portal_answer(&mut f, 0, "1002", Some(RsvpState::No)).await;
    assert_eq!(status(&f.service, &run).await, RunStatus::AtRisk);
    portal_answer(&mut f, 0, "1002", None).await;
    assert_ne!(
        status(&f.service, &run).await,
        RunStatus::AtRisk,
        "taken back"
    );
}

#[tokio::test]
async fn a_hand_set_planned_on_an_assumed_run_survives_the_recount() {
    let mut f = fixture().await;
    let run = f.run().to_owned();
    seed(&mut f, &[], AttendanceDefault::AssumeComing).await;
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
async fn a_started_run_is_frozen() {
    let mut f = fixture().await;
    let run = f.run().to_owned();
    react(&mut f, 0, "1001", EMOJI_YES, true).await;
    react(&mut f, 0, "1002", EMOJI_YES, true).await;
    assert_eq!(status(&f.service, &run).await, RunStatus::Confirmed);
    f.clock.set(kl(8, 29, 20, 10));
    assert_eq!(
        react(&mut f, 0, "1001", EMOJI_YES, false).await,
        RunStatus::Confirmed,
        "taking a ✅ back after the start"
    );
    f.service
        .as_origin(admin())
        .swap_participants(&run, &[], &["1003".into()], true, &Guild)
        .await
        .unwrap();
    assert_eq!(
        status(&f.service, &run).await,
        RunStatus::Confirmed,
        "swapping someone in after the start"
    );
    assert!(recount(&mut f).await.is_empty());
    set_status(&mut f, 0, RunStatus::Done).await;
    assert_eq!(status(&f.service, &run).await, RunStatus::Done, "manual");
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

/// The admin moves `run` two hours later, to Sat 29 Aug 22:00.
async fn move_run(f: &mut Fixture, run: &str) {
    let to = kl(8, 29, 22, 0).with_timezone(&Utc);
    f.service
        .as_origin(admin())
        .amend_run(run, to, &policy())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_move_ends_the_pin_and_re_derives() {
    let mut f = fixture().await;
    let run = pinned_on_incomplete_answers(&mut f).await;
    let before = head(&f.service).await;
    move_run(&mut f, &run).await;
    assert_eq!(head(&f.service).await, before + 1, "one commit");
    assert_unpinned_and_derived(&mut f, &run).await;
}

#[tokio::test]
async fn a_reset_of_slot_only_ends_the_pin_and_re_derives() {
    let mut f = fixture().await;
    let run = f.run().to_owned();
    move_run(&mut f, &run).await;
    pinned_on_incomplete_answers(&mut f).await;
    let before = head(&f.service).await;
    let outcome = f
        .service
        .as_origin(admin())
        .reset_to_fixed(&run, &policy())
        .await
        .unwrap();
    assert_eq!(outcome.value.run.datetime, kl(8, 29, 20, 0));
    assert_eq!(head(&f.service).await, before + 1, "one commit");
    assert_unpinned_and_derived(&mut f, &run).await;
}

#[tokio::test]
async fn a_weekly_time_edit_moving_the_run_ends_the_pin() {
    let mut f = fixture().await;
    let run = pinned_on_incomplete_answers(&mut f).await;
    let fixed = f.fixed.clone();
    let before = head(&f.service).await;
    let edit = FixedEdit {
        time: Some(hm(21, 0)),
        ..FixedEdit::default()
    };
    f.service
        .as_origin(admin())
        .update_fixed(&fixed, edit, &Guild, &policy())
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
    let mut f = fixture().await;
    let run = pinned_on_incomplete_answers(&mut f).await;
    let pinned = run_row(&f.service, &run).await.status_pin;
    async fn pin_blame(service: &Service, run: &str) -> Attribution {
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
        .change_fixed_party(&f.fixed, &[], &["1003".into()], &Guild, &policy())
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
