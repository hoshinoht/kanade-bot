//! A moved run's reminder cards through the tick. A move rebuilds the run's
//! reminder rows (`ensure_reminders` with `rebuild`): the old rows, the sent
//! morning card's included, are withdrawn and new ones written; a new slot
//! already past is written as sent, so the tick never posts it again, while a
//! slot still ahead is posted when due.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use kanade::infrastructure::store::SqliteStore;
use kanade_domain::history::Origin;
use kanade_domain::ids::RandomIds;
use kanade_domain::schedule::{NewRun, RunSource, RunStatus};

use crate::cards::{created, world};
use crate::scenarios::{HOME, config, delivery, now, week};
use crate::support::{self, Store};

/// Thu 10 Sep `hour`:`minute` in Kuala Lumpur (UTC+8).
fn kl(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 10, hour, minute, 0).unwrap() - TimeDelta::hours(8)
}

/// `(kind, fire_at, sent)` of `run`'s reminder rows.
async fn rows<S: Store>(store: &S, run: &str) -> Vec<(String, DateTime<Utc>, bool)> {
    let mut rows: Vec<_> = support::snapshot(store)
        .await
        .reminders
        .into_iter()
        .filter(|row| row.run_id == run)
        .map(|row| (row.kind, row.fire_at, row.sent_at.is_some()))
        .collect();
    rows.sort();
    rows
}

async fn sent_morning_card_is_never_reposted<S: Store>(store: &S) {
    let world = world();
    // Created at 08:00 for 23:00 tonight: its morning card fires at 09:00
    // and is posted then.
    let mut ids = RandomIds;
    let run = support::service(store, &mut ids, kl(8, 0))
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(HOME.into()),
            week_start: week(),
            datetime: kl(23, 0),
            bosses: vec!["XKalos".into()],
            participants: vec!["1001".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        })
        .await
        .expect("run");
    // Its cards, as materialisation places them.
    support::service(store, &mut ids, kl(8, 0))
        .as_origin(Origin::for_tests())
        .ensure_reminders(&run, &config().policy.reminders, false)
        .await
        .expect("reminders");
    let mut delivery = delivery(store, &world, &world.fake);
    // The process's first tick materialises (before 09:00, so the day-of
    // reconcile leaves the row queued); the next posts it.
    for at in [kl(8, 30), kl(9, 1)] {
        delivery.tick_at(at).await.expect("morning tick");
    }
    assert_eq!(posted(&world.fake), ["📅"], "the morning card is posted");

    // At 20:00 the run moves to 23:30: the rebuilt morning row's 09:00 slot
    // has passed, so it is written as sent; the countdowns are ahead.
    let mut ids = RandomIds;
    support::service(store, &mut ids, now())
        .as_origin(Origin::for_tests())
        .amend_run(&run, kl(23, 30), &config().policy)
        .await
        .expect("move");
    assert_eq!(
        rows(store, &run).await,
        [
            ("countdown_15".to_owned(), kl(23, 15), false),
            ("countdown_60".to_owned(), kl(22, 30), false),
            ("day_of".to_owned(), kl(9, 0), true),
        ]
    );

    for at in [now() + TimeDelta::minutes(1), kl(22, 29)] {
        delivery.tick_at(at).await.expect("tick");
    }
    assert_eq!(
        posted(&world.fake),
        ["📅", "🔁"],
        "the move's notice, and no second morning card"
    );
    delivery.tick_at(kl(22, 30)).await.expect("countdown tick");
    assert_eq!(
        posted(&world.fake),
        ["📅", "🔁", "⏰"],
        "the T-1h card for the new time is posted when due"
    );
}

/// Each post's leading emoji: 📅 morning card, ⏰ countdown, 🔁 move notice.
fn posted(fake: &kanade::bot::transport::FakeDiscord) -> Vec<String> {
    created(fake)
        .iter()
        .map(|post| {
            let text = post.content.as_deref().unwrap_or_default();
            text.chars().next().map(String::from).unwrap_or_default()
        })
        .collect()
}

#[tokio::test]
async fn a_moved_run_never_reposts_its_sent_morning_card() {
    sent_morning_card_is_never_reposted(&SqliteStore::open_temp().await).await;
}
