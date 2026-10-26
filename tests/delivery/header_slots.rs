//! Estimate-flavoured headers (beta.9 header-store): pre-generation sends
//! estimate context only with the port and the live
//! switch on; the line's `{cN}` is stored with its slot map and filled at
//! every render, which falls back to the seed when the switch is off or a
//! slotted member left the party. No render ever shows a brace.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kanade::bot::delivery::cards::{
    CardContext, CardKit, DifficultyMarks, HeaderSwitches, resolve_heading,
};
use kanade::bot::delivery::{EstimateFuture, IgnFuture, PartyEstimates, RunEstimate};
use kanade::infrastructure::scouter::CarrierOwner;
use kanade::infrastructure::store::SqliteStore;
use kanade_domain::catalog::BossTable;
use kanade_domain::chardata::{Carrier, PartyShares};
use kanade_domain::notify::{DedupeKey, DeliveryTarget, IntentContent, ReminderCardStore};
use kanade_domain::schedule::{Run, ScheduleSnapshot};
use kanade_domain::scheduler::{ScheduleStore, Scope};

use crate::cards::{
    Script, Scripted, before_due, catalog, created, edits, kalos_countdown, pregen, refresher,
    rewriting, shows_phrase, world,
};
use crate::scenarios::{self, World, now};

/// A port answering one estimate for every run, and `igns` as every
/// roster character's IGN.
pub(crate) struct Fixed(pub(crate) RunEstimate, pub(crate) Vec<String>);

impl PartyEstimates for Fixed {
    fn run<'a>(
        &'a self,
        _run: &'a Run,
        _schedule: &'a ScheduleSnapshot,
        _catalog: &'a BossTable,
    ) -> EstimateFuture<'a> {
        let found = self.0.clone();
        Box::pin(async move { Some(found) })
    }

    fn igns<'a>(&'a self, _members: &'a [String]) -> IgnFuture<'a> {
        let igns = self.1.clone();
        Box::pin(async move { Some(igns) })
    }
}

/// An invented carrier with one boss's effective %.
pub(crate) fn carrier(ign: &str, value: f64) -> Carrier {
    Carrier {
        ign: ign.into(),
        crit_assumed: false,
        effective: vec![Some(value)],
        per_boss: Vec::new(),
    }
}

/// The carrier owner of member `id`.
pub(crate) fn owner(id: &str) -> CarrierOwner {
    CarrierOwner {
        member_id: id.into(),
        character_id: format!("char-{id}"),
    }
}

/// Invented carriers: 1001 brings Lumirae (most), 1002 Tobiwan.
pub(crate) fn estimate(stale: bool) -> RunEstimate {
    RunEstimate {
        party: PartyShares {
            bosses: vec!["XKalos".into()],
            carriers: vec![carrier("Lumirae", 70.0), carrier("Tobiwan", 30.0)],
            missing: 0,
        },
        minutes: vec![Some(21.5)],
        possible: vec![Some(true)],
        owners: vec![owner("1001"), owner("1002")],
        igns: vec!["Lumirae".into(), "Tobiwan".into()],
        older_than: stale.then_some(7),
        ..RunEstimate::default()
    }
}

fn kit(rewriter: &Arc<Scripted>, on: &Arc<AtomicBool>, estimate: Option<RunEstimate>) -> CardKit {
    let on = Arc::clone(on);
    CardKit {
        estimates: estimate
            .map(|estimate| Arc::new(Fixed(estimate, Vec::new())) as Arc<dyn PartyEstimates>),
        switches: Some(Arc::new(move || HeaderSwitches {
            estimates: on.load(Ordering::SeqCst),
            teasing: false,
        })),
        ..rewriting(rewriter)
    }
}

/// The Kalos countdown (party 1001, 1002) after one pre-generation pass;
/// returns the store, the run and the record key.
async fn pregenerated(kit: &CardKit) -> (Arc<SqliteStore>, String, String) {
    pregenerated_in(kit, &world(), &["1001", "1002"]).await
}

/// The Kalos countdown for `party` after one pre-generation pass over
/// `world`'s roster; returns the store, the run and the record key.
pub(crate) async fn pregenerated_in(
    kit: &CardKit,
    world: &World,
    party: &[&str],
) -> (Arc<SqliteStore>, String, String) {
    let store = Arc::new(SqliteStore::open_temp().await);
    let run_id = kalos_countdown(&*store, party).await;
    pregen(&store, world, kit, before_due()).pass().await;
    let schedule = store.load(&Scope::All).await.expect("schedule");
    let reminder = schedule
        .reminders
        .iter()
        .find(|row| row.run_id == run_id)
        .expect("countdown");
    let key = DedupeKey::native(&[DeliveryTarget::Reminder(reminder.id.clone())])
        .expect("key")
        .as_str()
        .to_owned();
    (store, run_id, key)
}

#[tokio::test(start_paused = true)]
async fn a_carrier_placeholder_is_stored_and_filled_at_send_and_refresh() {
    let rewriter = Scripted::new(Script::Reply("Go {c1}, carry us!"));
    let on = Arc::new(AtomicBool::new(true));
    let cards = kit(&rewriter, &on, Some(estimate(false)));
    let (store, run_id, key) = pregenerated(&cards).await;
    let record = store
        .card_record(&key)
        .await
        .expect("read")
        .expect("stored");
    assert_eq!(record.heading.as_deref(), Some("Go {c1}, carry us!"));
    let slots = record.slots.expect("slot map");
    assert!(slots.used_estimates);
    assert_eq!(
        (
            slots.slots[0].member_id.as_str(),
            slots.slots[0].ign.as_str()
        ),
        ("1001", "Lumirae")
    );
    // The model saw totals and placeholders, never a name.
    let system = &rewriter.systems()[0];
    assert!(system.contains("Party estimate") && system.contains("{c1}"));
    for private in ["Lumirae", "Tobiwan", "Aria", "Bex", "1001", "char-"] {
        assert!(!system.contains(private), "{private} reached the prompt");
    }

    let world = world();
    let fake = Arc::new(crate::support::fake());
    let mut delivery = scenarios::delivery(&*store, &world, &*fake).with_cards(cards.clone());
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    let posted = created(&fake).pop().expect("posted");
    let content = posted.content.clone().unwrap_or_default();
    assert!(
        shows_phrase(Some(&content), "Go Lumirae, carry us!"),
        "{content}"
    );
    assert!(!content.contains('{'), "{content}");

    // A refresh with nothing changed shows the same line.
    let refresh = refresher(&store, &fake, &world, cards.clone());
    assert_eq!(refresh.refresh(std::slice::from_ref(&run_id)).await, 1);
    let edit = edits(&fake).pop().expect("an edit");
    let edited = edit.content.unwrap_or_default();
    assert!(
        shows_phrase(Some(&edited), "Go Lumirae, carry us!"),
        "{edited}"
    );

    // The switch turned off after pregen: the next render is the seed.
    on.store(false, Ordering::SeqCst);
    assert_eq!(refresh.refresh(std::slice::from_ref(&run_id)).await, 1);
    let edit = edits(&fake).pop().expect("an edit");
    let edited = edit.content.unwrap_or_default();
    assert!(shows_phrase(Some(&edited), "Onward!"), "{edited}");
    assert!(!edited.contains('{') && !edited.contains("Lumirae"));
}

#[tokio::test(start_paused = true)]
async fn a_slotted_member_who_left_the_party_gets_the_seed() {
    let rewriter = Scripted::new(Script::Reply("Go {c1}, carry us!"));
    let on = Arc::new(AtomicBool::new(true));
    let cards = kit(&rewriter, &on, Some(estimate(false)));
    let (store, run_id, key) = pregenerated(&cards).await;
    let record = store
        .card_record(&key)
        .await
        .expect("read")
        .expect("stored");
    let mut schedule = store.load(&Scope::All).await.expect("schedule");
    let catalog = catalog();
    let roster = world().roster;
    let marks = DifficultyMarks::default();
    let policy = scenarios::config().policy;
    let content = IntentContent::Countdown {
        run_id: run_id.clone(),
        minutes: 15,
    };
    let switches = HeaderSwitches {
        estimates: true,
        teasing: false,
    };
    let resolve = |schedule: &ScheduleSnapshot| {
        let ctx = CardContext {
            schedule,
            zone: policy.zone(),
            quiet: false,
            members: &roster,
            catalog: Some(&catalog),
            marks: &marks,
            v2: None,
        };
        resolve_heading(&record, &content, &ctx, switches)
    };
    assert_eq!(resolve(&schedule).as_deref(), Some("Go Lumirae, carry us!"));
    let run = schedule
        .runs
        .iter_mut()
        .find(|run| run.id == run_id)
        .expect("run");
    run.participants.retain(|id| id != "1001");
    assert_eq!(resolve(&schedule), None, "the seed");
}

#[tokio::test(start_paused = true)]
async fn without_estimates_the_switch_or_a_fresh_estimate_the_prompt_is_todays() {
    let baseline = Scripted::new(Script::Reply("Go {c1}!"));
    let off = Arc::new(AtomicBool::new(false));
    pregenerated(&kit(&baseline, &off, None)).await;
    let today = baseline.systems()[0].clone();
    assert!(!today.contains("Party estimate"));
    let on = Arc::new(AtomicBool::new(true));
    for (estimate, switch) in [
        (None, &on),
        (Some(estimate(false)), &off),
        (Some(estimate(true)), &on),
    ] {
        let rewriter = Scripted::new(Script::Reply("Go {c1}!"));
        let cards = kit(&rewriter, switch, estimate);
        let (store, _, key) = pregenerated(&cards).await;
        assert_eq!(rewriter.systems()[0], today);
        // `{c1}` without context is refused: nothing stored, the seed posts.
        assert_eq!(store.card_record(&key).await.expect("read"), None);
        let world = world();
        let mut delivery = scenarios::delivery(&*store, &world, &world.fake).with_cards(cards);
        delivery.dispatch_reminders(now()).await.expect("dispatch");
        let content = created(&world.fake)
            .pop()
            .and_then(|message| message.content);
        assert!(shows_phrase(content.as_deref(), "Onward!"), "{content:?}");
    }
}
