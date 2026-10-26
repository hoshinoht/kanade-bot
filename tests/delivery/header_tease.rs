//! Tease slots (beta.9 tease-prompt, D9–D12): with teasing on, an opted-in
//! party member with an estimate is offered as one more `{cN}` with their
//! share and the admins' note; with teasing off or nobody opted in the
//! prompt is exactly the estimate-context one. A stored tease line falls
//! back to the seed at the next render once teasing is off or the member
//! opts out, and a note that now names someone on the roster is not sent.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kanade::bot::delivery::cards::{CardKit, HeaderSwitches};
use kanade::bot::delivery::{PartyEstimates, RunEstimate};
use kanade::domain::members::{Member, MemberProfile, PingLevel};
use kanade::domain::notify::ReminderCardStore;

use crate::cards::{Script, Scripted, created, edits, refresher, rewriting, shows_phrase, world};
use crate::header_slots::{Fixed, carrier, estimate, owner, pregenerated_in};
use crate::scenarios::{self, World, now};

const PARTY: [&str; 3] = ["1001", "1002", "1003"];
const NOTE: &str = "Always lost on the way to the portal";
/// Another roster member's character, outside the party.
const OTHER_IGN: &str = "Zephyrine";

/// 1001 Lumirae (most), 1002 Tobiwan, 1003 Kestrelle (least).
fn trio() -> RunEstimate {
    let mut trio = estimate(false);
    trio.party.carriers.push(carrier("Kestrelle", 10.0));
    trio.owners.push(owner("1003"));
    trio.igns.push("Kestrelle".into());
    trio
}

/// Estimates always on; teasing as `teasing` says now.
fn tease_kit(rewriter: &Arc<Scripted>, teasing: &Arc<AtomicBool>) -> CardKit {
    let teasing = Arc::clone(teasing);
    let igns = vec![
        "Lumirae".into(),
        "Tobiwan".into(),
        "Kestrelle".into(),
        OTHER_IGN.into(),
    ];
    CardKit {
        estimates: Some(Arc::new(Fixed(trio(), igns)) as Arc<dyn PartyEstimates>),
        switches: Some(Arc::new(move || HeaderSwitches {
            estimates: true,
            teasing: teasing.load(Ordering::SeqCst),
        })),
        ..rewriting(rewriter)
    }
}

/// The cards world plus 1003 "Nimbus", opted in or not, with `note`.
fn tease_world(tease_ok: bool, note: &str) -> World {
    let mut world = world();
    world.roster.upsert_profile(MemberProfile {
        member: Member {
            user_id: "1003".into(),
            display_name: Some("Nimbus".into()),
            has_role: true,
            ping_level: PingLevel::Off,
            ..Member::default()
        },
        tease_ok,
        running_joke: Some(note.into()),
        ..MemberProfile::default()
    });
    world
}

/// The system prompt one pre-generation pass sends.
async fn prompt(teasing: bool, world: &World) -> String {
    let rewriter = Scripted::new(Script::Reply("Onward, team~"));
    let switch = Arc::new(AtomicBool::new(teasing));
    pregenerated_in(&tease_kit(&rewriter, &switch), world, &PARTY).await;
    rewriter.systems()[0].clone()
}

#[tokio::test(start_paused = true)]
async fn teasing_off_or_opted_out_sends_todays_context_prompt() {
    let context = prompt(false, &tease_world(true, NOTE)).await;
    assert!(context.contains("Party estimate") && context.contains("{c2}"));
    assert!(!context.contains("{c3}") && !context.contains("teased"));
    // Opted out with teasing on: byte for byte the same prompt.
    assert_eq!(prompt(true, &tease_world(false, NOTE)).await, context);
    // Opted in with teasing on: the tease slot, its share and note, no name.
    let tease = prompt(true, &tease_world(true, NOTE)).await;
    assert!(tease.contains("{c3} (9% of the party's estimate) has asked to be teased"));
    assert!(tease.contains(&format!("\"{NOTE}\"")));
    for private in [
        "Kestrelle",
        "Nimbus",
        "Lumirae",
        "Aria",
        "Bex",
        "1003",
        "char-",
    ] {
        assert!(!tease.contains(private), "{private} reached the prompt");
    }
}

#[tokio::test(start_paused = true)]
async fn a_note_that_now_names_someone_on_the_roster_is_not_sent() {
    // Bex is a party member's name; Zephyrine another roster member's IGN.
    for note in ["Still owes Bex a nap", "Naps more than zephyrine"] {
        let sent = prompt(true, &tease_world(true, note)).await;
        assert!(sent.contains("{c3}") && sent.contains("asked to be teased"));
        assert!(
            !sent.contains(note) && !sent.contains("running joke"),
            "{note}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_tease_line_falls_back_to_the_seed_once_teasing_or_the_opt_in_goes() {
    let rewriter = Scripted::new(Script::Reply("Go {c1}, wake up {c3}!"));
    let teasing = Arc::new(AtomicBool::new(true));
    let cards = tease_kit(&rewriter, &teasing);
    let world = tease_world(true, NOTE);
    let (store, run_id, key) = pregenerated_in(&cards, &world, &PARTY).await;
    let record = store
        .card_record(&key)
        .await
        .expect("read")
        .expect("stored");
    let slots = record.slots.expect("slot map");
    let marks: Vec<(&str, &str, bool)> = slots
        .slots
        .iter()
        .map(|slot| (slot.slot.as_str(), slot.member_id.as_str(), slot.tease))
        .collect();
    assert_eq!(marks, [("{c1}", "1001", false), ("{c3}", "1003", true)]);

    let fake = Arc::new(crate::support::fake());
    let mut delivery = scenarios::delivery(&*store, &world, &*fake).with_cards(cards.clone());
    delivery.dispatch_reminders(now()).await.expect("dispatch");
    let posted = created(&fake).pop().and_then(|message| message.content);
    let line = "Go Lumirae, wake up Kestrelle!";
    assert!(shows_phrase(posted.as_deref(), line), "{posted:?}");

    let refresh = refresher(&store, &fake, &world, cards.clone());
    assert_eq!(refresh.refresh(std::slice::from_ref(&run_id)).await, 1);
    let edited = edits(&fake).pop().and_then(|edit| edit.content);
    assert!(shows_phrase(edited.as_deref(), line), "{edited:?}");

    // The kill switch flipped after pregen: the next render is the seed.
    teasing.store(false, Ordering::SeqCst);
    assert_eq!(refresh.refresh(std::slice::from_ref(&run_id)).await, 1);
    let edited = edits(&fake).pop().and_then(|edit| edit.content);
    assert!(shows_phrase(edited.as_deref(), "Onward!"), "{edited:?}");
    assert!(!edited.unwrap_or_default().contains("Kestrelle"));

    // Back on, then the member opts out: the seed again.
    teasing.store(true, Ordering::SeqCst);
    assert_eq!(refresh.refresh(std::slice::from_ref(&run_id)).await, 1);
    let edited = edits(&fake).pop().and_then(|edit| edit.content);
    assert!(shows_phrase(edited.as_deref(), line), "{edited:?}");
    let opted_out = tease_world(false, NOTE);
    let refresh = refresher(&store, &fake, &opted_out, cards);
    assert_eq!(refresh.refresh(std::slice::from_ref(&run_id)).await, 1);
    let edited = edits(&fake).pop().and_then(|edit| edit.content);
    assert!(shows_phrase(edited.as_deref(), "Onward!"), "{edited:?}");
}
