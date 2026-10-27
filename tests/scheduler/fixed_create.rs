//! A new weekly timing and its materialised runs commit as one change: a
//! failed commit leaves neither, a success writes one change record, and a
//! key a plain create recorded still replays.

use chrono::{FixedOffset, TimeZone, Weekday};
use kanade_domain::history::{ChangeHistory, Origin, RowKey};
use kanade_domain::ids::RandomIds;
use kanade_domain::schedule::NewFixedRun;
use kanade_domain::scheduler::{ScheduleStore, SchedulerError, SchedulerService, Scope};
use kanade_store::SqliteStore;

use crate::common::TestClock;
use crate::support::{by_tests, hm, policy, weekly};

/// On Sun 27 Sep 01:00 in Kuala Lumpur.
async fn service() -> SchedulerService<SqliteStore, RandomIds, TestClock> {
    let now = FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 9, 27, 1, 0, 0)
        .unwrap();
    SchedulerService::new(
        SqliteStore::open_temp().await,
        RandomIds,
        TestClock::new(now),
    )
}

fn timing() -> NewFixedRun {
    weekly("222", Weekday::Mon, hm(21, 30), &["1001", "1002"])
}

#[tokio::test]
async fn a_failed_create_leaves_neither_the_timing_nor_its_runs() {
    let mut service = service().await;
    let head = service.store().history_head().await.unwrap();
    service.store().fail_commit_after(0);
    let refused = by_tests(&mut service)
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
    let mut service = service().await;
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
    let mut service = service().await;
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
