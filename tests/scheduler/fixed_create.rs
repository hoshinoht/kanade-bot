//! A new weekly timing and its materialised runs commit as one change: a
//! failed commit leaves neither, a success writes one change record, and a
//! key a plain create recorded still replays.

use chrono::{FixedOffset, NaiveTime, TimeZone, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::domain::history::{ChangeHistory, Origin, RowKey};
use kanade::domain::ids::RandomIds;
use kanade::domain::schedule::{NewFixedRun, ReminderPolicy, SchedulePolicy};
use kanade::domain::scheduler::{ScheduleStore, SchedulerError, SchedulerService, Scope};
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::common::TestClock;

fn service() -> SchedulerService<MemoryScheduleStore, RandomIds, TestClock> {
    let now = FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 9, 27, 1, 0, 0)
        .unwrap();
    SchedulerService::new(MemoryScheduleStore::new(), RandomIds, TestClock::new(now))
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

fn timing() -> NewFixedRun {
    NewFixedRun {
        owner_id: "1001".into(),
        channel_id: Some("222".into()),
        bosses: vec!["HFA".into()],
        weekday: Weekday::Mon,
        time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
        participants: vec!["1001".into(), "1002".into()],
        note: None,
    }
}

#[tokio::test]
async fn a_failed_create_leaves_neither_the_timing_nor_its_runs() {
    let mut service = service();
    let head = service.store().history_head().await.unwrap();
    service.store().fail_commit_after(0);
    let refused = service
        .as_origin(Origin::for_tests())
        .add_fixed_run_materialised(timing(), &policy())
        .await;
    assert!(refused.is_err(), "{refused:?}");
    let snapshot = service.store().load(&Scope::All).await.unwrap();
    assert!(snapshot.fixed_runs.is_empty(), "no timing");
    assert!(snapshot.runs.is_empty(), "no runs");
    assert!(snapshot.reminders.is_empty(), "no reminders");
    assert_eq!(service.store().history_head().await.unwrap(), head);
}

#[tokio::test]
async fn a_create_writes_the_timing_and_its_runs_in_one_change_record() {
    let mut service = service();
    let head = service.store().history_head().await.unwrap().seq;
    let fixed_id = service
        .as_origin(Origin::for_tests().with_request_id("create-1"))
        .add_fixed_run_materialised(timing(), &policy())
        .await
        .unwrap();
    let after = service.store().history_head().await.unwrap().seq;
    assert_eq!(after, head + 1, "one change record");
    let record = service
        .store()
        .load_change(after)
        .await
        .unwrap()
        .expect("record");
    let fixed_rows = record
        .rows
        .iter()
        .filter(|row| matches!(&row.key, RowKey::FixedRun(id) if id == &fixed_id))
        .count();
    let run_rows = record
        .rows
        .iter()
        .filter(|row| matches!(row.key, RowKey::Run(_)))
        .count();
    assert_eq!(fixed_rows, 1);
    let snapshot = service.store().load(&Scope::All).await.unwrap();
    let runs = snapshot
        .runs
        .iter()
        .filter(|run| run.fixed_run_id.as_deref() == Some(fixed_id.as_str()))
        .count();
    assert!(runs > 0, "the materialised weeks have runs");
    assert_eq!(run_rows, runs, "every run is in the same record");

    // A retry under the key replays, adding nothing.
    let replay = service
        .as_origin(Origin::for_tests().with_request_id("create-1"))
        .add_fixed_run_materialised(timing(), &policy())
        .await;
    assert!(
        matches!(replay, Err(SchedulerError::AlreadyApplied { .. })),
        "{replay:?}"
    );
    assert_eq!(service.store().history_head().await.unwrap().seq, after);
}

#[tokio::test]
async fn a_key_a_plain_create_recorded_still_replays() {
    let mut service = service();
    service
        .as_origin(Origin::for_tests().with_request_id("legacy"))
        .add_fixed_run(timing())
        .await
        .unwrap();
    let head = service.store().history_head().await.unwrap();
    let replay = service
        .as_origin(Origin::for_tests().with_request_id("legacy"))
        .add_fixed_run_materialised(timing(), &policy())
        .await;
    assert!(
        matches!(replay, Err(SchedulerError::AlreadyApplied { .. })),
        "{replay:?}"
    );
    let mut other = timing();
    other.note = Some("another".into());
    let mismatch = service
        .as_origin(Origin::for_tests().with_request_id("legacy"))
        .add_fixed_run_materialised(other, &policy())
        .await;
    assert!(
        matches!(mismatch, Err(SchedulerError::IdempotencyMismatch { .. })),
        "{mismatch:?}"
    );
    assert_eq!(service.store().history_head().await.unwrap(), head);
}
