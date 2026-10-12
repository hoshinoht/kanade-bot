//! The `CharacterData` task under paused time: the sweeper runs on the
//! monotonic period whatever the wall clock does, deletes rows stamped in
//! the future after a step back, never suppresses expiry, wakes on unlink
//! and is joined by `stop`.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use super::*;
use crate::domain::characters::CharacterStore;
use crate::domain::characters::{Character, CharacterChange, CharacterRecord, CharacterRole};
use crate::domain::chardata::UnlinkReason;
use crate::domain::history::{Actor, Surface};
use crate::domain::members::MemberStore;
use crate::infrastructure::store::chardata_conformance::{active, snapshot};
use crate::infrastructure::store::members_conformance::gateway;
use crate::infrastructure::store::{MemoryCharData, MemoryScheduleStore};

fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
}

struct Rig {
    service: Arc<CharacterData>,
    file: Arc<MemoryCharData>,
    now: Arc<Mutex<DateTime<Utc>>>,
}

/// Member 1001 with roster character `c1` (Wisteria), as the link fixture
/// names them, so the orphan pass keeps the link.
async fn rig() -> Rig {
    let file = Arc::new(MemoryCharData::new());
    let now = Arc::new(Mutex::new(at(11, 0)));
    let clock: Clock = {
        let now = Arc::clone(&now);
        Arc::new(move || *now.lock().unwrap())
    };
    let roster = Arc::new(MemoryScheduleStore::new());
    roster
        .apply_gateway(gateway("1001", "Alice", &["10"]))
        .await
        .unwrap();
    roster
        .apply_character(
            CharacterChange::Create(Character {
                id: "c1".into(),
                member_id: "1001".into(),
                ign: "Wisteria".into(),
                world: "msea".into(),
                class: None,
                level: None,
                role: CharacterRole::Main,
                retired_at: None,
                created_at: at(1, 0),
                updated_at: at(1, 0),
                updated_by: Actor::admin("token"),
                version: 1,
            }),
            CharacterRecord {
                at: at(1, 0),
                actor: Actor::admin("token"),
                surface: Surface::AdminPortal,
            },
        )
        .await
        .unwrap();
    let service = Arc::new(CharacterData::new(
        Arc::clone(&file) as Arc<dyn CharDataStore>,
        roster as Arc<dyn RosterSide>,
        None,
        clock,
        Timing::default(),
    ));
    Rig { service, file, now }
}

/// Let the task run until `done` holds.
async fn settle(mut done: impl AsyncFnMut() -> bool) {
    for _ in 0..10_000 {
        if done().await {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("the task did not get there");
}

#[tokio::test(start_paused = true)]
async fn the_sweeper_keeps_its_period_through_a_wall_clock_step_back() {
    let rig = rig().await;
    let file = Arc::clone(&rig.file);
    let service = Arc::clone(&rig.service);
    let task = Task::start(Arc::clone(&rig.service), SWEEP_EVERY);
    settle(async || service.health().await.last_sweep.is_some()).await;
    file.put_link(active("c1"), 0).await.unwrap();
    assert!(file.put_snapshot(snapshot("c1", at(11, 0))).await.unwrap());

    // The wall clock steps back two days: the row is now in the future.
    *rig.now.lock().unwrap() = at(9, 0);
    assert_eq!(
        file.snapshot("c1".into(), at(9, 0)).await.unwrap(),
        None,
        "reads refuse it before any sweep"
    );
    // The next sweep still comes an hour later on the monotonic clock and
    // deletes it (read as of its own time, it would be fresh).
    tokio::time::advance(SWEEP_EVERY).await;
    settle(async || {
        file.snapshot("c1".into(), at(11, 0))
            .await
            .unwrap()
            .is_none()
    })
    .await;
    assert!(
        file.link("c1".into()).await.unwrap().is_some(),
        "the link stays"
    );

    // Expiry is never suppressed: data past its 30 days goes on the next
    // hourly sweep.
    assert!(file.put_snapshot(snapshot("c1", at(9, 0))).await.unwrap());
    *rig.now.lock().unwrap() = at(9, 0) + TimeDelta::days(30);
    tokio::time::advance(SWEEP_EVERY).await;
    settle(async || {
        file.snapshot("c1".into(), at(9, 0) + TimeDelta::days(29))
            .await
            .unwrap()
            .is_none()
    })
    .await;
    task.stop().await;
}

/// The batch waits on the monotonic clock but runs only once the wall clock
/// reaches 04:00 SGT: stepped back two days during the wait, it does not
/// run early, and runs at the recomputed 04:00 instead.
#[tokio::test(start_paused = true)]
async fn the_batch_never_runs_before_four_after_a_step_back() {
    let rig = rig().await;
    let service = Arc::clone(&rig.service);
    // A long sweep period, so only the batch wait can wake the loop.
    let task = Task::start(Arc::clone(&rig.service), Duration::from_secs(1_000 * 3600));
    settle(async || service.health().await.last_sweep.is_some()).await;
    // 11 Oct 00:00 UTC is 08:00 SGT: the batch is 20 hours away.
    *rig.now.lock().unwrap() = at(9, 0);
    tokio::time::advance(Duration::from_secs(20 * 3600)).await;
    for _ in 0..1_000 {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        service.health().await.last_batch,
        None,
        "not before 04:00 SGT"
    );
    // The loop waits again from the new now: 20 hours to 9 Oct 20:00 UTC.
    *rig.now.lock().unwrap() = at(9, 20);
    tokio::time::advance(Duration::from_secs(20 * 3600)).await;
    settle(async || service.health().await.last_batch.is_some()).await;
    assert_eq!(service.health().await.last_batch.unwrap().at, at(9, 20));
    task.stop().await;
}

/// The wall clock steps forward past 04:00 SGT while the sweep timer
/// wakes the loop first: the owed batch runs once, not skipped to tomorrow.
#[tokio::test(start_paused = true)]
async fn a_forward_step_past_four_runs_the_owed_batch_once() {
    let rig = rig().await;
    let service = Arc::clone(&rig.service);
    let task = Task::start(Arc::clone(&rig.service), SWEEP_EVERY);
    settle(async || service.health().await.last_sweep.is_some()).await;
    // 11 Oct 00:00 UTC; the batch is owed at 20:00 UTC (04:00 SGT).
    *rig.now.lock().unwrap() = at(11, 21);
    tokio::time::advance(SWEEP_EVERY).await;
    settle(async || service.health().await.last_batch.is_some()).await;
    assert_eq!(service.health().await.last_batch.unwrap().at, at(11, 21));
    // More sweeps that day run no second batch.
    *rig.now.lock().unwrap() = at(11, 22);
    let swept = service.health().await.last_sweep;
    tokio::time::advance(SWEEP_EVERY).await;
    settle(async || service.health().await.last_sweep != swept).await;
    for _ in 0..1_000 {
        tokio::task::yield_now().await;
    }
    assert_eq!(service.health().await.last_batch.unwrap().at, at(11, 21));
    task.stop().await;
}

/// Repeated step-backs never run the batch early: each wake before it
/// waits again for a positive time (no spin: the test returns).
#[tokio::test(start_paused = true)]
async fn repeated_step_backs_never_run_early() {
    let rig = rig().await;
    let service = Arc::clone(&rig.service);
    let task = Task::start(Arc::clone(&rig.service), Duration::from_secs(1_000 * 3600));
    settle(async || service.health().await.last_sweep.is_some()).await;
    for day in (5..10).rev() {
        *rig.now.lock().unwrap() = at(day, 0);
        tokio::time::advance(Duration::from_secs(3600)).await;
        for _ in 0..1_000 {
            tokio::task::yield_now().await;
        }
    }
    assert_eq!(service.health().await.last_batch, None);
    task.stop().await;
}

#[tokio::test(start_paused = true)]
async fn an_unlink_wakes_the_sweeper_and_stop_joins_the_task() {
    let rig = rig().await;
    let service = Arc::clone(&rig.service);
    let task = Task::start(Arc::clone(&rig.service), SWEEP_EVERY);
    settle(async || service.health().await.last_sweep.is_some()).await;
    let first = service.health().await.last_sweep;
    *rig.now.lock().unwrap() = at(11, 0) + TimeDelta::minutes(5);
    rig.file.put_link(active("c1"), 0).await.unwrap();
    service
        .unlink(
            "c1",
            &Actor::admin("token"),
            Surface::AdminPortal,
            UnlinkReason::Requested,
            None,
        )
        .await
        .unwrap();
    // Woken at once, well inside the hour.
    settle(async || service.health().await.last_sweep != first).await;
    task.stop().await;
}
