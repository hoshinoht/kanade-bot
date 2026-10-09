//! Properties beyond the vectors: idempotence, restart safety, stale bounds and
//! the v5 day-of clamp.

use chrono::{DateTime, FixedOffset, NaiveTime, TimeDelta, TimeZone, Utc, Weekday};
use chrono_tz::{America::New_York, Asia::Kuala_Lumpur, Tz};
use kanade::domain::{
    ids::IdGenerator,
    schedule::{
        COUNTDOWN_GRACE, DAY_OF, DAY_OF_GRACE, NewFixedRun, NewRun, ReminderPolicy, RunSource,
        RunStatus, ScheduleSnapshot, is_stale, reminder_specs,
    },
    scheduler::{ScheduleStore, SchedulerService, Scope},
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

type Service = SchedulerService<MemoryScheduleStore, CountingIds, TestClock>;

fn kl(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(y, mo, d, h, mi, 0)
        .unwrap()
}

fn hm(hour: u32, minute: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
}

fn policy(zone: Tz, ping: NaiveTime) -> ReminderPolicy {
    ReminderPolicy {
        zone,
        ping_time: ping,
        countdowns: vec![60, 15],
    }
}

fn service(clock: &TestClock) -> Service {
    SchedulerService::new(
        MemoryScheduleStore::new(),
        CountingIds::default(),
        clock.clone(),
    )
}

fn weekly(weekday: Weekday, time: NaiveTime, bosses: &[&str]) -> NewFixedRun {
    NewFixedRun {
        owner_pinned: false,
        owner_id: "42".into(),
        channel_id: Some("900".into()),
        bosses: bosses.iter().map(|b| (*b).into()).collect(),
        weekday,
        time,
        participants: vec!["1".into(), "2".into()],
        note: None,
    }
}

async fn state(service: &Service) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.unwrap()
}

fn due(state: &ScheduleSnapshot, now: DateTime<Utc>) -> Vec<String> {
    state
        .reminders
        .iter()
        .filter(|row| row.sent_at.is_none() && row.fire_at <= now)
        .map(|row| row.id.clone())
        .collect()
}

#[tokio::test]
async fn materialising_twice_yields_an_identical_snapshot() {
    let clock = TestClock::new(kl(2026, 8, 27, 1, 0));
    let mut service = service(&clock);
    let policy = policy(Kuala_Lumpur, hm(9, 0));
    service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(weekly(Weekday::Mon, hm(21, 30), &["HFA"]))
        .await
        .unwrap();
    service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(weekly(Weekday::Wed, hm(0, 30), &["HMaleficStar"]))
        .await
        .unwrap();
    service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(weekly(Weekday::Tue, hm(20, 0), &["HLucid"]))
        .await
        .unwrap();
    // A matching one-off the Tuesday timing adopts rather than duplicates.
    let one_off = service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some("900".into()),
            week_start: kl(2026, 8, 27, 0, 0).with_timezone(&Utc),
            datetime: kl(2026, 9, 1, 22, 0).with_timezone(&Utc),
            bosses: vec!["HLucid".into()],
            participants: vec!["1".into()],
            status: RunStatus::Confirmed,
            source: RunSource::Amend,
        })
        .await
        .unwrap();
    for week in [kl(2026, 8, 27, 0, 0), kl(2026, 9, 3, 0, 0)] {
        let first = service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .materialise_week(week, &policy)
            .await
            .unwrap();
        let once = state(&service).await;
        let again = service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .materialise_week(week, &policy)
            .await
            .unwrap();
        assert!(!first.is_empty() && again.is_empty(), "{week}");
        assert_eq!(
            state(&service).await,
            once,
            "{week}: second pass changed state"
        );
    }
    let adopted = state(&service).await;
    let run = adopted.runs.iter().find(|run| run.id == one_off).unwrap();
    assert!(run.fixed_run_id.is_some());
    assert_eq!(
        adopted.runs.len(),
        6,
        "three timings x two weeks, one adopted"
    );
}

#[tokio::test]
async fn a_tick_free_restart_creates_no_due_reminders_for_past_slots() {
    let week = kl(2026, 8, 27, 0, 0);
    let policy = policy(Kuala_Lumpur, hm(9, 0));
    let late = kl(2026, 8, 31, 21, 20);

    // Cold start after every ping of Monday 21:30 passed: rows are born sent,
    // and a slot more than two hours past is not created at all.
    let clock = TestClock::new(late);
    let mut cold = service(&clock);
    cold.as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(weekly(Weekday::Mon, hm(21, 30), &["HFA"]))
        .await
        .unwrap();
    cold.as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(weekly(Weekday::Mon, hm(18, 0), &["HLucid"]))
        .await
        .unwrap();
    assert_eq!(
        cold.as_origin(kanade::domain::history::Origin::for_tests())
            .materialise_week(week, &policy)
            .await
            .unwrap()
            .len(),
        1
    );
    let cold_state = state(&cold).await;
    assert_eq!(cold_state.reminders.len(), 3);
    assert!(due(&cold_state, late.with_timezone(&Utc)).is_empty());
    assert!(
        cold_state
            .reminders
            .iter()
            .all(|row| row.sent_at == Some(late.with_timezone(&Utc)))
    );

    // Materialised in the morning, then down without a tick until 21:20: the
    // restart adds nothing, and the backlog it finds is judged by staleness.
    let clock = TestClock::new(kl(2026, 8, 31, 8, 0));
    let mut warm = service(&clock);
    warm.as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(weekly(Weekday::Mon, hm(21, 30), &["HFA"]))
        .await
        .unwrap();
    warm.as_origin(kanade::domain::history::Origin::for_tests())
        .materialise_week(week, &policy)
        .await
        .unwrap();
    let before = state(&warm).await;
    clock.set(late);
    assert!(
        warm.as_origin(kanade::domain::history::Origin::for_tests())
            .materialise_week(week, &policy)
            .await
            .unwrap()
            .is_empty()
    );
    let after = state(&warm).await;
    assert_eq!(after, before, "restart rewrote reminder rows");
    let now = late.with_timezone(&Utc);
    let fresh: Vec<&str> = after
        .reminders
        .iter()
        .filter(|row| row.sent_at.is_none() && row.fire_at <= now)
        .filter(|row| !is_stale(&row.kind, row.fire_at, now))
        .map(|row| row.kind.as_str())
        .collect();
    assert_eq!(
        fresh,
        ["countdown_15"],
        "only the 21:15 countdown is still worth posting"
    );
}

#[tokio::test]
async fn cancelled_and_done_runs_keep_no_pending_reminders() {
    let clock = TestClock::new(kl(2026, 8, 27, 1, 0));
    let mut service = service(&clock);
    let policy = policy(Kuala_Lumpur, hm(9, 0));
    let weeks = [kl(2026, 8, 27, 0, 0), kl(2026, 9, 3, 0, 0)];
    let retired = service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(weekly(Weekday::Mon, hm(21, 30), &["HFA"]))
        .await
        .unwrap();
    service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(weekly(Weekday::Tue, hm(21, 30), &["HLucid"]))
        .await
        .unwrap();
    for week in weeks {
        service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .materialise_week(week, &policy)
            .await
            .unwrap();
    }
    assert_eq!(
        service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .retire_fixed_run(&retired, &weeks, &policy)
            .await
            .unwrap(),
        2
    );
    clock.set(kl(2026, 9, 2, 0, 0));
    assert_eq!(
        service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .mark_done()
            .await
            .unwrap()
            .len(),
        1
    );
    let end = state(&service).await;
    for run in end.runs.iter().filter(|run| run.status.is_terminal()) {
        let pending = end
            .reminders
            .iter()
            .filter(|row| row.run_id == run.id && row.sent_at.is_none())
            .count();
        assert_eq!(
            pending, 0,
            "{:?} run {} keeps pending pings",
            run.status, run.id
        );
    }
}

#[test]
fn stale_grace_is_strictly_greater_than_the_bound() {
    let fire = Utc.with_ymd_and_hms(2026, 8, 31, 4, 0, 0).unwrap();
    let second = TimeDelta::seconds(1);
    for (kind, grace) in [
        (DAY_OF, DAY_OF_GRACE),
        ("countdown_60", COUNTDOWN_GRACE),
        ("countdown_0", COUNTDOWN_GRACE),
        ("custom", COUNTDOWN_GRACE),
    ] {
        assert!(!is_stale(kind, fire, fire + grace), "{kind} at the bound");
        assert!(
            is_stale(kind, fire, fire + grace + second),
            "{kind} one second past"
        );
    }
    assert_eq!(DAY_OF_GRACE, TimeDelta::hours(12));
    assert_eq!(COUNTDOWN_GRACE, TimeDelta::minutes(30));
}

#[test]
fn day_of_never_fires_at_or_after_the_start_across_dst_transitions() {
    let pings: Vec<NaiveTime> = (0..96).map(|q| hm(q / 4, q % 4 * 15)).collect();
    for (from, days) in [
        (Utc.with_ymd_and_hms(2026, 3, 7, 0, 0, 0).unwrap(), 3),
        (Utc.with_ymd_and_hms(2026, 10, 31, 0, 0, 0).unwrap(), 3),
    ] {
        for step in 0..days * 96 {
            let start = (from + TimeDelta::minutes(15 * step)).fixed_offset();
            for ping in &pings {
                let specs =
                    reminder_specs(start, RunStatus::Planned, &policy(New_York, *ping)).unwrap();
                let day_of = specs[0].fire_at;
                assert!(day_of < start, "ping {ping} run {start}: day_of {day_of}");
                assert!(
                    start - day_of <= TimeDelta::hours(49),
                    "ping {ping} run {start}"
                );
            }
        }
    }
}

/// A boss-week reset inside the spring-forward gap (Sun 02:30 New York on
/// 2026-03-08) keeps its wall clock, so a 03:00 slot belongs to that week.
/// Expected values are v4's (the v4 tree, git history up to `487c4ed`):
/// `materialise.materialise_week` over `weeks.materialised_week_starts(tz, 6,
/// time(2, 30), 2026-03-08T07:40Z)` with these five timings added in order,
/// ids `00000000-0000-4000-8003-{n:012}`, ping 09:00, countdowns [60].
#[tokio::test]
async fn a_reset_inside_a_dst_gap_materialises_like_v4() {
    use kanade::domain::schedule::SchedulePolicy;
    use kanade::domain::weeks::week_end;
    use serde_json::json;

    let ids: Vec<String> = (1..200)
        .map(|n| format!("00000000-0000-4000-8003-{n:012}"))
        .collect();
    let now = Utc.with_ymd_and_hms(2026, 3, 8, 7, 40, 0).unwrap();
    let (mut service, _clock) = crate::common::service(&json!({
        "clock": "2026-03-08T07:40:00+00:00",
        "uuid_sequence": ids,
    }));
    let policy = SchedulePolicy::new(
        ReminderPolicy {
            zone: New_York,
            ping_time: hm(9, 0),
            countdowns: vec![60],
        },
        Weekday::Sun,
        hm(2, 30),
    );
    for (weekday, time) in [
        (Weekday::Sun, hm(2, 45)),
        (Weekday::Sun, hm(3, 0)),
        (Weekday::Sun, hm(4, 0)),
        (Weekday::Mon, hm(21, 30)),
        (Weekday::Sat, hm(23, 0)),
    ] {
        service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .add_fixed_run(NewFixedRun {
                owner_pinned: false,
                owner_id: "42".into(),
                channel_id: Some("900".into()),
                bosses: vec!["HFA".into()],
                weekday,
                time,
                participants: vec!["1".into()],
                note: None,
            })
            .await
            .unwrap();
    }
    let weeks = policy.materialised_weeks(now).unwrap();
    let starts: Vec<String> = weeks.iter().map(|week| week.isoformat()).collect();
    assert_eq!(
        starts,
        [
            "2026-03-08T02:30:00-05:00",
            "2026-03-15T02:30:00-04:00",
            "2026-03-22T02:30:00-04:00"
        ]
    );
    assert_eq!(
        week_end(&weeks[0], New_York).unwrap().isoformat(),
        "2026-03-15T02:30:00-04:00"
    );
    let suffix = |id: &str| id[id.len() - 3..].to_owned();
    let mut created = Vec::new();
    for week in weeks {
        let ids = service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .materialise_week(week, &policy.reminders)
            .await
            .unwrap();
        created.push(ids.iter().map(|id| suffix(id)).collect::<Vec<_>>());
    }
    assert_eq!(
        created,
        [
            ["006", "007", "008", "009", "010"],
            ["021", "022", "023", "024", "025"],
            ["036", "037", "038", "039", "040"]
        ]
    );
    let runs: Vec<String> = crate::common::snapshot(&service)
        .await
        .runs
        .iter()
        .map(|run| {
            format!(
                "{} {} {} {}",
                suffix(&run.id),
                suffix(run.fixed_run_id.as_deref().unwrap()),
                crate::common::iso(run.week_start),
                crate::common::iso(run.datetime)
            )
        })
        .collect();
    let week = |n: &str| format!("2026-03-{n}+00:00");
    let expected: Vec<String> = [
        ("009", "002", "08T07:30:00", "08T07:00:00"),
        ("008", "001", "08T07:30:00", "08T07:45:00"),
        ("010", "003", "08T07:30:00", "08T08:00:00"),
        ("006", "004", "08T07:30:00", "10T01:30:00"),
        ("007", "005", "08T07:30:00", "15T03:00:00"),
        ("023", "001", "15T06:30:00", "15T06:45:00"),
        ("024", "002", "15T06:30:00", "15T07:00:00"),
        ("025", "003", "15T06:30:00", "15T08:00:00"),
        ("021", "004", "15T06:30:00", "17T01:30:00"),
        ("022", "005", "15T06:30:00", "22T03:00:00"),
        ("038", "001", "22T06:30:00", "22T06:45:00"),
        ("039", "002", "22T06:30:00", "22T07:00:00"),
        ("040", "003", "22T06:30:00", "22T08:00:00"),
        ("036", "004", "22T06:30:00", "24T01:30:00"),
        ("037", "005", "22T06:30:00", "29T03:00:00"),
    ]
    .iter()
    .map(|(run, fixed, start, at)| format!("{run} {fixed} {} {}", week(start), week(at)))
    .collect();
    assert_eq!(runs, expected);
}
