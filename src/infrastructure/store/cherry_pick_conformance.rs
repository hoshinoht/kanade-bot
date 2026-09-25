//! Cherry-pick every store must support, driven through
//! [`SchedulerService`] against a fresh store per check. Boss weeks reset
//! Thursday 00:00 in London; British Summer Time starts on Sunday
//! 29 March 2026. Failures panic with the check name.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};

use crate::domain::history::{
    Actor, BlameIndex, BlameTarget, ChangeHistory, ChangeRef, EDIT_OVERRIDE, Expect,
    HistoryRefusal, Origin, PickMode, PickRefusal, PickStep, RevertMode, Surface, Via, blame,
};
use crate::domain::ids::IdGenerator;
use crate::domain::members::{Member, Roster};
use crate::domain::notify::DeliveryJournal;
use crate::domain::schedule::{
    EMOJI_NO, EMOJI_YES, NewFixedRun, NewRun, ReminderPolicy, RsvpSource, RsvpState, RunSource,
    RunStatus, SchedulePolicy, ScheduleSnapshot, StatusChange,
};
use crate::domain::scheduler::{Clock, PickError, ScheduleStore, SchedulerService, Scope};

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal + Sync>(
    make: impl AsyncFn() -> S,
) {
    a_slot_maps_to_the_same_local_time_across_spring_forward(make().await).await;
    strict_refuses_conflicts_and_force_is_bound_to_the_review(make().await).await;
    timings_and_one_off_runs_are_unsupported(make().await).await;
    answers_map_to_members_on_the_target_run(make().await).await;
    picks_are_idempotent_per_request_id(make().await).await;
    weeks_without_a_target_run_and_bad_weeks_are_refused(make().await).await;
    previews_write_nothing(make().await).await;
    picked_answers_recount_the_target_status(make().await).await;
    finished_and_otot_targets(make().await).await;
    a_forced_field_without_history_is_still_an_override(make().await).await;
    swaps_and_sole_statuses_pick(make().await).await;
    a_forced_swap_drops_members_already_gone(make().await).await;
    a_shifted_source_slot_is_not_a_conflict(make().await).await;
    a_preview_reads_values_and_versions_consistently(make().await).await;
    a_change_inside_the_forced_commit_is_stale(make().await).await;
    an_emptied_trimmed_delta_is_no_effect_in_both_modes(make().await).await;
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
        utc(3, 19, 1, 0)
    }
}

type Service<S> = SchedulerService<S, Ids, Now>;

fn utc(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .single()
        .expect("valid instant")
}

fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::Europe::London,
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

fn roster() -> Roster {
    let mut roster = Roster::new();
    for id in ["1", "2", "3", "4"] {
        roster.upsert(Member {
            user_id: id.into(),
            display_name: Some(format!("m{id}")),
            has_role: true,
            ..Member::default()
        });
    }
    roster
}

/// Boss weeks: 0 from 19 Mar (GMT), 1 from 26 Mar (GMT, BST starts inside),
/// 2 from 2 Apr (BST).
fn week(index: u32) -> DateTime<Utc> {
    match index {
        0 => utc(3, 19, 0, 0),
        1 => utc(3, 26, 0, 0),
        _ => utc(4, 1, 23, 0),
    }
}

/// A Saturday 20:00 London timing (party 1 and 2) materialised for weeks
/// 0 to 2; returns the service, the timing and its runs by week.
async fn fixture<S: ScheduleStore>(store: S) -> (Service<S>, String, [String; 3]) {
    let mut service = SchedulerService::new(store, Ids::default(), Now);
    let fixed = service
        .as_origin(admin())
        .add_fixed_run(NewFixedRun {
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
        .as_origin(Origin::new(
            Actor::system("delivery"),
            Surface::DeliveryTick,
        ))
        .materialise_weeks(&policy())
        .await
        .expect("materialise");
    let state = snapshot(&service).await;
    let run = |index: u32| {
        state
            .runs
            .iter()
            .find(|run| run.week_start == week(index))
            .expect("materialised run")
            .id
            .clone()
    };
    let runs = [run(0), run(1), run(2)];
    (service, fixed, runs)
}

async fn snapshot<S: ScheduleStore>(service: &Service<S>) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.expect("load")
}

async fn head<S: ScheduleStore + ChangeHistory>(service: &Service<S>) -> ChangeRef {
    service.store().history_head().await.expect("head")
}

async fn datetime<S: ScheduleStore>(service: &Service<S>, run: &str) -> DateTime<Utc> {
    snapshot(service)
        .await
        .runs
        .iter()
        .find(|row| row.id == run)
        .expect("run")
        .datetime
}

async fn journal_is_empty<S: ScheduleStore + DeliveryJournal>(service: &Service<S>, check: &str) {
    assert!(
        service
            .store()
            .load_view()
            .await
            .expect("journal")
            .targets()
            .is_empty(),
        "{check}: nothing was posted or claimed"
    );
}

async fn a_slot_maps_to_the_same_local_time_across_spring_forward<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, runs) = fixture(store).await;
    // 20:00 GMT on 21 Mar moves to 21:00 GMT.
    service
        .as_origin(admin())
        .amend_run(&runs[0], utc(3, 21, 21, 0), &policy())
        .await
        .expect("dst: source move");
    let picked = head(&service).await;
    assert_eq!(
        datetime(&service, &runs[2]).await,
        utc(4, 4, 19, 0),
        "dst: the target run is 20:00 BST"
    );
    let outcome = service
        .cherry_pick(
            "root",
            None,
            picked.seq,
            week(2),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await
        .expect("dst: pick");
    // 21:00 local in the summer-time week is 20:00 UTC, not 21:00 UTC.
    assert_eq!(datetime(&service, &runs[2]).await, utc(4, 4, 20, 0), "dst");
    assert_eq!(
        outcome.plan.steps,
        [PickStep::Amend {
            run_id: runs[2].clone(),
            to: utc(4, 4, 20, 0)
        }],
        "dst: steps"
    );
    assert!(!outcome.notices.is_empty(), "dst: notices are returned");
    journal_is_empty(&service, "dst").await;
    // One CherryPick record referencing the picked change; blame shows it.
    let record = service
        .store()
        .load_change(outcome.seq)
        .await
        .expect("record")
        .expect("record");
    assert_eq!(record.origin.surface, Surface::CherryPick, "dst");
    assert_eq!(record.origin.actor, Actor::admin("root"), "dst");
    assert_eq!(record.refs, std::slice::from_ref(&picked), "dst: refs");
    let blamed = blame(service.store(), &BlameTarget::Run(runs[2].clone()))
        .await
        .expect("blame")
        .expect("run");
    let slot = blamed
        .lines
        .iter()
        .find(|line| line.field == "slot")
        .and_then(|line| line.last.clone())
        .expect("dst: slot blamed");
    assert_eq!(slot.seq, outcome.seq, "dst");
    assert_eq!(slot.via, Via::Referenced(vec![picked]), "dst: via");
}

async fn strict_refuses_conflicts_and_force_is_bound_to_the_review<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, runs) = fixture(store).await;
    service
        .as_origin(admin())
        .amend_run(&runs[0], utc(3, 21, 21, 0), &policy())
        .await
        .expect("conflict: source move");
    let picked = head(&service).await;
    // The target was moved meanwhile: its slot is not the picked `before`.
    service
        .as_origin(admin())
        .amend_run(&runs[1], utc(3, 28, 18, 0), &policy())
        .await
        .expect("conflict: target move");
    let result = service
        .cherry_pick(
            "root",
            None,
            picked.seq,
            week(1),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await;
    let Err(PickError::Conflicts(conflicts)) = result else {
        panic!("conflict: expected Conflicts, got {result:?}");
    };
    assert_eq!(conflicts.len(), 1, "conflict");
    assert_eq!(conflicts[0].field, "slot", "conflict");
    let reviewed = service
        .preview_cherry_pick(picked.seq, week(1), &policy(), &roster())
        .await
        .expect("conflict: preview")
        .force;
    // The target moves again after the review: the force is stale.
    service
        .as_origin(admin())
        .amend_run(&runs[1], utc(3, 28, 17, 0), &policy())
        .await
        .expect("conflict: moved again");
    let theirs = head(&service).await;
    let result = service
        .cherry_pick(
            "root",
            None,
            picked.seq,
            week(1),
            PickMode::Force(reviewed),
            &policy(),
            &roster(),
        )
        .await;
    assert!(
        matches!(result, Err(PickError::PickStale { .. })),
        "force: stale review {result:?}"
    );
    assert_eq!(head(&service).await, theirs, "force: nothing written");
    // Reviewed again, the force applies and is recorded as an override.
    let reviewed = service
        .preview_cherry_pick(picked.seq, week(1), &policy(), &roster())
        .await
        .expect("conflict: preview again")
        .force;
    assert_eq!(reviewed.overrides, std::slice::from_ref(&theirs), "force");
    let force = || PickMode::Force(reviewed.clone());
    let outcome = service
        .cherry_pick(
            "root",
            Some("force-1".into()),
            picked.seq,
            week(1),
            force(),
            &policy(),
            &roster(),
        )
        .await
        .expect("conflict: forced");
    assert_eq!(
        datetime(&service, &runs[1]).await,
        utc(3, 28, 21, 0),
        "force"
    );
    assert_eq!(outcome.overridden, std::slice::from_ref(&theirs), "force");
    let record = service
        .store()
        .load_change(outcome.seq)
        .await
        .expect("record")
        .expect("record");
    assert_eq!(record.refs, [picked.clone(), theirs.clone()], "force: refs");
    assert!(
        record.notices.iter().any(|kind| kind == EDIT_OVERRIDE),
        "force: marked as an override"
    );
    let blamed = blame(service.store(), &BlameTarget::Run(runs[1].clone()))
        .await
        .expect("blame")
        .expect("run");
    let slot = blamed
        .lines
        .iter()
        .find(|line| line.field == "slot")
        .and_then(|line| line.last.clone())
        .expect("force: slot blamed");
    assert_eq!(
        slot.via,
        Via::Override {
            picked: Some(picked.clone()),
            overridden: vec![theirs]
        },
        "force: via"
    );
    // Retries: the reviewed set is part of the request.
    assert!(
        matches!(
            service
                .cherry_pick("root", Some("force-1".into()), picked.seq, week(1), force(), &policy(), &roster())
                .await,
            Err(PickError::AlreadyApplied { seq, .. }) if seq == outcome.seq
        ),
        "force: exact retry"
    );
    assert!(
        matches!(
            service
                .cherry_pick(
                    "root",
                    Some("force-1".into()),
                    picked.seq,
                    week(1),
                    PickMode::Force(Expect::default()),
                    &policy(),
                    &roster()
                )
                .await,
            Err(PickError::IdempotencyMismatch { .. })
        ),
        "force: another reviewed set"
    );
}

async fn timings_and_one_off_runs_are_unsupported<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, _runs) = fixture(store).await;
    // Change 1 added the weekly timing.
    let result = service
        .cherry_pick(
            "root",
            None,
            1,
            week(1),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await;
    assert!(
        matches!(
            result,
            Err(PickError::Refused(PickRefusal::UnsupportedPick { .. }))
        ),
        "unsupported: timing {result:?}"
    );
    let one_off = service
        .as_origin(admin())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some("900".into()),
            week_start: week(0),
            datetime: utc(3, 20, 20, 0),
            bosses: vec!["HFA".into()],
            participants: vec!["1".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        })
        .await
        .expect("one-off");
    service
        .as_origin(admin())
        .amend_run(&one_off, utc(3, 20, 21, 0), &policy())
        .await
        .expect("one-off move");
    let moved = head(&service).await;
    let result = service
        .cherry_pick(
            "root",
            None,
            moved.seq,
            week(1),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await;
    assert!(
        matches!(
            &result,
            Err(PickError::Refused(PickRefusal::UnsupportedPick { reason })) if reason.contains("one-off")
        ),
        "unsupported: one-off {result:?}"
    );
    // A rollback is undone with a revert, never picked.
    service
        .revert_changes(
            "root",
            None,
            &[moved.seq],
            RevertMode::Strict,
            &policy().reminders,
            &BTreeSet::new(),
        )
        .await
        .expect("revert");
    let revert = head(&service).await;
    let result = service
        .cherry_pick(
            "root",
            None,
            revert.seq,
            week(1),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await;
    assert!(
        matches!(result, Err(PickError::Refused(PickRefusal::Rollback))),
        "unsupported: rollback {result:?}"
    );
    // Unknown changes (and the genesis record) are unknown.
    for seq in [0, 9_999] {
        assert!(
            matches!(
                service
                    .cherry_pick(
                        "root",
                        None,
                        seq,
                        week(1),
                        PickMode::Strict,
                        &policy(),
                        &roster()
                    )
                    .await,
                Err(PickError::History(HistoryRefusal::UnknownChange(_)))
            ),
            "unsupported: unknown {seq}"
        );
    }
}

async fn answers_map_to_members_on_the_target_run<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, runs) = fixture(store).await;
    service
        .as_origin(admin())
        .set_rsvp(&runs[0], "2", RsvpState::No, RsvpSource::Chat)
        .await
        .expect("answers: source answer");
    let answered = head(&service).await;
    service
        .cherry_pick(
            "root",
            None,
            answered.seq,
            week(1),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await
        .expect("answers: pick");
    let state = snapshot(&service).await;
    let answer = state
        .rsvps
        .iter()
        .find(|row| row.run_id == runs[1] && row.user_id == "2")
        .expect("answers: mapped");
    assert_eq!(answer.state, RsvpState::No, "answers");
    // A member no longer on the target run cannot have an answer picked.
    service
        .as_origin(admin())
        .swap_participants(&runs[2], &["2".into()], &["3".into()], true, &roster())
        .await
        .expect("answers: swap");
    let result = service
        .cherry_pick(
            "root",
            None,
            answered.seq,
            week(2),
            PickMode::Force(Expect::default()),
            &policy(),
            &roster(),
        )
        .await;
    assert!(
        matches!(
            result,
            Err(PickError::Refused(PickRefusal::UnsupportedPick { .. }))
        ),
        "answers: not on the run {result:?}"
    );
}

async fn picks_are_idempotent_per_request_id<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, runs) = fixture(store).await;
    service
        .as_origin(admin())
        .amend_run(&runs[0], utc(3, 21, 21, 0), &policy())
        .await
        .expect("idempotent: source move");
    let picked = head(&service).await;
    let first = service
        .cherry_pick(
            "root",
            Some("pick-1".into()),
            picked.seq,
            week(1),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await
        .expect("idempotent: pick");
    assert!(
        matches!(
            service
                .cherry_pick(
                    "root",
                    Some("pick-1".into()),
                    picked.seq,
                    week(1),
                    PickMode::Strict,
                    &policy(),
                    &roster(),
                )
                .await,
            Err(PickError::AlreadyApplied { seq, .. }) if seq == first.seq
        ),
        "idempotent: exact retry"
    );
    assert!(
        matches!(
            service
                .cherry_pick(
                    "root",
                    Some("pick-1".into()),
                    picked.seq,
                    week(2),
                    PickMode::Strict,
                    &policy(),
                    &roster(),
                )
                .await,
            Err(PickError::IdempotencyMismatch { seq }) if seq == first.seq
        ),
        "idempotent: another request"
    );
    // Another instant in the same boss week is the same request.
    assert!(
        matches!(
            service
                .cherry_pick(
                    "root",
                    Some("pick-1".into()),
                    picked.seq,
                    utc(3, 29, 12, 0),
                    PickMode::Strict,
                    &policy(),
                    &roster(),
                )
                .await,
            Err(PickError::AlreadyApplied { seq, .. }) if seq == first.seq
        ),
        "idempotent: same week, other instant"
    );
    // Picking again (no request id) finds everything in place.
    assert!(
        matches!(
            service
                .cherry_pick(
                    "root",
                    None,
                    picked.seq,
                    week(1),
                    PickMode::Strict,
                    &policy(),
                    &roster()
                )
                .await,
            Err(PickError::NoEffect)
        ),
        "idempotent: repeat is no effect"
    );
    assert_eq!(
        head(&service).await.seq,
        first.seq,
        "idempotent: applied once"
    );
}

async fn weeks_without_a_target_run_and_bad_weeks_are_refused<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, fixed, runs) = fixture(store).await;
    service
        .as_origin(admin())
        .amend_run(&runs[0], utc(3, 21, 21, 0), &policy())
        .await
        .expect("weeks: source move");
    let picked = head(&service).await;
    let pick = |target: DateTime<Utc>| (picked.seq, target);
    for (target, check) in [
        (utc(4, 9, 12, 0), "no target run"),
        (utc(3, 20, 12, 0), "same week"),
        (utc(3, 12, 12, 0), "past week"),
    ] {
        let (seq, target) = pick(target);
        let result = service
            .cherry_pick(
                "root",
                None,
                seq,
                target,
                PickMode::Force(Expect::default()),
                &policy(),
                &roster(),
            )
            .await;
        let expected = match check {
            "no target run" => matches!(
                &result,
                Err(PickError::Refused(PickRefusal::NoTargetRun { fixed_run_id })) if *fixed_run_id == fixed
            ),
            "same week" => matches!(result, Err(PickError::Refused(PickRefusal::SameWeek))),
            _ => matches!(result, Err(PickError::Refused(PickRefusal::WeekPassed))),
        };
        assert!(expected, "weeks: {check} {result:?}");
    }
    assert_eq!(head(&service).await, picked, "weeks: nothing written");
}

async fn previews_write_nothing<S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal>(
    store: S,
) {
    let (mut service, _fixed, runs) = fixture(store).await;
    service
        .as_origin(admin())
        .amend_run(&runs[0], utc(3, 21, 21, 0), &policy())
        .await
        .expect("preview: source move");
    let picked = head(&service).await;
    let preview = service
        .preview_cherry_pick(picked.seq, week(2), &policy(), &roster())
        .await
        .expect("preview");
    assert_eq!(
        preview.plan.target_week,
        week(2),
        "preview: the resulting week"
    );
    assert_eq!(preview.plan.source_week, week(0), "preview");
    assert!(preview.plan.conflicts.is_empty(), "preview");
    assert!(!preview.no_effect, "preview");
    assert!(!preview.notices.is_empty(), "preview: notices listed");
    assert_eq!(head(&service).await, picked, "preview: nothing written");
    assert_eq!(
        datetime(&service, &runs[2]).await,
        utc(4, 4, 19, 0),
        "preview"
    );
    journal_is_empty(&service, "preview").await;
}

async fn status_of<S: ScheduleStore>(service: &Service<S>, run: &str) -> RunStatus {
    snapshot(service)
        .await
        .runs
        .iter()
        .find(|row| row.id == run)
        .expect("run")
        .status
}

async fn react<S: ScheduleStore>(service: &mut Service<S>, run: &str, user: &str, emoji: &str) {
    service
        .as_origin(Origin::new(Actor::member(user), Surface::Discord))
        .apply_reaction(run, user, emoji, true)
        .await
        .expect("reaction");
}

async fn picked_answers_recount_the_target_status<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, runs) = fixture(store).await;
    // A ❌ puts the source at risk; picked, it puts the target at risk.
    react(&mut service, &runs[0], "2", EMOJI_NO).await;
    let declined = head(&service).await;
    assert_eq!(
        status_of(&service, &runs[0]).await,
        RunStatus::AtRisk,
        "recount"
    );
    service
        .cherry_pick(
            "root",
            None,
            declined.seq,
            week(1),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await
        .expect("recount: pick no");
    assert_eq!(
        status_of(&service, &runs[1]).await,
        RunStatus::AtRisk,
        "recount: at risk"
    );
    // A ✅ that completes the target's tally confirms it.
    react(&mut service, &runs[2], "2", EMOJI_YES).await;
    react(&mut service, &runs[0], "1", EMOJI_YES).await;
    let accepted = head(&service).await;
    service
        .cherry_pick(
            "root",
            None,
            accepted.seq,
            week(2),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await
        .expect("recount: pick yes");
    assert_eq!(
        status_of(&service, &runs[2]).await,
        RunStatus::Confirmed,
        "recount: confirmed"
    );
}

async fn finished_and_otot_targets<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, runs) = fixture(store).await;
    service
        .as_origin(admin())
        .amend_run(&runs[0], utc(3, 21, 21, 0), &policy())
        .await
        .expect("finished: source move");
    let moved = head(&service).await;
    let set = |status: RunStatus| StatusChange {
        status,
        announce: false,
        via_portal: true,
    };
    service
        .as_origin(admin())
        .set_status(&runs[1], set(RunStatus::Cancelled), &policy().reminders)
        .await
        .expect("finished: cancel");
    for mode in [PickMode::Strict, PickMode::Force(Expect::default())] {
        let result = service
            .cherry_pick("root", None, moved.seq, week(1), mode, &policy(), &roster())
            .await;
        assert!(
            matches!(
                result,
                Err(PickError::Refused(PickRefusal::TargetFinished {
                    status: RunStatus::Cancelled
                }))
            ),
            "finished: {result:?}"
        );
    }
    service
        .as_origin(admin())
        .set_status(&runs[2], set(RunStatus::Otot), &policy().reminders)
        .await
        .expect("finished: otot");
    let result = service
        .cherry_pick(
            "root",
            None,
            moved.seq,
            week(2),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await;
    assert!(
        matches!(
            result,
            Err(PickError::Refused(PickRefusal::UnsupportedPick { .. }))
        ),
        "otot: no move {result:?}"
    );
    service
        .as_origin(admin())
        .set_rsvp(&runs[0], "2", RsvpState::Yes, RsvpSource::Chat)
        .await
        .expect("otot: answer");
    let answered = head(&service).await;
    service
        .cherry_pick(
            "root",
            None,
            answered.seq,
            week(2),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await
        .expect("otot: answers pick");
    assert_eq!(
        status_of(&service, &runs[2]).await,
        RunStatus::Otot,
        "otot stays"
    );
}

async fn a_forced_field_without_history_is_still_an_override<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, runs) = fixture(store).await;
    react(&mut service, &runs[0], "2", EMOJI_YES).await;
    react(&mut service, &runs[0], "2", EMOJI_NO).await;
    let changed = head(&service).await;
    // The target never had an answer from 2: the conflicting field has no
    // recorded change, so nothing can be named as overridden.
    let preview = service
        .preview_cherry_pick(changed.seq, week(1), &policy(), &roster())
        .await
        .expect("no history: preview");
    assert_eq!(preview.force.fields.len(), 1, "no history");
    assert_eq!(preview.force.fields[0].seen, None, "no history");
    assert!(preview.force.overrides.is_empty(), "no history");
    let outcome = service
        .cherry_pick(
            "root",
            None,
            changed.seq,
            week(1),
            PickMode::Force(preview.force),
            &policy(),
            &roster(),
        )
        .await
        .expect("no history: forced");
    let record = service
        .store()
        .load_change(outcome.seq)
        .await
        .expect("record")
        .expect("record");
    assert!(
        record.notices.iter().any(|kind| kind == EDIT_OVERRIDE),
        "no history: marked"
    );
    assert_eq!(
        record.refs,
        std::slice::from_ref(&changed),
        "no history: refs"
    );
    let blamed = blame(service.store(), &BlameTarget::Run(runs[1].clone()))
        .await
        .expect("blame")
        .expect("run");
    let line = blamed
        .lines
        .iter()
        .find(|line| line.field == "rsvp:2")
        .and_then(|line| line.last.clone())
        .expect("no history: blamed");
    assert_eq!(
        line.via,
        Via::Override {
            picked: Some(changed),
            overridden: Vec::new()
        },
        "no history: via"
    );
}

async fn swaps_and_sole_statuses_pick<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, runs) = fixture(store).await;
    for run in &runs[..2] {
        react(&mut service, run, "2", EMOJI_YES).await;
    }
    // 2 leaves (their answer goes with them); 3 joins.
    service
        .as_origin(admin())
        .swap_participants(&runs[0], &["2".into()], &["3".into()], true, &roster())
        .await
        .expect("swap: source");
    let swapped = head(&service).await;
    service
        .cherry_pick(
            "root",
            None,
            swapped.seq,
            week(1),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await
        .expect("swap: pick");
    let state = snapshot(&service).await;
    let target = state
        .runs
        .iter()
        .find(|row| row.id == runs[1])
        .expect("run");
    let mut party = target.participants.clone();
    party.sort();
    assert_eq!(party, ["1", "3"], "swap");
    assert!(
        !state
            .rsvps
            .iter()
            .any(|row| row.run_id == runs[1] && row.user_id == "2"),
        "swap: the leaver's answer is gone"
    );
    // A status change on its own is picked.
    service
        .as_origin(admin())
        .set_status(
            &runs[0],
            StatusChange {
                status: RunStatus::Otot,
                announce: false,
                via_portal: true,
            },
            &policy().reminders,
        )
        .await
        .expect("status: source");
    let status = head(&service).await;
    service
        .cherry_pick(
            "root",
            None,
            status.seq,
            week(2),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await
        .expect("status: pick");
    assert_eq!(
        status_of(&service, &runs[2]).await,
        RunStatus::Otot,
        "status"
    );
}

async fn a_forced_swap_drops_members_already_gone<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, runs) = fixture(store).await;
    service
        .as_origin(admin())
        .swap_participants(&runs[0], &["2".into()], &["3".into()], true, &roster())
        .await
        .expect("forced swap: source");
    let swapped = head(&service).await;
    // 2 already left the target for 4.
    service
        .as_origin(admin())
        .swap_participants(&runs[1], &["2".into()], &["4".into()], true, &roster())
        .await
        .expect("forced swap: upstream");
    let preview = service
        .preview_cherry_pick(swapped.seq, week(1), &policy(), &roster())
        .await
        .expect("forced swap: preview");
    assert_eq!(preview.plan.conflicts.len(), 1, "forced swap");
    service
        .cherry_pick(
            "root",
            None,
            swapped.seq,
            week(1),
            PickMode::Force(preview.force),
            &policy(),
            &roster(),
        )
        .await
        .expect("forced swap: no NotOnRun");
    let state = snapshot(&service).await;
    let mut party = state
        .runs
        .iter()
        .find(|row| row.id == runs[1])
        .expect("run")
        .participants
        .clone();
    party.sort();
    assert_eq!(party, ["1", "3", "4"], "forced swap");
}

async fn a_shifted_source_slot_is_not_a_conflict<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, _runs) = fixture(store).await;
    // Sunday 01:30 London: on 29 Mar that wall time does not exist.
    let night = service
        .as_origin(admin())
        .add_fixed_run(NewFixedRun {
            owner_id: "1".into(),
            channel_id: Some("900".into()),
            bosses: vec!["HFA".into()],
            weekday: Weekday::Sun,
            time: NaiveTime::from_hms_opt(1, 30, 0).expect("valid time"),
            participants: vec!["1".into(), "2".into()],
            note: None,
        })
        .await
        .expect("shifted: timing");
    service
        .as_origin(admin())
        .materialise_weeks(&policy())
        .await
        .expect("shifted: materialise");
    let state = snapshot(&service).await;
    let run_in = |index: u32| {
        state
            .runs
            .iter()
            .find(|row| {
                row.fixed_run_id.as_deref() == Some(night.as_str()) && row.week_start == week(index)
            })
            .expect("shifted: run")
            .clone()
    };
    let (source, target) = (run_in(1), run_in(2));
    assert_eq!(target.datetime, utc(4, 5, 0, 30), "shifted: 01:30 BST");
    // 04:00 BST on 29 Mar.
    service
        .as_origin(admin())
        .amend_run(&source.id, utc(3, 29, 3, 0), &policy())
        .await
        .expect("shifted: source move");
    let moved = head(&service).await;
    service
        .cherry_pick(
            "root",
            None,
            moved.seq,
            week(2),
            PickMode::Strict,
            &policy(),
            &roster(),
        )
        .await
        .expect("shifted: no false conflict");
    assert_eq!(
        datetime(&service, &target.id).await,
        utc(4, 5, 3, 0),
        "shifted: 04:00 BST"
    );
}

/// When the race fires, relative to the wrapped call.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Race {
    /// Before the first version read (between the preview's value and
    /// version reads).
    BeforeVersions,
    /// Before the first commit that carries expectations (inside the
    /// forced pick's commit window).
    BeforeCommit,
}

/// Moves `run` an hour earlier once, at `race`.
struct Racing<'a, S> {
    inner: &'a S,
    race: Race,
    run: String,
    fired: std::sync::atomic::AtomicBool,
}

impl<S: ScheduleStore + Sync> Racing<'_, S> {
    async fn fire(&self) {
        if self.fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let state = self.inner.load(&Scope::All).await.expect("race: load");
        let mut row = state
            .runs
            .iter()
            .find(|row| row.id == self.run)
            .expect("race: run")
            .clone();
        row.datetime -= chrono::TimeDelta::hours(1);
        let meta = crate::domain::history::ChangeMeta {
            origin: admin(),
            at: utc(3, 19, 1, 0),
            notices: Vec::new(),
            refs: Vec::new(),
            request_digest: None,
            expect: Expect::default(),
        };
        self.inner
            .commit(
                state.revision,
                crate::domain::schedule::ChangeSet {
                    changes: vec![crate::domain::schedule::Change::PutRun(row)],
                },
                meta,
            )
            .await
            .expect("race: upstream");
    }
}

impl<S: ScheduleStore + Sync> ScheduleStore for Racing<'_, S> {
    async fn load(
        &self,
        scope: &Scope,
    ) -> Result<ScheduleSnapshot, crate::domain::scheduler::StoreError> {
        self.inner.load(scope).await
    }

    async fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> Result<
        Option<crate::domain::scheduler::RecordedRequest>,
        crate::domain::scheduler::StoreError,
    > {
        self.inner.recorded_request(actor, request_id).await
    }

    async fn commit(
        &self,
        expected_revision: u64,
        changes: crate::domain::schedule::ChangeSet,
        meta: crate::domain::history::ChangeMeta,
    ) -> Result<Option<crate::domain::scheduler::Committed>, crate::domain::scheduler::StoreError>
    {
        if self.race == Race::BeforeCommit && !meta.expect.is_empty() {
            self.fire().await;
        }
        self.inner.commit(expected_revision, changes, meta).await
    }
}

impl<S: ChangeHistory + Sync> ChangeHistory for Racing<'_, S> {
    async fn load_change(
        &self,
        seq: u64,
    ) -> Result<Option<crate::domain::history::ChangeRecord>, crate::domain::scheduler::StoreError>
    {
        self.inner.load_change(seq).await
    }

    async fn load_checked(
        &self,
        seq: u64,
    ) -> Result<crate::domain::history::CheckedChange, crate::domain::scheduler::StoreError> {
        self.inner.load_checked(seq).await
    }

    async fn list_changes(
        &self,
        query: &crate::domain::history::ChangeQuery,
    ) -> Result<crate::domain::history::ChangePage, crate::domain::scheduler::StoreError> {
        self.inner.list_changes(query).await
    }

    async fn count_changes(
        &self,
        filter: &crate::domain::history::ChangeFilter,
    ) -> Result<u64, crate::domain::scheduler::StoreError> {
        self.inner.count_changes(filter).await
    }

    async fn verify_history(
        &self,
    ) -> Result<crate::domain::history::HistoryVerification, crate::domain::scheduler::StoreError>
    {
        self.inner.verify_history().await
    }

    async fn history_head(&self) -> Result<ChangeRef, crate::domain::scheduler::StoreError> {
        self.inner.history_head().await
    }
}

impl<S: ScheduleStore + BlameIndex + Sync> BlameIndex for Racing<'_, S> {
    async fn last_changes(
        &self,
        target: &BlameTarget,
    ) -> Result<std::collections::BTreeMap<String, u64>, crate::domain::scheduler::StoreError> {
        if self.race == Race::BeforeVersions {
            self.fire().await;
        }
        self.inner.last_changes(target).await
    }

    async fn read_versioned(
        &self,
        targets: &[BlameTarget],
    ) -> Result<Vec<crate::domain::history::Versioned>, crate::domain::scheduler::StoreError> {
        self.inner.read_versioned(targets).await
    }
}

fn racing<'a, S>(inner: &'a S, race: Race, run: &str) -> Racing<'a, S> {
    Racing {
        inner,
        race,
        run: run.to_owned(),
        fired: std::sync::atomic::AtomicBool::new(false),
    }
}

/// A source slot move of run 0 and an upstream move of run 1: one slot
/// conflict when picking into week 1.
async fn conflicted<S: ScheduleStore + ChangeHistory + BlameIndex>(
    store: S,
) -> (S, [String; 3], ChangeRef) {
    let (mut service, _fixed, runs) = fixture(store).await;
    service
        .as_origin(admin())
        .amend_run(&runs[0], utc(3, 21, 21, 0), &policy())
        .await
        .expect("race: source move");
    let picked = head(&service).await;
    service
        .as_origin(admin())
        .amend_run(&runs[1], utc(3, 28, 18, 0), &policy())
        .await
        .expect("race: target move");
    (service.into_store(), runs, picked)
}

async fn a_preview_reads_values_and_versions_consistently<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal + Sync,
>(
    store: S,
) {
    let (store, runs, picked) = conflicted(store).await;
    let racy = racing(&store, Race::BeforeVersions, &runs[1]);
    let service = SchedulerService::new(racy, Ids::default(), Now);
    let preview = service
        .preview_cherry_pick(picked.seq, week(1), &policy(), &roster())
        .await
        .expect("race: preview");
    let newest = store.history_head().await.expect("head");
    // The value the admin sees and the version they echo are the newest.
    assert_eq!(
        preview.plan.conflicts[0].current,
        crate::domain::history::PickValue::Slot(utc(3, 28, 17, 0)),
        "race: current value"
    );
    assert_eq!(
        preview.force.overrides,
        [newest],
        "race: versions match the value"
    );
    assert!(
        preview.strict_refuses,
        "race: strict refuses these conflicts"
    );
}

async fn a_change_inside_the_forced_commit_is_stale<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal + Sync,
>(
    store: S,
) {
    let (store, runs, picked) = conflicted(store).await;
    // A wrapper that has already fired: the preview sees no race.
    let calm = racing(&store, Race::BeforeVersions, &runs[1]);
    calm.fired.store(true, std::sync::atomic::Ordering::SeqCst);
    let reviewed = SchedulerService::new(calm, Ids::default(), Now)
        .preview_cherry_pick(picked.seq, week(1), &policy(), &roster())
        .await
        .expect("stale commit: preview")
        .force;
    let racy = racing(&store, Race::BeforeCommit, &runs[1]);
    let mut service = SchedulerService::new(racy, Ids::default(), Now);
    let result = service
        .cherry_pick(
            "root",
            None,
            picked.seq,
            week(1),
            PickMode::Force(reviewed),
            &policy(),
            &roster(),
        )
        .await;
    assert!(
        matches!(result, Err(PickError::PickStale { .. })),
        "stale commit: the store's in-transaction check {result:?}"
    );
    // Only the racing move landed.
    let head = store.history_head().await.expect("head");
    let record = store
        .load_change(head.seq)
        .await
        .expect("record")
        .expect("record");
    assert_ne!(record.origin.surface, Surface::CherryPick, "stale commit");
}

async fn an_emptied_trimmed_delta_is_no_effect_in_both_modes<
    S: ScheduleStore + ChangeHistory + BlameIndex + DeliveryJournal,
>(
    store: S,
) {
    let (mut service, _fixed, runs) = fixture(store).await;
    service
        .as_origin(admin())
        .swap_participants(&runs[0], &["2".into()], &["3".into()], true, &roster())
        .await
        .expect("trimmed: source swap");
    let swapped = head(&service).await;
    // The target already lost 2 and has 3, plus 4: nothing left to do.
    service
        .as_origin(admin())
        .swap_participants(
            &runs[1],
            &["2".into()],
            &["3".into(), "4".into()],
            true,
            &roster(),
        )
        .await
        .expect("trimmed: upstream");
    let preview = service
        .preview_cherry_pick(swapped.seq, week(1), &policy(), &roster())
        .await
        .expect("trimmed: preview");
    assert!(preview.no_effect, "trimmed: preview says no effect");
    assert!(!preview.strict_refuses, "trimmed: strict does not refuse");
    assert!(preview.plan.conflicts.is_empty(), "trimmed");
    for mode in [PickMode::Strict, PickMode::Force(preview.force.clone())] {
        assert!(
            matches!(
                service
                    .cherry_pick(
                        "root",
                        None,
                        swapped.seq,
                        week(1),
                        mode,
                        &policy(),
                        &roster()
                    )
                    .await,
                Err(PickError::NoEffect)
            ),
            "trimmed: both modes"
        );
    }
}
