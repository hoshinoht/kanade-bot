//! v5 attendance through the tick on the SQLite store: the unknown-window
//! recount, morning mentions of unknown members only, and tallies on cards
//! and the digest.

use chrono::{DateTime, NaiveTime, TimeDelta, TimeZone, Utc, Weekday};
use kanade::bot::delivery::Delivery;
use kanade::discord::transport::Call;
use kanade_domain::attendance::AttendanceDefault;
use kanade_domain::history::{Actor, Origin, Surface};
use kanade_domain::ids::RandomIds;
use kanade_domain::schedule::{EMOJI_YES, NewFixedRun, RunStatus};
use kanade_store::SqliteStore;

use crate::scenarios::{HOME, config, now, previous_week, world};
use crate::support::{Store, mark_digest_week, service, snapshot};

/// Sat 12 Sep 21:00 in Kuala Lumpur.
fn start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 12, 13, 0, 0).unwrap()
}

/// `(content, mentioned user ids, body text)` of every message created so
/// far; the body is the embeds' descriptions and field values, or the V2
/// texts, one per line.
fn posts(calls: &[Call]) -> Vec<(String, Vec<String>, String)> {
    calls
        .iter()
        .filter_map(|call| match call {
            Call::Create { message, .. } => Some((
                message.content.clone().unwrap_or_default(),
                message
                    .allowed_mentions
                    .users
                    .iter()
                    .map(|id| id.get().to_string())
                    .collect(),
                message
                    .embeds
                    .iter()
                    .flat_map(|embed| {
                        embed
                            .description
                            .iter()
                            .cloned()
                            .chain(embed.fields.iter().map(|field| field.value.clone()))
                    })
                    .chain(crate::support::v2_texts(&message.components))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )),
            _ => None,
        })
        .collect()
}

async fn run_status<S: Store>(store: &S, run: &str) -> RunStatus {
    snapshot(store)
        .await
        .runs
        .iter()
        .find(|row| row.id == run)
        .expect("run")
        .status
}

/// Weekly Sat 21:00 with 1001 ("always in") and 1002 (no answer); the
/// previous week's digest is recorded so this week's posts.
async fn seed<S: Store>(store: &S) -> String {
    let mut ids = RandomIds;
    // Seed data is historical, not an admin action the tick should announce.
    let admin = Origin::new(Actor::system("seed"), Surface::Import);
    let mut service = service(store, &mut ids, now());
    let fixed = service
        .as_origin(admin)
        .add_fixed_run(NewFixedRun {
            owner_pinned: false,
            owner_id: "1001".into(),
            channel_id: Some(HOME.into()),
            bosses: vec!["Kalos".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(21, 0, 0).unwrap(),
            participants: vec!["1001".into(), "1002".into()],
            note: None,
        })
        .await
        .expect("timing");
    kanade_store::conformance::seed_attendance(store, &fixed, &["1001"], AttendanceDefault::OptIn)
        .await;
    mark_digest_week(store, previous_week(), now()).await;
    fixed
}

async fn v5_tick_recounts_pings_unknowns_and_shows_tallies<S: Store>(store: &S) {
    seed(store).await;
    let world = world();
    let mut delivery = Delivery::new(
        store,
        RandomIds,
        &world.fake,
        &world.alerts,
        &world.roster,
        &world.channels,
        config(),
    );
    // First tick: materialise, and the digest lists the run with its tally.
    let first = delivery.tick_at(now()).await.expect("tick");
    let run = snapshot(store)
        .await
        .runs
        .iter()
        .find(|row| row.datetime == start())
        .expect("run")
        .id
        .clone();
    assert!(first.recounted.is_empty(), "far from the window");
    let digest = posts(&world.fake.calls());
    assert_eq!(digest.len(), 1, "the digest posted");
    assert!(
        digest[0].2.contains("· 1 in, 1 waiting"),
        "digest tally: {:?}",
        digest[0].2
    );
    // One minute before the window: nothing changes.
    let early = delivery
        .tick_at(start() - TimeDelta::hours(12) - TimeDelta::minutes(1))
        .await
        .expect("tick");
    assert!(early.recounted.is_empty());
    assert_eq!(run_status(store, &run).await, RunStatus::Planned);
    // The window opens with the 09:00 morning ping: at risk, and the ping
    // names only the unknown member.
    let opened = delivery
        .tick_at(start() - TimeDelta::hours(12))
        .await
        .expect("tick");
    assert_eq!(opened.recounted, std::slice::from_ref(&run));
    assert_eq!(run_status(store, &run).await, RunStatus::AtRisk);
    let morning = posts(&world.fake.calls()).pop().expect("morning ping");
    assert!(
        morning.0.starts_with("📅 **Today — Sat 12 Sep**\n") && morning.2.contains("❗ at risk"),
        "{morning:?}"
    );
    assert_eq!(morning.1, ["1002"], "morning mentions unknowns only");
    // 1002 answers: confirmed, and cards say "expected" (1001 is assumed).
    let mut ids = RandomIds;
    let answered = service(store, &mut ids, start() - TimeDelta::hours(2))
        .as_origin(Origin::new(Actor::member("1002"), Surface::Discord))
        .apply_reaction(&run, "1002", EMOJI_YES, true)
        .await
        .expect("reaction");
    assert_eq!(answered.new_status, RunStatus::Confirmed);
    delivery
        .tick_at(start() - TimeDelta::minutes(60))
        .await
        .expect("tick");
    let countdown = posts(&world.fake.calls()).pop().expect("countdown");
    assert!(
        countdown.0.starts_with("⏰ **") && countdown.2.contains("✅ 2 in"),
        "{countdown:?}"
    );
    assert_eq!(countdown.1, ["1001", "1002"], "countdowns keep v4");
}

#[tokio::test]
async fn v5_attendance_through_the_tick() {
    v5_tick_recounts_pings_unknowns_and_shows_tallies(&SqliteStore::open_temp().await).await;
}
