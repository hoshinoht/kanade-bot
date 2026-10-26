//! v5-only mutation behaviour: choices for amended runs on a weekly-timing
//! edit, and resetting an amended run to its weekly timing.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, Utc, Weekday};
use kanade::infrastructure::store::SqliteStore;
use kanade_domain::history::Origin;
use kanade_domain::notify::NoticeOutbox;
use kanade_domain::schedule::{
    AmendedRunChoice, EMOJI_NO, EMOJI_YES, FixedEdit, FixedEditChoices, FixedEditRequest, FixedRun,
    NewRun, NoticeChange, Outcome, Reminder, RunSource, RunState, RunStatus, ScheduleError,
    ScheduleSnapshot, StatusChange,
};
use kanade_domain::scheduler::{SchedulerError, SchedulerResult, SchedulerService};

use crate::common::{TestClock, assert_sound};
use crate::support::{
    CountingIds, Guild, OneTiming as Fixture, by_tests, hm, kl, one_timing, owned, policy,
    snapshot, utc, weekly,
};

type Service = SchedulerService<SqliteStore, CountingIds, TestClock>;

/// [`one_timing`] with the 14 Sep run cancelled: runs by week are 31 Aug,
/// 7 Sep (amended to Wed 9 Sep 21:00) and 14 Sep (cancelled).
async fn fixture() -> Fixture {
    let mut f = one_timing().await;
    set_status(&mut f.service, &f.runs[2], RunStatus::Cancelled, false).await;
    f
}

async fn set_status(service: &mut Service, run: &str, status: RunStatus, announce: bool) {
    let change = StatusChange {
        status,
        announce,
        via_portal: true,
    };
    by_tests(service)
        .set_status(run, change, &policy().reminders)
        .await
        .unwrap();
}

async fn react(service: &mut Service, run: &str, user: &str, emoji: &str) {
    let reacted = by_tests(service).apply_reaction(run, user, emoji, true);
    reacted.await.unwrap();
}

async fn apply_edit(
    service: &mut Service,
    fixed: &str,
    edit: FixedEdit,
    choices: FixedEditChoices,
) -> SchedulerResult<Outcome<FixedRun>> {
    let request = FixedEditRequest {
        fixed_id: fixed.into(),
        edit,
        choices,
    };
    by_tests(service)
        .apply_fixed_edit(&request, &Guild, &policy())
        .await
}

async fn reset(service: &mut Service, run: &str) -> SchedulerResult<Outcome<RunState>> {
    by_tests(service).reset_to_fixed(run, &policy()).await
}

/// Mark the run's morning card as posted with `message`.
async fn send_day_of(service: &mut Service, run: &str, message: &str) -> Reminder {
    let rows = service.reminders(run).await.unwrap();
    let day_of = rows.into_iter().find(|row| row.kind == "day_of").unwrap();
    let sent = by_tests(service).mark_reminder_sent(&day_of.id, Some(message));
    sent.await.unwrap();
    day_of
}

fn run_at(state: &ScheduleSnapshot, id: &str) -> DateTime<Utc> {
    state.runs.iter().find(|run| run.id == id).unwrap().datetime
}

/// `(kind, fire_at, sent_at)` of one run's reminders, ids ignored.
fn pings(
    state: &ScheduleSnapshot,
    run: &str,
) -> Vec<(String, DateTime<Utc>, Option<DateTime<Utc>>)> {
    state
        .reminders
        .iter()
        .filter(|row| row.run_id == run)
        .map(|row| (row.kind.clone(), row.fire_at, row.sent_at))
        .collect()
}

fn reminder_rows(state: &ScheduleSnapshot, run: &str) -> Vec<Reminder> {
    let rows = state.reminders.iter().filter(|row| row.run_id == run);
    rows.cloned().collect()
}

fn time_edit(hour: u32, minute: u32) -> FixedEdit {
    FixedEdit {
        time: Some(hm(hour, minute)),
        ..FixedEdit::default()
    }
}

fn per_run(entries: &[(&str, AmendedRunChoice)]) -> FixedEditChoices {
    FixedEditChoices::PerRun(
        entries
            .iter()
            .map(|(id, choice)| ((*id).to_owned(), *choice))
            .collect::<BTreeMap<_, _>>(),
    )
}

#[tokio::test]
async fn weekly_timing_add_and_remove_enqueue_notices_once_in_memory() {
    let clock = TestClock::new(kl(27, 1, 0));
    let mut service = SchedulerService::new(
        SqliteStore::open_temp().await,
        CountingIds::default(),
        clock,
    );
    let timing = || weekly("222", Weekday::Mon, hm(21, 30), &["1001"]);
    let keyed = |key: &str| Origin::for_tests().with_request_id(key);
    let added = service
        .as_origin(keyed("fixed-add"))
        .add_fixed_run(timing())
        .await
        .unwrap();
    let notices = service.store().outbox_notices().await.unwrap();
    assert!(matches!(
        &notices[..],
        [row] if matches!(&row.notice.change, NoticeChange::FixedAdded { fixed_id, .. } if fixed_id == &added)
    ));
    assert!(matches!(
        service
            .as_origin(keyed("fixed-add"))
            .add_fixed_run(timing())
            .await,
        Err(SchedulerError::AlreadyApplied { .. })
    ));
    assert_eq!(service.store().outbox_notices().await.unwrap().len(), 1);

    service
        .as_origin(keyed("fixed-remove"))
        .retire_fixed_run(&added, &[kl(27, 0, 0)], &policy().reminders)
        .await
        .unwrap();
    let notices = service.store().outbox_notices().await.unwrap();
    assert!(matches!(
        notices.last().map(|row| &row.notice.change),
        Some(NoticeChange::FixedRemoved { fixed_id, .. }) if fixed_id == &added
    ));
}

#[tokio::test]
async fn preview_lists_only_live_amended_runs_a_day_or_time_edit_would_move() {
    let f = fixture().await;
    let policy = policy();
    let preview = async |edit: FixedEdit| {
        let affected = f.service.preview_fixed_edit(&f.fixed, &edit, &policy);
        affected.await.unwrap()
    };
    let affected = preview(time_edit(22, 0)).await;
    assert_eq!(affected.len(), 1);
    let amended = &affected[0];
    assert_eq!(amended.run_id, f.runs[1]);
    assert_eq!(amended.datetime, utc(kl(9, 21, 0)));
    assert_eq!(amended.fixed_slot, utc(kl(7, 21, 30)));
    assert_eq!(amended.new_slot, utc(kl(7, 22, 0)));
    let note = FixedEdit {
        note: Some("bring pots".into()),
        ..FixedEdit::default()
    };
    assert!(preview(note).await.is_empty());
    // Editing to the slot the amended run already sits on moves nothing.
    let onto = FixedEdit {
        weekday: Some(Weekday::Wed),
        ..time_edit(21, 0)
    };
    assert!(preview(onto).await.is_empty());
}

#[tokio::test]
async fn keeping_an_amended_run_leaves_its_slot_and_pings_while_others_follow() {
    let mut f = fixture().await;
    // The amended run's morning ping already went out with a card.
    let amended_day_of = send_day_of(&mut f.service, &f.runs[1], "555").await;
    let before = snapshot(&f.service).await;
    let keep = per_run(&[(&f.runs[1], AmendedRunChoice::KeepForThisWeek)]);
    let outcome = apply_edit(&mut f.service, &f.fixed, time_edit(22, 0), keep.clone())
        .await
        .unwrap();
    assert_eq!(outcome.value.time, hm(22, 0));
    let after = snapshot(&f.service).await;
    assert_eq!(
        run_at(&after, &f.runs[0]),
        utc(kl(31, 22, 0)),
        "unamended week follows"
    );
    assert_eq!(run_at(&after, &f.runs[1]), utc(kl(9, 21, 0)), "kept slot");
    assert_eq!(
        run_at(&after, &f.runs[2]),
        utc(kl(14, 21, 30)),
        "cancelled untouched"
    );
    let kept = reminder_rows(&after, &f.runs[1]);
    assert_eq!(
        kept,
        reminder_rows(&before, &f.runs[1]),
        "kept run's pings untouched"
    );
    assert!(
        kept.iter()
            .any(|row| row.id == amended_day_of.id && row.message_id.as_deref() == Some("555"))
    );
    assert_sound(
        &after,
        utc(kl(27, 1, 0)),
        Some(&f.runs.iter().collect::<Vec<_>>()),
    );

    // Idempotent: the same edit and choice again changes no slot or ping.
    let again = apply_edit(&mut f.service, &f.fixed, time_edit(22, 0), keep)
        .await
        .unwrap();
    assert_eq!(again.value, outcome.value);
    let twice = snapshot(&f.service).await;
    assert_eq!(twice.runs, after.runs);
    for run in &f.runs {
        assert_eq!(pings(&twice, run), pings(&after, run), "run {run}");
    }

    // Later weeks follow the new slot once they materialise.
    f.clock.set(kl(10, 1, 0));
    by_tests(&mut f.service)
        .materialise_weeks(&policy())
        .await
        .unwrap();
    let later = snapshot(&f.service).await;
    assert!(
        later
            .runs
            .iter()
            .any(|run| run.datetime == utc(kl(21, 22, 0)))
    );
    assert_eq!(run_at(&later, &f.runs[1]), utc(kl(9, 21, 0)));
}

#[tokio::test]
async fn updating_an_amended_run_matches_the_v4_default() {
    let mut chosen = fixture().await;
    let mut default = fixture().await;
    let update = per_run(&[(&chosen.runs[1], AmendedRunChoice::UpdateToFixed)]);
    apply_edit(&mut chosen.service, &chosen.fixed, time_edit(22, 0), update)
        .await
        .unwrap();
    by_tests(&mut default.service)
        .update_fixed(&default.fixed, time_edit(22, 0), &Guild, &policy())
        .await
        .unwrap();
    let state = snapshot(&chosen.service).await;
    assert_eq!(state, snapshot(&default.service).await);
    assert_eq!(run_at(&state, &chosen.runs[1]), utc(kl(7, 22, 0)));
    assert_eq!(pings(&state, &chosen.runs[1]).len(), 3);
    assert_sound(
        &state,
        utc(kl(27, 1, 0)),
        Some(&chosen.runs.iter().collect::<Vec<_>>()),
    );
}

#[tokio::test]
async fn choices_must_name_exactly_the_affected_runs() {
    let mut f = fixture().await;
    let before = snapshot(&f.service).await;
    let keep = AmendedRunChoice::KeepForThisWeek;
    let note = FixedEdit {
        note: Some("x".into()),
        ..FixedEdit::default()
    };
    let cases = [
        (
            time_edit(22, 0),
            per_run(&[(&f.runs[1], keep), (&f.runs[0], keep)]),
            ScheduleError::UnexpectedChoice(f.runs[0].clone()),
        ),
        (
            time_edit(22, 0),
            per_run(&[(&f.runs[2], AmendedRunChoice::UpdateToFixed)]),
            ScheduleError::UnexpectedChoice(f.runs[2].clone()),
        ),
        (
            time_edit(22, 0),
            per_run(&[]),
            ScheduleError::MissingChoice(f.runs[1].clone()),
        ),
        // A note-only edit affects nothing, so any per-run choice is refused.
        (
            note,
            per_run(&[(&f.runs[1], keep)]),
            ScheduleError::UnexpectedChoice(f.runs[1].clone()),
        ),
    ];
    for (edit, choices, expected) in cases {
        let error = apply_edit(&mut f.service, &f.fixed, edit, choices).await;
        assert_eq!(error.unwrap_err(), SchedulerError::Schedule(expected));
    }
    assert_eq!(snapshot(&f.service).await, before, "refusals write nothing");
}

#[tokio::test]
async fn reset_restores_the_weekly_slot_party_and_channel() {
    let mut f = fixture().await;
    let run = &f.runs[1];
    by_tests(&mut f.service)
        .swap_participants(run, &owned(&["1002"]), &owned(&["1003"]), true, &Guild)
        .await
        .unwrap();
    for user in ["1001", "1003"] {
        react(&mut f.service, run, user, EMOJI_YES).await;
    }
    let status = snapshot(&f.service).await;
    let status = status.runs.iter().find(|r| &r.id == run).unwrap().status;
    assert_eq!(status, RunStatus::Confirmed);
    // Late enough that the restored slot's morning and T-1h pings have passed.
    f.clock.set(kl(7, 21, 0));
    let outcome = reset(&mut f.service, run).await.unwrap();
    let state = &outcome.value;
    assert_eq!(state.run.datetime, utc(kl(7, 21, 30)));
    assert_eq!(state.run.participants, ["1001", "1002"]);
    assert_eq!(state.run.channel_id.as_deref(), Some("222"));
    // The amend status rule, then a re-derive: 1002 is unknown inside the
    // 12 h window.
    assert_eq!(state.run.status, RunStatus::AtRisk, "re-derived");
    assert_eq!(
        state.rsvps.keys().collect::<Vec<_>>(),
        ["1001"],
        "the member taken off loses their answer"
    );
    assert!(!state.roster_change.changed());
    assert!(matches!(
        outcome.notices.as_slice(),
        [intent] if matches!(intent.change, NoticeChange::RunReset { .. })
            && intent.effect_kind() == "notice.run.reset.restored"
    ));
    let after = snapshot(&f.service).await;
    let now = utc(kl(7, 21, 0));
    assert_sound(&after, now, Some(&[run]));
    let sent: Vec<(String, bool)> = pings(&after, run)
        .into_iter()
        .map(|(kind, _, sent)| (kind, sent.is_some()))
        .collect();
    assert_eq!(
        sent,
        [
            ("day_of".to_owned(), true),
            ("countdown_60".to_owned(), true),
            ("countdown_15".to_owned(), false)
        ]
    );

    // Idempotent: a matching run is left exactly as it is, quietly.
    let again = reset(&mut f.service, run).await.unwrap();
    assert!(again.notices.is_empty());
    assert_eq!(snapshot(&f.service).await, after);
}

#[tokio::test]
async fn reset_refuses_one_offs_retired_timings_and_finished_runs() {
    let mut f = fixture().await;
    let one_off = by_tests(&mut f.service)
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some("333".into()),
            week_start: utc(kl(27, 0, 0)),
            datetime: utc(kl(29, 22, 0)),
            bosses: vec!["XKalos".into()],
            participants: vec!["1001".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        })
        .await
        .unwrap();
    let before = snapshot(&f.service).await;
    let refusal = async |service: &mut Service, run: &str| match reset(service, run).await {
        Err(SchedulerError::Schedule(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert_eq!(
        refusal(&mut f.service, &one_off).await,
        ScheduleError::NotAFixedRun(one_off.clone())
    );
    assert_eq!(
        refusal(&mut f.service, &f.runs[2]).await,
        ScheduleError::RunNotLive {
            run_id: f.runs[2].clone(),
            status: "cancelled".into()
        }
    );
    set_status(&mut f.service, &f.runs[0], RunStatus::Done, false).await;
    assert!(matches!(
        refusal(&mut f.service, &f.runs[0]).await,
        ScheduleError::RunNotLive { .. }
    ));
    assert_eq!(
        snapshot(&f.service)
            .await
            .runs
            .iter()
            .find(|r| r.id == f.runs[2]),
        before.runs.iter().find(|r| r.id == f.runs[2]),
        "cancelled run untouched"
    );
    // Retire only the first week's timing: the amended run keeps a dangling link.
    let first_week = [kl(27, 0, 0)];
    by_tests(&mut f.service)
        .retire_fixed_run(&f.fixed, &first_week, &policy().reminders)
        .await
        .unwrap();
    assert_eq!(
        refusal(&mut f.service, &f.runs[1]).await,
        ScheduleError::FixedRunRetired(f.fixed.clone())
    );
    assert_eq!(
        run_at(&snapshot(&f.service).await, &f.runs[1]),
        utc(kl(9, 21, 0))
    );
}

#[tokio::test]
async fn a_fixed_edit_removing_the_lone_decliner_recomputes_status_like_swap() {
    // New line-up still missing an answer -> planned; only yes-sayers left -> confirmed.
    for (line_up, expected) in [
        (vec!["1001", "1003"], RunStatus::Planned),
        (vec!["1001"], RunStatus::Confirmed),
    ] {
        let mut f = fixture().await;
        let run = &f.runs[0];
        react(&mut f.service, run, "1001", EMOJI_YES).await;
        react(&mut f.service, run, "1002", EMOJI_NO).await;
        let status =
            |state: &ScheduleSnapshot| state.runs.iter().find(|r| &r.id == run).unwrap().status;
        assert_eq!(status(&snapshot(&f.service).await), RunStatus::AtRisk);
        let edit = FixedEdit {
            participants: Some(owned(&line_up)),
            ..FixedEdit::default()
        };
        by_tests(&mut f.service)
            .update_fixed(&f.fixed, edit, &Guild, &policy())
            .await
            .unwrap();
        let after = snapshot(&f.service).await;
        assert_eq!(status(&after), expected, "line-up {line_up:?}");
        assert!(
            !after
                .rsvps
                .iter()
                .any(|row| &row.run_id == run && row.user_id == "1002"),
            "the decliner's answer is dropped"
        );
        let cancelled = after.runs.iter().find(|r| r.id == f.runs[2]).unwrap();
        assert_eq!(
            cancelled.status,
            RunStatus::Cancelled,
            "sticky status untouched"
        );
    }
}

#[tokio::test]
async fn reset_refuses_a_weekly_slot_at_or_before_now_and_writes_nothing() {
    // After the weekly Mon 7 Sep 21:30 slot, before the amended Wed 9 Sep 21:00;
    // then exactly on the slot.
    for now in [kl(8, 12, 0), kl(7, 21, 30)] {
        let mut f = fixture().await;
        f.clock.set(now);
        let before = snapshot(&f.service).await;
        let error = reset(&mut f.service, &f.runs[1]).await.unwrap_err();
        assert_eq!(
            error,
            SchedulerError::Schedule(ScheduleError::ResetSlotPassed {
                run_id: f.runs[1].clone(),
                slot_day: "Mon 07 Sep".into(),
                slot_time: "21:30".into(),
            }),
            "{now}"
        );
        assert_eq!(snapshot(&f.service).await, before, "{now}: nothing written");
    }
    // One second before the slot it still resets.
    let mut f = fixture().await;
    f.clock.set(kl(7, 21, 30) - TimeDelta::seconds(1));
    let outcome = reset(&mut f.service, &f.runs[1]).await.unwrap();
    assert_eq!(outcome.value.run.datetime, utc(kl(7, 21, 30)));
    assert_eq!(outcome.notices.len(), 1);
}

#[tokio::test]
async fn restoring_after_the_morning_card_posted_recreates_it_as_sent() {
    for from in [RunStatus::Cancelled, RunStatus::Otot, RunStatus::Done] {
        let mut f = fixture().await;
        let run = &f.runs[0];
        // Mon 31 Aug 10:00: the 09:00 card has gone out.
        f.clock.set(kl(31, 10, 0));
        send_day_of(&mut f.service, run, "777").await;
        for status in [from, RunStatus::Planned] {
            set_status(&mut f.service, run, status, true).await;
        }
        let now = utc(kl(31, 10, 0));
        let state = snapshot(&f.service).await;
        assert_sound(&state, now, Some(&[run]));
        assert_eq!(
            pings(&state, run),
            [
                ("day_of".to_owned(), utc(kl(31, 9, 0)), Some(now)),
                ("countdown_60".to_owned(), utc(kl(31, 20, 30)), None),
                ("countdown_15".to_owned(), utc(kl(31, 21, 15)), None),
            ],
            "{from:?} -> planned re-arms nothing already past"
        );
    }
}

#[tokio::test]
async fn keeping_an_amended_run_still_takes_the_roster_and_channel_edit() {
    let mut f = fixture().await;
    let run = &f.runs[1];
    react(&mut f.service, run, "1001", EMOJI_YES).await;
    react(&mut f.service, run, "1002", EMOJI_NO).await;
    let before = snapshot(&f.service).await;
    let edit = FixedEdit {
        participants: Some(owned(&["1001", "1003"])),
        channel_id: Some("333".into()),
        ..time_edit(22, 0)
    };
    let keep = per_run(&[(run, AmendedRunChoice::KeepForThisWeek)]);
    apply_edit(&mut f.service, &f.fixed, edit, keep)
        .await
        .unwrap();
    let after = snapshot(&f.service).await;
    let kept = after.runs.iter().find(|r| &r.id == run).unwrap();
    assert_eq!(kept.datetime, utc(kl(9, 21, 0)), "slot kept");
    assert_eq!(kept.participants, ["1001", "1003"], "roster pushed");
    assert_eq!(kept.channel_id.as_deref(), Some("333"), "channel pushed");
    assert_eq!(
        kept.status,
        RunStatus::Planned,
        "lone no dropped, recomputed"
    );
    assert!(
        !after
            .rsvps
            .iter()
            .any(|r| &r.run_id == run && r.user_id == "1002")
    );
    assert_eq!(
        reminder_rows(&after, run),
        reminder_rows(&before, run),
        "kept run's pings untouched"
    );
    let followed = after.runs.iter().find(|r| r.id == f.runs[0]).unwrap();
    assert_eq!(followed.datetime, utc(kl(31, 22, 0)));
    assert_eq!(followed.channel_id.as_deref(), Some("333"));
    assert_sound(
        &after,
        utc(kl(27, 1, 0)),
        Some(&f.runs.iter().collect::<Vec<_>>()),
    );
}
