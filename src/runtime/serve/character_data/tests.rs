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

fn wisteria() -> Character {
    Character {
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
    }
}

/// Member 1001 with roster character `c1` (Wisteria), as the link fixture
/// names them, so the orphan pass keeps the link.
async fn rig() -> Rig {
    rig_with(None).await
}

/// The same, with the scouter at `scouter` when given.
async fn rig_with(scouter: Option<std::net::SocketAddr>) -> Rig {
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
            CharacterChange::Create(wisteria()),
            CharacterRecord {
                at: at(1, 0),
                actor: Actor::admin("token"),
                surface: Surface::AdminPortal,
            },
        )
        .await
        .unwrap();
    let mut service = CharacterData::new(
        Arc::clone(&file) as Arc<dyn CharDataStore>,
        roster as Arc<dyn RosterSide>,
        None,
        Arc::clone(&clock),
        Timing::default(),
    );
    if let Some(address) = scouter {
        let url = format!("http://{address}");
        let client = ScouterClient::new(&url).unwrap();
        service = service.with_estimates(Estimates::new(client, url, clock));
    }
    let service = Arc::new(service);
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

/// A lookup the sidecar never answers is cancelled and joined by `stop`,
/// so it has ended before serve closes the stores (not 190 s later).
#[tokio::test]
async fn stop_cancels_and_joins_a_running_estimate_lookup() {
    use std::time::Duration;

    use tokio::io::AsyncReadExt;

    use crate::infrastructure::scouter::{JobRequest, RosterIgn, Start, Subject, Target};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let rig = rig_with(Some(listener.local_addr().unwrap())).await;
    let task = Task::start(Arc::clone(&rig.service), SWEEP_EVERY);
    let estimates = rig.service.estimates().unwrap();
    let Start::Started(job) = estimates.start(JobRequest {
        target: Target::Characters(vec!["c1".into()]),
        subjects: vec![Subject {
            character_id: "c1".into(),
            member_id: "1001".into(),
            ign: RosterIgn::of(&wisteria()).unwrap(),
        }],
        missing: Vec::new(),
        started_by: Actor::admin("token"),
        refresh: false,
    }) else {
        panic!("nothing else runs");
    };
    // The lookup is in flight: its request arrived and is never answered.
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut buffer = [0_u8; 1024];
    assert!(socket.read(&mut buffer).await.unwrap() > 0);
    assert_eq!(estimates.running().map(|running| running.id), Some(job.id));
    tokio::time::timeout(Duration::from_secs(5), task.stop())
        .await
        .expect("stop returns without waiting for the lookup");
    // The lookup's connection is already gone when stop returns: the task
    // ended (and dropped it) before the store close that follows.
    let closed = tokio::time::timeout(Duration::from_millis(500), socket.read(&mut buffer))
        .await
        .expect("closed at once");
    assert!(matches!(closed, Ok(0) | Err(_)));
}

/// A fresh Compose volume root can come up 0755 (2026-10-12 crash-loop): a
/// file directly in it is refused, one in a subdirectory serve creates opens.
#[tokio::test]
async fn the_file_opens_under_a_group_readable_volume_root() {
    use std::fs::{self, DirBuilder, Permissions};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let tmp = fs::canonicalize(std::env::temp_dir()).unwrap();
    let root = tmp.join(format!("kanade-chardata-volume-{}", uuid::Uuid::new_v4()));
    let locks = tmp.join(format!("kanade-chardata-locks-{}", uuid::Uuid::new_v4()));
    DirBuilder::new().mode(0o700).create(&locks).unwrap();
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, Permissions::from_mode(0o755)).unwrap();
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

    let Err(Error::Startup(refused)) = open_file(&root.join("chardata.sqlite"), &locks).await
    else {
        panic!("a file in the 0755 root is refused");
    };
    assert!(refused.contains("unsafe path"), "{refused}");

    let db = root.join("db/chardata.sqlite");
    for _ in 0..2 {
        let file = open_file(&db, &locks).await.expect("opens, then reopens");
        close(file, Duration::ZERO).await;
    }
    assert_eq!(mode(&root.join("db")), 0o700);
    assert_eq!(mode(&db), 0o600);
    assert_eq!(mode(&root), 0o755, "the volume root is left alone");
    fs::remove_dir_all(&root).unwrap();
    fs::remove_dir_all(&locks).unwrap();
}
