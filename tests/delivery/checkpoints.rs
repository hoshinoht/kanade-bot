//! The tick's automatic history checkpoint: one per boss week, taken when the
//! week is materialised, idempotent across ticks and restarts.

use chrono::TimeDelta;
use kanade::bot::delivery::AdminAlert;
use kanade::infrastructure::store::SqliteStore;
use kanade_domain::history::{Actor, ChangeHistory, CheckpointKind, Checkpoints};

use crate::scenarios::{delivery, now, week, world};
use crate::support::Store;

async fn each_week_gets_one_checkpoint<S: Store + ChangeHistory>(store: &S) {
    let world = world();
    let mut first = delivery(store, &world, &world.fake);
    first.tick_at(now()).await.expect("tick");
    let head = store.history_head().await.expect("head");
    first
        .tick_at(now() + TimeDelta::hours(1))
        .await
        .expect("same week");
    drop(first);
    // A restarted process materialises again on its first tick.
    let mut restarted = delivery(store, &world, &world.fake);
    restarted
        .tick_at(now() + TimeDelta::hours(2))
        .await
        .expect("restart");

    let checkpoints = store.list_checkpoints(Some(week())).await.expect("list");
    assert_eq!(checkpoints.len(), 1, "each_week_gets_one_checkpoint");
    let checkpoint = &checkpoints[0];
    assert_eq!(checkpoint.kind, CheckpointKind::Auto);
    // The Wednesday 16:00Z reset is Thursday in Kuala Lumpur.
    assert_eq!(checkpoint.name, "week 2026-09-09 start");
    assert_eq!(checkpoint.head, head, "the head right after materialising");
    assert_eq!(checkpoint.created_by, Actor::system("delivery"));

    restarted
        .tick_at(now() + TimeDelta::days(7))
        .await
        .expect("next week");
    let all = store.list_checkpoints(None).await.expect("list");
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].week, week() + TimeDelta::days(7));
    assert_eq!(all[1].name, "week 2026-09-16 start");
}

#[tokio::test]
async fn each_boss_week_gets_one_automatic_checkpoint() {
    each_week_gets_one_checkpoint(&SqliteStore::open_temp().await).await;
}

#[tokio::test]
async fn a_failed_checkpoint_never_aborts_the_tick_and_is_retried() {
    let store = SqliteStore::open_temp().await;
    let world = world();
    // Break checkpoint writes behind the store's back.
    let dir = store.temp_dir();
    dir.raw("DROP TABLE checkpoints;").await.expect("drop");
    let mut delivery = delivery(&store, &world, &world.fake);
    delivery.tick_at(now()).await.expect("the tick still runs");
    assert!(
        world
            .alerts
            .alerts()
            .iter()
            .any(|alert| matches!(alert, AdminAlert::CheckpointFailed { week_start, .. } if *week_start == week())),
        "{:?}",
        world.alerts.alerts()
    );

    dir.raw(
        "CREATE TABLE checkpoints (
            id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL CHECK (kind IN ('auto', 'admin')),
            seq INTEGER NOT NULL REFERENCES change_log (seq), hash TEXT NOT NULL,
            revision INTEGER NOT NULL, week_start TEXT NOT NULL, created_at TEXT NOT NULL,
            created_by_kind TEXT NOT NULL, created_by_id TEXT NOT NULL);
         CREATE UNIQUE INDEX checkpoints_auto_week ON checkpoints (week_start) WHERE kind = 'auto';",
    )
    .await
    .expect("recreate");
    delivery
        .tick_at(now() + TimeDelta::minutes(1))
        .await
        .expect("next tick");
    let checkpoints = store.list_checkpoints(Some(week())).await.expect("list");
    assert_eq!(checkpoints.len(), 1, "the next tick retried the checkpoint");
    assert_eq!(checkpoints[0].kind, CheckpointKind::Auto);
    drop(delivery);
    store.close().await.expect("close");
}
