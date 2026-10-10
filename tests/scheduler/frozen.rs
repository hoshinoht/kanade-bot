//! v5 (user decision 2026-10-10, "Frozen, shown as ended"): a live run past
//! its end (start + run length; own-time at the boss reset) refuses moves,
//! swaps, amends and late answers inside the commit, weekly-timing edits and
//! retirement leave it alone, settling still works and the digest counts it
//! as cleared. Each case runs the same run just before and just after its end.

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::domain::completion::{RunEnds, RunEndsSource};
use kanade::domain::history::Origin;
use kanade::domain::members::Roster;
use kanade::domain::notify::digest_inclusion;
use kanade::domain::schedule::{
    FixedField, FixedRunPatch, MemberRunRefusal, NewFixedRun, NewRun, ReminderPolicy, RsvpSource,
    RsvpState, RunSource, RunStatus, ScheduleError, SchedulePolicy, SettleRun, StatusChange,
    utc_instant,
};
use kanade::domain::scheduler::{
    DeclineNoticeContext, ScheduleStore, SchedulerError, SchedulerService, Scope,
};
use kanade::domain::settings::RunLengths;
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::common::{SeqIds, TestClock};

type Service = SchedulerService<MemoryScheduleStore, SeqIds, TestClock>;

const ALICE: &str = "1001";
const BOB: &str = "1002";

fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0).unwrap()
}

/// Kuala Lumpur, boss reset Wednesday 00:00 local (Tue 16:00Z).
fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::Asia::Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60],
        },
        Weekday::Wed,
        NaiveTime::MIN,
    )
}

/// Thu 10 Sep 20:00 local, one boss: the default length ends it at 12:30Z.
const START: (u32, u32) = (10, 12);
fn start() -> DateTime<Utc> {
    utc(START.0, START.1, 0)
}
fn before_end() -> DateTime<Utc> {
    utc(10, 12, 29)
}
fn after_end() -> DateTime<Utc> {
    utc(10, 12, 30)
}

struct World {
    service: Service,
    clock: TestClock,
    run: String,
    fixed: String,
}

fn ids() -> SeqIds {
    let ids: Vec<String> = (1..=200).map(|n| format!("id-{n:03}")).collect();
    SeqIds::new(&serde_json::json!(ids))
}

/// A weekly timing (Thu 20:00) and its run this week, both on Alice and Bob,
/// with the clock at `now`.
async fn world(status: RunStatus, now: DateTime<Utc>) -> World {
    let clock = TestClock::new(utc(1, 0, 0).fixed_offset());
    let mut service = SchedulerService::new(MemoryScheduleStore::new(), ids(), clock.clone())
        .with_run_ends(RunEndsSource::fixed(RunLengths::default(), None, policy()));
    let fixed = service
        .as_origin(Origin::for_tests())
        .add_fixed_run(NewFixedRun {
            owner_id: ALICE.into(),
            channel_id: Some("900".into()),
            bosses: vec!["Kalos".into()],
            weekday: Weekday::Thu,
            time: NaiveTime::from_hms_opt(20, 0, 0).unwrap(),
            participants: vec![ALICE.into(), BOB.into()],
            note: None,
            owner_pinned: false,
        })
        .await
        .unwrap();
    let week = utc_instant(&policy().week_of(&start()).unwrap()).unwrap();
    let run = service
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: Some(fixed.clone()),
            channel_id: Some("900".into()),
            week_start: week,
            datetime: start(),
            bosses: vec!["Kalos".into()],
            participants: vec![ALICE.into(), BOB.into()],
            status,
            source: RunSource::Fixed,
        })
        .await
        .unwrap();
    clock.set(now.fixed_offset());
    World {
        service,
        clock,
        run,
        fixed,
    }
}

fn ended(result: Result<impl std::fmt::Debug, SchedulerError>) {
    match result {
        Err(SchedulerError::Schedule(ScheduleError::RunEnded { .. })) => {}
        other => panic!("expected run_ended, got {other:?}"),
    }
}

fn member_ended(result: Result<impl std::fmt::Debug, SchedulerError>) {
    assert!(
        matches!(
            result,
            Err(SchedulerError::Schedule(ScheduleError::MemberRun(
                MemberRunRefusal::Ended
            )))
        ),
        "expected the member refusal run_ended, got {result:?}"
    );
}

async fn status(world: &World) -> RunStatus {
    let snapshot = world.service.store().load(&Scope::All).await.unwrap();
    snapshot
        .runs
        .iter()
        .find(|run| run.id == world.run)
        .unwrap()
        .status
}

#[tokio::test]
async fn a_move_is_refused_once_the_run_has_ended() {
    let policy = policy();
    let to = utc(12, 12, 0);
    let mut open = world(RunStatus::Planned, before_end()).await;
    let run = open.run.clone();
    open.service
        .as_origin(Origin::for_tests())
        .amend_run(&run, to, &policy)
        .await
        .expect("before its end the run moves");
    let mut frozen = world(RunStatus::Planned, after_end()).await;
    let run = frozen.run.clone();
    ended(
        frozen
            .service
            .as_origin(Origin::for_tests())
            .amend_run(&run, to, &policy)
            .await,
    );
}

#[tokio::test]
async fn a_swap_is_refused_once_the_run_has_ended() {
    let roster = Roster::new();
    let remove = [BOB.to_owned()];
    let mut open = world(RunStatus::Planned, before_end()).await;
    let run = open.run.clone();
    open.service
        .as_origin(Origin::for_tests())
        .swap_participants(&run, &remove, &[], false, &roster)
        .await
        .expect("before its end the party changes");
    let mut frozen = world(RunStatus::Planned, after_end()).await;
    let run = frozen.run.clone();
    ended(
        frozen
            .service
            .as_origin(Origin::for_tests())
            .swap_participants(&run, &remove, &[], false, &roster)
            .await,
    );
}

#[tokio::test]
async fn a_late_answer_by_reaction_or_the_admin_api_is_refused() {
    let mut open = world(RunStatus::Planned, before_end()).await;
    let run = open.run.clone();
    let result = open
        .service
        .as_origin(Origin::for_tests())
        .apply_reaction(&run, ALICE, "\u{2705}", true)
        .await
        .expect("before its end a reaction counts");
    assert!(result.applied);
    open.service
        .as_origin(Origin::for_tests())
        .portal_answer(&run, BOB, Some(RsvpState::Yes))
        .await
        .expect("before its end an admin answer counts");

    let mut frozen = world(RunStatus::Planned, after_end()).await;
    let run = frozen.run.clone();
    ended(
        frozen
            .service
            .as_origin(Origin::for_tests())
            .apply_reaction(&run, ALICE, "\u{2705}", true)
            .await,
    );
    ended(
        frozen
            .service
            .as_origin(Origin::for_tests())
            .portal_answer(&run, BOB, Some(RsvpState::Yes))
            .await,
    );
    ended(
        frozen
            .service
            .as_origin(Origin::for_tests())
            .set_rsvp(&run, BOB, RsvpState::No, RsvpSource::Chat)
            .await,
    );
}

#[tokio::test]
async fn a_member_move_and_answer_are_refused_with_run_ended() {
    let policy = policy();
    let context = || DeclineNoticeContext {
        channel_id: None,
        reference_id: None,
        display_name: "Alice".into(),
    };
    // Before its start the member may still move it and answer.
    let mut open = world(RunStatus::Planned, utc(10, 11, 50)).await;
    let run = open.run.clone();
    open.service
        .as_origin(Origin::for_tests())
        .member_answer_with_decline(&run, ALICE, RsvpState::Yes, context(), &policy)
        .await
        .expect("an answer before the end");
    open.service
        .as_origin(Origin::for_tests())
        .member_amend_run(ALICE, &run, utc(12, 12, 0), &policy)
        .await
        .expect("a move before the start");

    let mut frozen = world(RunStatus::Planned, after_end()).await;
    let run = frozen.run.clone();
    member_ended(
        frozen
            .service
            .as_origin(Origin::for_tests())
            .member_answer_with_decline(&run, ALICE, RsvpState::Yes, context(), &policy)
            .await,
    );
    member_ended(
        frozen
            .service
            .as_origin(Origin::for_tests())
            .member_amend_run(ALICE, &run, utc(12, 12, 0), &policy)
            .await,
    );
}

#[tokio::test]
async fn weekly_edits_and_retirement_skip_an_ended_run() {
    let policy = policy();
    let weeks = [policy.week_of(&start()).unwrap()];
    let patch = || FixedRunPatch {
        bosses: Some(vec!["Kaling".into()]),
        ..FixedRunPatch::default()
    };
    for (now, touched) in [(before_end(), 1), (after_end(), 0)] {
        let mut world = world(RunStatus::Planned, now).await;
        let fixed = world.fixed.clone();
        let count = world
            .service
            .as_origin(Origin::for_tests())
            .edit_fixed_run(
                &fixed,
                patch(),
                &[FixedField::Bosses],
                &weeks,
                &policy.reminders,
            )
            .await
            .unwrap();
        assert_eq!(count, touched, "edit at {now}");
        let retired = world
            .service
            .as_origin(Origin::for_tests())
            .retire_fixed_run(&fixed, &weeks, &policy.reminders)
            .await
            .unwrap();
        assert_eq!(retired, touched, "retire at {now}");
        let expected = if touched == 1 {
            RunStatus::Cancelled
        } else {
            RunStatus::Planned
        };
        assert_eq!(status(&world).await, expected, "at {now}");
    }
}

#[tokio::test]
async fn staff_status_and_the_prompt_still_settle_an_ended_run() {
    let policy = policy();
    let mut staff = world(RunStatus::Planned, after_end()).await;
    let run = staff.run.clone();
    staff
        .service
        .as_origin(Origin::for_tests())
        .set_status(
            &run,
            StatusChange {
                status: RunStatus::Done,
                announce: false,
                via_portal: false,
            },
            &policy.reminders,
        )
        .await
        .expect("staff settle a frozen run");
    assert_eq!(status(&staff).await, RunStatus::Done);

    let mut prompt = world(RunStatus::Planned, after_end()).await;
    let run = prompt.run.clone();
    let settled = prompt
        .service
        .as_origin(Origin::for_tests())
        .settle_run(
            SettleRun {
                run_id: run,
                status: RunStatus::Cancelled,
                datetime: start(),
                user: ALICE.into(),
                staff: false,
            },
            &policy.reminders,
        )
        .await
        .expect("the prompt settles a frozen run");
    assert!(settled);
    assert_eq!(status(&prompt).await, RunStatus::Cancelled);
}

#[tokio::test]
async fn an_own_time_run_stays_editable_until_the_reset() {
    let policy = policy();
    let remove = [BOB.to_owned()];
    // Tue 15 Sep 23:59 local: long after its slot, still before the reset.
    let mut open = world(RunStatus::Otot, utc(15, 15, 59)).await;
    let run = open.run.clone();
    open.service
        .as_origin(Origin::for_tests())
        .swap_participants(&run, &remove, &[], false, &Roster::new())
        .await
        .expect("own time is open all week");
    let mut frozen = world(RunStatus::Otot, utc(15, 16, 0)).await;
    let run = frozen.run.clone();
    ended(
        frozen
            .service
            .as_origin(Origin::for_tests())
            .amend_run(&run, utc(16, 12, 0), &policy)
            .await,
    );
}

#[tokio::test]
async fn the_digest_counts_an_ended_run_as_cleared() {
    let world = world(RunStatus::AtRisk, start()).await;
    let snapshot = world.service.store().load(&Scope::All).await.unwrap();
    let week = snapshot.runs[0].week_start;
    let ends = RunEnds::new(RunLengths::default(), None, policy());
    let zone = policy().zone();
    let open = digest_inclusion(&snapshot.runs, week, zone, Some((&ends, before_end()))).unwrap();
    assert_eq!((open.cleared, open.unsettled, open.at_risk), (0, 1, 1));
    let over = digest_inclusion(&snapshot.runs, week, zone, Some((&ends, after_end()))).unwrap();
    assert_eq!((over.cleared, over.unsettled, over.at_risk), (1, 0, 0));
    // The v4 counts (no run ends) are unchanged.
    let v4 = digest_inclusion(&snapshot.runs, week, zone, None).unwrap();
    assert_eq!((v4.cleared, v4.unsettled, v4.at_risk), (0, 1, 1));
}

fn at(now: DateTime<Utc>) -> chrono::DateTime<chrono::FixedOffset> {
    now.fixed_offset()
}

/// A party change recorded on next week's run, picked onto this week's.
#[tokio::test]
async fn a_cherry_pick_onto_an_ended_run_is_refused() {
    use kanade::domain::history::{ChangeHistory, PickMode};
    let policy = policy();
    let this_week = utc_instant(&policy.week_of(&start()).unwrap()).unwrap();
    for (now, refused) in [(before_end(), false), (after_end(), true)] {
        let mut world = world(RunStatus::Planned, utc(1, 0, 0)).await;
        let next = world
            .service
            .as_origin(Origin::for_tests())
            .create_run(NewRun {
                fixed_run_id: Some(world.fixed.clone()),
                channel_id: Some("900".into()),
                week_start: this_week + chrono::TimeDelta::days(7),
                datetime: start() + chrono::TimeDelta::days(7),
                bosses: vec!["Kalos".into()],
                participants: vec![ALICE.into(), BOB.into()],
                status: RunStatus::Planned,
                source: RunSource::Fixed,
            })
            .await
            .unwrap();
        world
            .service
            .as_origin(Origin::for_tests())
            .swap_participants(&next, &[BOB.to_owned()], &[], false, &Roster::new())
            .await
            .unwrap();
        let picked = world.service.store().history_head().await.unwrap();
        world.clock.set(at(now));
        let result = world
            .service
            .cherry_pick(
                "root",
                None,
                picked.seq,
                this_week,
                PickMode::Strict,
                &policy,
                &Roster::new(),
            )
            .await;
        if refused {
            assert!(
                matches!(
                    result,
                    Err(kanade::domain::scheduler::PickError::Schedule(
                        ScheduleError::RunEnded { .. }
                    ))
                ),
                "{result:?}"
            );
        } else {
            result.expect("before its end the pick applies");
        }
    }
}

/// The inbox lists the amended runs a weekly-timing change asks about from
/// this preview: an ended amended run is not one, and the change approves
/// without a choice for it.
#[tokio::test]
async fn a_weekly_change_asks_no_choice_for_an_ended_amended_run() {
    use kanade::domain::schedule::{FixedEdit, FixedEditChoices, FixedEditRequest};
    let policy = policy();
    let mut world = world(RunStatus::Planned, utc(10, 11, 0)).await;
    let run = world.run.clone();
    // Amended to 19:30 local: it ends at 12:00Z.
    world
        .service
        .as_origin(Origin::for_tests())
        .amend_run(&run, utc(10, 11, 30), &policy)
        .await
        .unwrap();
    let edit = FixedEdit {
        time: NaiveTime::from_hms_opt(21, 0, 0),
        ..FixedEdit::default()
    };
    world.clock.set(at(utc(10, 11, 59)));
    let open = world
        .service
        .preview_fixed_edit(&world.fixed, &edit, &policy)
        .await
        .unwrap();
    assert_eq!(
        open.len(),
        1,
        "before its end the amended run is asked about"
    );
    world.clock.set(at(utc(10, 12, 0)));
    let over = world
        .service
        .preview_fixed_edit(&world.fixed, &edit, &policy)
        .await
        .unwrap();
    assert!(over.is_empty(), "{over:?}");
    let request = FixedEditRequest {
        fixed_id: world.fixed.clone(),
        edit,
        choices: FixedEditChoices::PerRun(Default::default()),
    };
    world
        .service
        .as_origin(Origin::for_tests())
        .apply_fixed_edit(&request, &Roster::new(), &policy)
        .await
        .expect("approves without a choice for the ended run");
    let snapshot = world.service.store().load(&Scope::All).await.unwrap();
    let kept = snapshot.runs.iter().find(|row| row.id == run).unwrap();
    assert_eq!(
        kept.datetime,
        utc(10, 11, 30),
        "the ended run is left alone"
    );
}

/// What `propose` runs for a chat proposal: translate, then replay under the
/// scheduler's run ends.
async fn propose(
    world: &World,
    kind: kanade::domain::proposals::ChangeKind,
    to: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<(), String> {
    use kanade::domain::proposals::{ProposedChange, translate};
    let policy = policy();
    let ends = std::sync::Arc::new(RunEnds::new(RunLengths::default(), None, policy.clone()));
    let snapshot = world.service.store().load(&Scope::All).await.unwrap();
    let mut change = ProposedChange::new(kind);
    change.run_id = Some(world.run.clone());
    change.new_datetime = to;
    let translation = translate(&change, &snapshot, &policy, Some(&ends), now)
        .map_err(|refusal| refusal.to_string())?;
    kanade::domain::drafts::replay(
        &snapshot,
        &translation.ops,
        &policy,
        Some(&ends),
        &Roster::new(),
        now,
    )
    .map(|_| ())
    .map_err(|rejected| rejected.error.to_string())
}

#[tokio::test]
async fn moving_a_cancelled_or_own_time_run_after_its_old_slot_works() {
    use kanade::domain::proposals::ChangeKind;
    let to = Some(utc(12, 12, 0));
    let cancelled = world(RunStatus::Cancelled, after_end()).await;
    propose(&cancelled, ChangeKind::Move, to, after_end())
        .await
        .expect("a cancelled run is revived and moved");
    // Own time, after its slot and before the reset.
    let own = world(RunStatus::Otot, utc(11, 0, 0)).await;
    propose(&own, ChangeKind::Move, to, utc(11, 0, 0))
        .await
        .expect("an own-time run is revived and moved");
}

#[tokio::test]
async fn cancel_and_own_time_proposals_are_refused_on_an_ended_run() {
    use kanade::domain::proposals::ChangeKind;
    for kind in [ChangeKind::Cancel, ChangeKind::Otot] {
        let open = world(RunStatus::Planned, before_end()).await;
        propose(&open, kind, None, before_end())
            .await
            .expect("before its end");
        let frozen = world(RunStatus::Planned, after_end()).await;
        assert_eq!(
            propose(&frozen, kind, None, after_end()).await,
            Err(kanade::domain::schedule::RUN_ENDED.to_owned()),
            "{kind:?}"
        );
    }
}

struct Party;

impl kanade::domain::members::Directory for Party {
    fn member(&self, user_id: &str) -> Option<kanade::domain::members::Member> {
        [ALICE, BOB]
            .contains(&user_id)
            .then(|| kanade::domain::members::Member {
                user_id: user_id.to_owned(),
                has_role: true,
                ..Default::default()
            })
    }

    fn is_watched(&self, _channel_id: &str) -> bool {
        true
    }
}

#[tokio::test]
async fn a_cancel_proposed_before_the_end_is_refused_when_approved_after_it() {
    use kanade::domain::drafts::ProposalSource;
    use kanade::domain::proposals::{Approver, ChangeKind, ProposedChange, Refusal};
    use kanade::domain::scheduler::{ProposalError, ProposalRequest, Supersede};
    for kind in [ChangeKind::Cancel, ChangeKind::Otot] {
        let mut world = world(RunStatus::Planned, before_end()).await;
        let mut change = ProposedChange::new(kind);
        change.run_id = Some(world.run.clone());
        change.channel_id = Some("900".into());
        let id = world
            .service
            .propose(
                ProposalRequest {
                    change,
                    source: ProposalSource::Extraction,
                    source_id: "log-1".into(),
                    supersede: Supersede::Older,
                },
                &policy(),
                &Party,
            )
            .await
            .expect("proposed before the end")
            .proposal
            .id;
        world.clock.set(after_end().fixed_offset());
        let approver = Approver {
            user_id: BOB.into(),
            has_role: true,
            is_admin: false,
            via_portal: false,
        };
        let refused = world
            .service
            .approve_proposal(&id, &approver, &policy(), &Party)
            .await;
        assert_eq!(
            refused.map(|_| ()),
            Err(ProposalError::Refused(Refusal::Rule(
                kanade::domain::schedule::RUN_ENDED.to_owned()
            ))),
            "{kind:?}"
        );
        assert_eq!(status(&world).await, RunStatus::Planned, "{kind:?}");
    }
}
