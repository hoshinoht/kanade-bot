//! v5-only mutation behaviour: choices for amended runs on a weekly-timing
//! edit, and resetting an amended run to its weekly timing.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::domain::{
    ids::IdGenerator,
    members::{Directory, Member},
    notify::NoticeOutbox,
    schedule::{
        AmendedRunChoice, EMOJI_NO, EMOJI_YES, FixedEdit, FixedEditChoices, FixedEditRequest,
        NewFixedRun, NewRun, NoticeChange, ReminderPolicy, RunSource, RunStatus, ScheduleError,
        SchedulePolicy, ScheduleSnapshot, StatusChange,
    },
    scheduler::{ScheduleStore, SchedulerError, SchedulerService, Scope},
};
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::common::{TestClock, assert_sound};

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
            display_name: Some(format!("member {user_id}")),
            has_role: true,
            ..Member::default()
        })
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        ["222", "333"].contains(&channel_id)
    }
}

type Service = SchedulerService<MemoryScheduleStore, CountingIds, TestClock>;

fn kl(d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
    let month = if d >= 27 { 8 } else { 9 };
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, month, d, h, mi, 0)
        .unwrap()
}

fn utc(at: DateTime<FixedOffset>) -> DateTime<Utc> {
    at.with_timezone(&Utc)
}

fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

/// Weekly Mon 21:30 materialised for weeks of 27 Aug, 3 Sep and 10 Sep; the
/// 3 Sep run is amended to Wed 9 Sep 21:00 and the 10 Sep run is cancelled.
struct Fixture {
    service: Service,
    clock: TestClock,
    fixed: String,
    /// Runs by week: 31 Aug, 7 Sep (amended), 14 Sep (cancelled).
    runs: [String; 3],
}

async fn fixture() -> Fixture {
    let clock = TestClock::new(kl(27, 1, 0));
    let mut service = SchedulerService::new(
        MemoryScheduleStore::new(),
        CountingIds::default(),
        clock.clone(),
    );
    let fixed = service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(NewFixedRun {
            owner_pinned: false,
            owner_id: "1001".into(),
            channel_id: Some("222".into()),
            bosses: vec!["HFA".into()],
            weekday: Weekday::Mon,
            time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
            participants: vec!["1001".into(), "1002".into()],
            note: None,
        })
        .await
        .unwrap();
    service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .materialise_weeks(&policy())
        .await
        .unwrap();
    let state = snapshot(&service).await;
    let run_at = |at: DateTime<FixedOffset>| {
        state
            .runs
            .iter()
            .find(|run| run.datetime == utc(at))
            .unwrap()
            .id
            .clone()
    };
    let runs = [
        run_at(kl(31, 21, 30)),
        run_at(kl(7, 21, 30)),
        run_at(kl(14, 21, 30)),
    ];
    service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .amend_run(&runs[1], utc(kl(9, 21, 0)), &policy())
        .await
        .unwrap();
    let cancel = StatusChange {
        status: RunStatus::Cancelled,
        announce: false,
        via_portal: true,
    };
    service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .set_status(&runs[2], cancel, &policy().reminders)
        .await
        .unwrap();
    Fixture {
        service,
        clock,
        fixed,
        runs,
    }
}

#[tokio::test]
async fn weekly_timing_add_and_remove_enqueue_notices_once_in_memory() {
    let clock = TestClock::new(kl(27, 1, 0));
    let mut service = SchedulerService::new(
        MemoryScheduleStore::new(),
        CountingIds::default(),
        clock.clone(),
    );
    let added = service
        .as_origin(kanade::domain::history::Origin::for_tests().with_request_id("fixed-add"))
        .add_fixed_run(NewFixedRun {
            owner_pinned: false,
            owner_id: "1001".into(),
            channel_id: Some("222".into()),
            bosses: vec!["HFA".into()],
            weekday: Weekday::Mon,
            time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
            participants: vec!["1001".into()],
            note: None,
        })
        .await
        .unwrap();
    let notices = service.store().outbox_notices().await.unwrap();
    assert!(matches!(
        &notices[..],
        [row] if matches!(&row.notice.change, NoticeChange::FixedAdded { fixed_id, .. } if fixed_id == &added)
    ));
    assert!(matches!(
        service
            .as_origin(kanade::domain::history::Origin::for_tests().with_request_id("fixed-add"),)
            .add_fixed_run(NewFixedRun {
                owner_pinned: false,
                owner_id: "1001".into(),
                channel_id: Some("222".into()),
                bosses: vec!["HFA".into()],
                weekday: Weekday::Mon,
                time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
                participants: vec!["1001".into()],
                note: None,
            })
            .await,
        Err(SchedulerError::AlreadyApplied { .. })
    ));
    assert_eq!(service.store().outbox_notices().await.unwrap().len(), 1);

    service
        .as_origin(kanade::domain::history::Origin::for_tests().with_request_id("fixed-remove"))
        .retire_fixed_run(&added, &[kl(27, 0, 0)], &policy().reminders)
        .await
        .unwrap();
    let notices = service.store().outbox_notices().await.unwrap();
    assert!(matches!(
        notices.last().map(|row| &row.notice.change),
        Some(NoticeChange::FixedRemoved { fixed_id, .. }) if fixed_id == &added
    ));
}

async fn snapshot(service: &Service) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.unwrap()
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

fn time_edit(hour: u32, minute: u32) -> FixedEdit {
    FixedEdit {
        time: NaiveTime::from_hms_opt(hour, minute, 0),
        ..FixedEdit::default()
    }
}

fn request(fixed: &str, edit: FixedEdit, choices: FixedEditChoices) -> FixedEditRequest {
    FixedEditRequest {
        fixed_id: fixed.into(),
        edit,
        choices,
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
async fn preview_lists_only_live_amended_runs_a_day_or_time_edit_would_move() {
    let f = fixture().await;
    let policy = policy();
    let affected = f
        .service
        .preview_fixed_edit(&f.fixed, &time_edit(22, 0), &policy)
        .await
        .unwrap();
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
    assert!(
        f.service
            .preview_fixed_edit(&f.fixed, &note, &policy)
            .await
            .unwrap()
            .is_empty()
    );
    // Editing to the slot the amended run already sits on moves nothing.
    let onto = FixedEdit {
        weekday: Some(Weekday::Wed),
        time: NaiveTime::from_hms_opt(21, 0, 0),
        ..FixedEdit::default()
    };
    assert!(
        f.service
            .preview_fixed_edit(&f.fixed, &onto, &policy)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn keeping_an_amended_run_leaves_its_slot_and_pings_while_others_follow() {
    let mut f = fixture().await;
    let policy = policy();
    // The amended run's morning ping already went out with a card.
    let amended_day_of = f
        .service
        .reminders(&f.runs[1])
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.kind == "day_of")
        .unwrap();
    f.service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .mark_reminder_sent(&amended_day_of.id, Some("555"))
        .await
        .unwrap();
    let before = snapshot(&f.service).await;
    let keep = per_run(&[(&f.runs[1], AmendedRunChoice::KeepForThisWeek)]);
    let outcome = f
        .service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .apply_fixed_edit(
            &request(&f.fixed, time_edit(22, 0), keep.clone()),
            &Guild,
            &policy,
        )
        .await
        .unwrap();
    assert_eq!(
        outcome.value.time,
        NaiveTime::from_hms_opt(22, 0, 0).unwrap()
    );
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
    let rows = |state: &ScheduleSnapshot| {
        state
            .reminders
            .iter()
            .filter(|row| row.run_id == f.runs[1])
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(rows(&after), rows(&before), "kept run's pings untouched");
    assert!(
        rows(&after)
            .iter()
            .any(|row| row.id == amended_day_of.id && row.message_id.as_deref() == Some("555"))
    );
    assert_sound(
        &after,
        utc(kl(27, 1, 0)),
        Some(&f.runs.iter().collect::<Vec<_>>()),
    );

    // Idempotent: the same edit and choice again changes no slot or ping.
    let again = f
        .service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .apply_fixed_edit(&request(&f.fixed, time_edit(22, 0), keep), &Guild, &policy)
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
    f.service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .materialise_weeks(&policy)
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
    let policy = policy();
    let mut chosen = fixture().await;
    let mut default = fixture().await;
    let update = per_run(&[(&chosen.runs[1], AmendedRunChoice::UpdateToFixed)]);
    chosen
        .service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .apply_fixed_edit(
            &request(&chosen.fixed, time_edit(22, 0), update),
            &Guild,
            &policy,
        )
        .await
        .unwrap();
    default
        .service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .update_fixed(&default.fixed, time_edit(22, 0), &Guild, &policy)
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
    let policy = policy();
    let before = snapshot(&f.service).await;
    let cases = [
        (
            per_run(&[
                (&f.runs[1], AmendedRunChoice::KeepForThisWeek),
                (&f.runs[0], AmendedRunChoice::KeepForThisWeek),
            ]),
            ScheduleError::UnexpectedChoice(f.runs[0].clone()),
        ),
        (
            per_run(&[(&f.runs[2], AmendedRunChoice::UpdateToFixed)]),
            ScheduleError::UnexpectedChoice(f.runs[2].clone()),
        ),
        (
            per_run(&[]),
            ScheduleError::MissingChoice(f.runs[1].clone()),
        ),
    ];
    for (choices, expected) in cases {
        let error = f
            .service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .apply_fixed_edit(
                &request(&f.fixed, time_edit(22, 0), choices),
                &Guild,
                &policy,
            )
            .await
            .unwrap_err();
        assert_eq!(error, SchedulerError::Schedule(expected));
    }
    // A note-only edit affects nothing, so any per-run choice is refused.
    let note = FixedEdit {
        note: Some("x".into()),
        ..FixedEdit::default()
    };
    let stray = per_run(&[(&f.runs[1], AmendedRunChoice::KeepForThisWeek)]);
    assert!(matches!(
        f.service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .apply_fixed_edit(&request(&f.fixed, note, stray), &Guild, &policy)
            .await,
        Err(SchedulerError::Schedule(ScheduleError::UnexpectedChoice(_)))
    ));
    assert_eq!(snapshot(&f.service).await, before, "refusals write nothing");
}

#[tokio::test]
async fn reset_restores_the_weekly_slot_party_and_channel() {
    let mut f = fixture().await;
    let policy = policy();
    let run = &f.runs[1];
    f.service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .swap_participants(run, &["1002".into()], &["1003".into()], true, &Guild)
        .await
        .unwrap();
    for user in ["1001", "1003"] {
        f.service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .apply_reaction(run, user, EMOJI_YES, true)
            .await
            .unwrap();
    }
    assert_eq!(
        snapshot(&f.service)
            .await
            .runs
            .iter()
            .find(|r| &r.id == run)
            .unwrap()
            .status,
        RunStatus::Confirmed
    );
    // Late enough that the restored slot's morning and T-1h pings have passed.
    f.clock.set(kl(7, 21, 0));
    let outcome = f
        .service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .reset_to_fixed(run, &policy)
        .await
        .unwrap();
    let state = &outcome.value;
    assert_eq!(state.run.datetime, utc(kl(7, 21, 30)));
    assert_eq!(state.run.participants, ["1001", "1002"]);
    assert_eq!(state.run.channel_id.as_deref(), Some("222"));
    assert_eq!(state.run.status, RunStatus::Planned, "amend status rule");
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
    let again = f
        .service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .reset_to_fixed(run, &policy)
        .await
        .unwrap();
    assert!(again.notices.is_empty());
    assert_eq!(snapshot(&f.service).await, after);
}

#[tokio::test]
async fn reset_refuses_one_offs_retired_timings_and_finished_runs() {
    let mut f = fixture().await;
    let policy = policy();
    let one_off = f
        .service
        .as_origin(kanade::domain::history::Origin::for_tests())
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
    let refusal = |result| match result {
        Err(SchedulerError::Schedule(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    };
    assert_eq!(
        refusal(
            f.service
                .as_origin(kanade::domain::history::Origin::for_tests())
                .reset_to_fixed(&one_off, &policy)
                .await
        ),
        ScheduleError::NotAFixedRun(one_off.clone())
    );
    assert_eq!(
        refusal(
            f.service
                .as_origin(kanade::domain::history::Origin::for_tests())
                .reset_to_fixed(&f.runs[2], &policy)
                .await
        ),
        ScheduleError::RunNotLive {
            run_id: f.runs[2].clone(),
            status: "cancelled".into()
        }
    );
    let done = StatusChange {
        status: RunStatus::Done,
        announce: false,
        via_portal: true,
    };
    f.service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .set_status(&f.runs[0], done, &policy.reminders)
        .await
        .unwrap();
    assert!(matches!(
        refusal(
            f.service
                .as_origin(kanade::domain::history::Origin::for_tests())
                .reset_to_fixed(&f.runs[0], &policy)
                .await
        ),
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
    f.service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .retire_fixed_run(&f.fixed, &first_week, &policy.reminders)
        .await
        .unwrap();
    assert_eq!(
        refusal(
            f.service
                .as_origin(kanade::domain::history::Origin::for_tests())
                .reset_to_fixed(&f.runs[1], &policy)
                .await
        ),
        ScheduleError::FixedRunRetired(f.fixed.clone())
    );
    assert_eq!(
        run_at(&snapshot(&f.service).await, &f.runs[1]),
        utc(kl(9, 21, 0))
    );
}

#[tokio::test]
async fn a_fixed_edit_removing_the_lone_decliner_recomputes_status_like_swap() {
    let policy = policy();
    // New line-up still missing an answer -> planned; only yes-sayers left -> confirmed.
    for (line_up, expected) in [
        (vec!["1001", "1003"], RunStatus::Planned),
        (vec!["1001"], RunStatus::Confirmed),
    ] {
        let mut f = fixture().await;
        let run = &f.runs[0];
        f.service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .apply_reaction(run, "1001", EMOJI_YES, true)
            .await
            .unwrap();
        f.service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .apply_reaction(run, "1002", EMOJI_NO, true)
            .await
            .unwrap();
        let status =
            |state: &ScheduleSnapshot| state.runs.iter().find(|r| &r.id == run).unwrap().status;
        assert_eq!(status(&snapshot(&f.service).await), RunStatus::AtRisk);
        let edit = FixedEdit {
            participants: Some(line_up.iter().map(|uid| (*uid).to_owned()).collect()),
            ..FixedEdit::default()
        };
        f.service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .update_fixed(&f.fixed, edit, &Guild, &policy)
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
    let policy = policy();
    // After the weekly Mon 7 Sep 21:30 slot, before the amended Wed 9 Sep 21:00;
    // then exactly on the slot.
    for now in [kl(8, 12, 0), kl(7, 21, 30)] {
        let mut f = fixture().await;
        f.clock.set(now);
        let before = snapshot(&f.service).await;
        let error = f
            .service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .reset_to_fixed(&f.runs[1], &policy)
            .await
            .unwrap_err();
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
    f.clock.set(kl(7, 21, 30) - chrono::TimeDelta::seconds(1));
    let outcome = f
        .service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .reset_to_fixed(&f.runs[1], &policy)
        .await
        .unwrap();
    assert_eq!(outcome.value.run.datetime, utc(kl(7, 21, 30)));
    assert_eq!(outcome.notices.len(), 1);
}

#[tokio::test]
async fn restoring_after_the_morning_card_posted_recreates_it_as_sent() {
    let policy = policy();
    for from in [RunStatus::Cancelled, RunStatus::Otot, RunStatus::Done] {
        let mut f = fixture().await;
        let run = &f.runs[0];
        // Mon 31 Aug 10:00: the 09:00 card has gone out.
        f.clock.set(kl(31, 10, 0));
        let day_of = f
            .service
            .reminders(run)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.kind == "day_of")
            .unwrap();
        f.service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .mark_reminder_sent(&day_of.id, Some("777"))
            .await
            .unwrap();
        for status in [from, RunStatus::Planned] {
            let change = StatusChange {
                status,
                announce: true,
                via_portal: true,
            };
            f.service
                .as_origin(kanade::domain::history::Origin::for_tests())
                .set_status(run, change, &policy.reminders)
                .await
                .unwrap();
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
    let policy = policy();
    let run = &f.runs[1];
    f.service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .apply_reaction(run, "1001", EMOJI_YES, true)
        .await
        .unwrap();
    f.service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .apply_reaction(run, "1002", EMOJI_NO, true)
        .await
        .unwrap();
    let before = snapshot(&f.service).await;
    let edit = FixedEdit {
        time: NaiveTime::from_hms_opt(22, 0, 0),
        participants: Some(vec!["1001".into(), "1003".into()]),
        channel_id: Some("333".into()),
        ..FixedEdit::default()
    };
    let keep = per_run(&[(run, AmendedRunChoice::KeepForThisWeek)]);
    f.service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .apply_fixed_edit(&request(&f.fixed, edit, keep), &Guild, &policy)
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
    let rows = |state: &ScheduleSnapshot| {
        state
            .reminders
            .iter()
            .filter(|row| &row.run_id == run)
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(rows(&after), rows(&before), "kept run's pings untouched");
    let followed = after.runs.iter().find(|r| r.id == f.runs[0]).unwrap();
    assert_eq!(followed.datetime, utc(kl(31, 22, 0)));
    assert_eq!(followed.channel_id.as_deref(), Some("333"));
    assert_sound(
        &after,
        utc(kl(27, 1, 0)),
        Some(&f.runs.iter().collect::<Vec<_>>()),
    );
}
