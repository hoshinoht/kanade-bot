//! The `CharacterData` task under paused time: the sweeper runs on the
//! monotonic period whatever the wall clock does, deletes rows stamped in
//! the future after a step back, never suppresses expiry, wakes on unlink
//! and is joined by `stop`; the estimate refresh passes 10 minutes after
//! start, then hourly.

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
use crate::infrastructure::store::{SqliteCharData, SqliteStore};

fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
}

struct Rig {
    service: Arc<CharacterData>,
    file: Arc<SqliteCharData>,
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
    let file = Arc::new(SqliteCharData::open_temp().await);
    let now = Arc::new(Mutex::new(at(11, 0)));
    let clock: Clock = {
        let now = Arc::clone(&now);
        Arc::new(move || *now.lock().unwrap())
    };
    let roster = Arc::new(SqliteStore::open_temp().await);
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
    let task = Task::start(Arc::clone(&rig.service), SWEEP_EVERY, None);
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
    let task = Task::start(
        Arc::clone(&rig.service),
        Duration::from_secs(1_000 * 3600),
        None,
    );
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
    let task = Task::start(Arc::clone(&rig.service), SWEEP_EVERY, None);
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
    let task = Task::start(
        Arc::clone(&rig.service),
        Duration::from_secs(1_000 * 3600),
        None,
    );
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
    let task = Task::start(Arc::clone(&rig.service), SWEEP_EVERY, None);
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
    let task = Task::start(Arc::clone(&rig.service), SWEEP_EVERY, None);
    let estimates = rig.service.estimates().unwrap();
    let Start::Started(job) = estimates.start(JobRequest {
        target: Target::Characters(vec!["c1".into()]),
        subjects: vec![Subject {
            character_id: "c1".into(),
            member_id: "1001".into(),
            ign: RosterIgn::of(&wisteria()).unwrap(),
        }],
        missing: Vec::new(),
        parties: Vec::new(),
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

/// The refresh's schedule, as a pass reads it: these runs.
struct Parties(Vec<crate::infrastructure::scouter::UpcomingRun>);

impl UpcomingParties for Parties {
    fn upcoming(&self, _: DateTime<Utc>) -> crate::infrastructure::scouter::UpcomingFuture<'_> {
        let runs = self.0.clone();
        Box::pin(async move {
            Some(crate::infrastructure::scouter::Upcoming {
                current_week: at(8, 0),
                runs,
            })
        })
    }
}

fn passes() -> usize {
    crate::runtime::logging::captured()
        .iter()
        .filter(|line| line["event"] == "estimate_refresh_pass")
        .count()
}

/// The first pass waits 10 minutes (a crash loop never hammers the
/// sidecar), then one runs every hour.
#[tokio::test(start_paused = true)]
async fn the_refresh_passes_ten_minutes_after_start_then_hourly() {
    use std::time::Duration;

    crate::runtime::logging::capture();
    // Nothing listens: each pass is skipped, and still logged.
    let rig = rig_with(Some("127.0.0.1:9".parse().unwrap())).await;
    let parties: Arc<dyn UpcomingParties> = Arc::new(Parties(Vec::new()));
    let task = Task::start(
        Arc::clone(&rig.service),
        SWEEP_EVERY,
        Some((parties, REFRESH)),
    );
    tokio::time::sleep(Duration::from_secs(10 * 60 - 1)).await;
    settle(async || rig.service.health().await.last_sweep.is_some()).await;
    assert_eq!(passes(), 0, "not before 10 minutes");
    tokio::time::sleep(Duration::from_secs(2)).await;
    settle(async || passes() == 1).await;
    tokio::time::sleep(Duration::from_secs(60 * 60 - 10)).await;
    assert_eq!(passes(), 1, "not before the hour");
    tokio::time::sleep(Duration::from_secs(20)).await;
    settle(async || passes() == 2).await;
    tokio::time::sleep(Duration::from_secs(60 * 60)).await;
    settle(async || passes() == 3).await;
    task.stop().await;
}

/// `stop` joins the task and the running background lookup at once, and
/// drops the background lookups still waiting.
#[tokio::test]
async fn stop_joins_with_background_lookups_waiting() {
    use std::time::Duration;

    use crate::domain::schedule::RunStatus;
    use crate::infrastructure::scouter::{RosterIgn, Subject, UpcomingRun};

    // Up, but never answers a lookup.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .route(
            "/healthz",
            axum::routing::get(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/v1/bosses",
            axum::routing::get(std::future::pending::<String>),
        );
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let rig = rig_with(Some(address)).await;
    let lilac = Character {
        id: "c2".into(),
        ign: "Lilac".into(),
        ..wisteria()
    };
    let run = |id: &str, hours: i64, character: &Character| UpcomingRun {
        run_id: id.into(),
        bosses: vec!["HLimbo".into()],
        status: RunStatus::Planned,
        datetime: at(11, 0) + TimeDelta::hours(hours),
        week_start: at(8, 0),
        subjects: vec![Subject {
            character_id: character.id.clone(),
            member_id: character.member_id.clone(),
            ign: RosterIgn::of(character).unwrap(),
        }],
        missing: Vec::new(),
        parties: Vec::new(),
    };
    let parties: Arc<dyn UpcomingParties> = Arc::new(Parties(vec![
        run("r1", 2, &wisteria()),
        run("r2", 3, &lilac),
    ]));
    let refresh = Refresh {
        first: Duration::from_millis(10),
        every: Duration::from_secs(60 * 60),
    };
    let task = Task::start(
        Arc::clone(&rig.service),
        SWEEP_EVERY,
        Some((parties, refresh)),
    );
    let estimates = rig.service.estimates().unwrap();
    let waiting = [RosterIgn::of(&lilac).unwrap()];
    for _ in 0..500 {
        if estimates.running().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(estimates.running().is_some(), "r1 is being looked up");
    assert!(estimates.looking_up(&waiting), "r2 waits");
    tokio::time::timeout(Duration::from_secs(5), task.stop())
        .await
        .expect("stop returns without waiting for the lookup");
    assert!(
        !estimates.looking_up(&waiting),
        "the waiting job is dropped"
    );
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
