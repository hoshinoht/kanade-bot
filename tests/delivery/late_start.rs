//! A process started after the morning ping time. Its first tick reconciles
//! day-of rows (materialisation); v5 deviation from v4: a due, unsent morning
//! card already at its ping time stays queued while its run is still ahead, so
//! dispatch posts it on that tick unless it is past the day-of grace
//! (`DAY_OF_GRACE`, 12 h). A card already posted is never posted again, and
//! the card of a run that has started is retired unsent.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use kanade_domain::history::Origin;
use kanade_domain::ids::RandomIds;
use kanade_domain::schedule::{DAY_OF, NewRun, RunSource, RunStatus};
use kanade_store::SqliteStore;

use crate::cards::{created, world};
use crate::scenarios::{HOME, config, delivery, week};
use crate::support::{self, Store};

/// Thu 10 Sep `hour`:`minute` in Kuala Lumpur (UTC+8).
fn kl(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 10, hour, minute, 0).unwrap() - TimeDelta::hours(8)
}

/// A run at `start` tonight, made at 08:00 with its cards (morning card 09:00).
async fn run_at<S: Store>(store: &S, start: DateTime<Utc>) -> String {
    let mut ids = RandomIds;
    let run = support::service(store, &mut ids, kl(8, 0))
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(HOME.into()),
            week_start: week(),
            datetime: start,
            bosses: vec!["XKalos".into()],
            participants: vec!["1001".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        })
        .await
        .expect("run");
    support::service(store, &mut ids, kl(8, 0))
        .as_origin(Origin::for_tests())
        .ensure_reminders(&run, &config().policy.reminders, false)
        .await
        .expect("reminders");
    run
}

/// Whether `run`'s morning card row is handled (posted or retired).
async fn morning_handled<S: Store>(store: &S, run: &str) -> bool {
    support::snapshot(store)
        .await
        .reminders
        .into_iter()
        .find(|row| row.run_id == run && row.kind == DAY_OF)
        .expect("morning row")
        .sent_at
        .is_some()
}

/// Each post's leading emoji: 📅 morning card, ⏰ countdown.
fn posted(fake: &kanade::discord::transport::FakeDiscord) -> Vec<String> {
    created(fake)
        .iter()
        .map(|post| {
            let text = post.content.as_deref().unwrap_or_default();
            text.chars().next().map(String::from).unwrap_or_default()
        })
        .collect()
}

async fn late_start_posts_the_due_morning_card<S: Store>(store: &S) {
    let world = world();
    run_at(store, kl(23, 0)).await;
    // Started at 10:00, an hour after the 09:00 ping: the first tick's
    // reconcile leaves the due card queued and dispatch posts it.
    delivery(store, &world, &world.fake)
        .tick_at(kl(10, 0))
        .await
        .expect("first tick");
    assert_eq!(
        posted(&world.fake),
        ["📅"],
        "the late morning card is posted"
    );

    // Another process later in the day: the posted card is not posted again.
    delivery(store, &world, &world.fake)
        .tick_at(kl(11, 0))
        .await
        .expect("restart tick");
    assert_eq!(
        posted(&world.fake),
        ["📅"],
        "a posted card is never reposted"
    );
}

async fn late_start_retires_a_started_runs_card<S: Store>(store: &S) {
    let world = world();
    let run = run_at(store, kl(9, 30)).await;
    // The run began at 09:30; its countdowns are past their grace as well.
    delivery(store, &world, &world.fake)
        .tick_at(kl(10, 0))
        .await
        .expect("first tick");
    assert!(posted(&world.fake).is_empty(), "{:?}", posted(&world.fake));
    assert!(morning_handled(store, &run).await, "retired unsent");
}

async fn late_start_respects_the_day_of_grace<S: Store>(store: &S) {
    let world = world();
    let run = run_at(store, kl(23, 30)).await;
    // 21:30 is 12.5 h after the 09:00 ping: past `DAY_OF_GRACE`, so dispatch
    // retires it as stale; the countdowns (22:30, 23:15) are still ahead.
    delivery(store, &world, &world.fake)
        .tick_at(kl(21, 30))
        .await
        .expect("first tick");
    assert!(posted(&world.fake).is_empty(), "{:?}", posted(&world.fake));
    assert!(morning_handled(store, &run).await, "retired as stale");
}

#[tokio::test]
async fn a_late_start_still_posts_the_due_morning_card() {
    late_start_posts_the_due_morning_card(&SqliteStore::open_temp().await).await;
}

#[tokio::test]
async fn a_late_start_retires_the_morning_card_of_a_started_run() {
    late_start_retires_a_started_runs_card(&SqliteStore::open_temp().await).await;
}

#[tokio::test]
async fn a_late_start_leaves_an_expired_morning_card_unposted() {
    late_start_respects_the_day_of_grace(&SqliteStore::open_temp().await).await;
}
