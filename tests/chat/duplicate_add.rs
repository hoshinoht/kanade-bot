//! D-ADD-DUPLICATE: unlike v4, `propose_add` refuses a one-off run while an
//! open run in the same boss week (any channel) already has one of its
//! bosses and one of its party, listing that run and steering the model to
//! `propose_move`; `extra: true` is the model's way past it once the asker
//! clearly wants a second run. Weekly adds are unchanged.

use kanade::chat::answer::Generation;
use kanade::chat::tools::REFUSED;
use kanade_domain::proposals::ChangeKind;
use kanade_llm::FakeAction;
use serde_json::{Value, json};

use crate::looping::{Ports, ask_full_set, conversation, said};
use crate::model::calls;
use crate::world::{World, member, sgt_run};

const MODEL: &str = "synthetic-dupadd";
const ASTER: &str = "100000000000000101";
const BRAMBLE: &str = "100000000000000102";
const COBALT: &str = "100000000000000103";
const DUNE: &str = "100000000000000104";
const FENNEL: &str = "100000000000000105";
const GALE: &str = "100000000000000106";
const HOME: &str = "100000000000000201";
const OTHER: &str = "100000000000000202";
/// The weekly Extreme Kalos run this boss week (Fri 09 Oct 22:00).
const KALOS: &str = "k1000001";

/// A run row; the Kalos run comes from the weekly.
fn run(id: &str, bosses: &[&str], at: &str, channel: &str, party: &[&str], status: &str) -> Value {
    let mut row = sgt_run(id, bosses, at, channel, party, status);
    if id == KALOS {
        row["fixed_run_id"] = json!("f1000001");
    }
    row
}

fn kalos(channel: &str, status: &str) -> Value {
    run(
        KALOS,
        &["XKalos"],
        "2026-10-09T22:00",
        channel,
        &[ASTER, BRAMBLE, COBALT, DUNE, FENNEL],
        status,
    )
}

/// Invented: Thu 08 Oct 2026 19:00 SGT, the first day of a boss week (reset
/// Thursday 00:00). Hard Carling tonight 22:30 and, from the weekly, Extreme
/// Kalos Friday 22:00 with five members; `runs` replaces the Kalos run.
async fn world(runs: Vec<Value>) -> World {
    let mut all = vec![run(
        "c1000002",
        &["HCarling"],
        "2026-10-08T22:30",
        HOME,
        &[ASTER, GALE],
        "planned",
    )];
    all.extend(runs);
    World::new(&json!({
        "clock": "2026-10-08T19:00:00+08:00", "timezone": "Asia/Singapore",
        "reset_weekday": 3, "reset_time": "00:00",
        "uuid_sequence": (1..40).map(|n| format!("d0d0d0d0-0000-4000-8000-{n:012}")).collect::<Vec<_>>(),
        "catalog": {"bosses": [
                {"short": "Carling", "full": "Chief Carling", "difficulties": ["h"], "aliases": ["carl"]},
                {"short": "Kalos", "full": "Kalos the Guardian", "difficulties": ["x"], "aliases": ["kalos"]},
                {"short": "Limbo", "full": "Limbo", "difficulties": ["h"], "aliases": ["limbo"]}],
            "difficulties": [{"prefix": "h", "label": "Hard"}, {"prefix": "x", "label": "Extreme"}]},
        "channels": [{"id": HOME, "name": "boss-alpha"}, {"id": OTHER, "name": "boss-beta"}],
        "settings": {"guild_id": "100000000000000900", "watched_channel_ids": format!("{HOME},{OTHER}"),
                     "chat_pilot_channel_ids": format!("{HOME},{OTHER}"), "chat_pilot_category_ids": ""},
        "bot_user": {"id": "100000000000000001", "name": "Kanade"}, "self_role_id": null, "guides": [],
        "world": {
            "members": [member(ASTER, "Aster"), member(BRAMBLE, "Bramble"), member(COBALT, "Cobalt"),
                        member(DUNE, "Dune"), member(FENNEL, "Fennel"), member(GALE, "Gale")],
            "fixed": [{"id": "f1000001", "owner_id": ASTER, "channel_id": HOME, "bosses": ["XKalos"],
                       "weekday": 4, "time": "22:00",
                       "participants": [ASTER, BRAMBLE, COBALT, DUNE, FENNEL]}],
            "rsvps": [],
            "runs": all}
    }))
    .await
}

fn add(arguments: Value) -> FakeAction {
    calls(MODEL, &[("propose_add", arguments)])
}

/// The live call's arguments: four of the Kalos party, by mention and name.
fn party_add(boss: &str, when: &str) -> Value {
    json!({"boss": boss, "when": when,
           "participants": format!("<@{BRAMBLE}>, <@{COBALT}>, <@{DUNE}>, Fennel")})
}

/// Aster answers tonight's Carling reminder: "10pm we xkalos first".
async fn ask(world: &mut World, call: Value) -> (Generation, Ports) {
    let ctx = world.context(&json!({"author_id": ASTER, "channel_id": HOME}));
    let conversation = conversation(format!(
        "Aster: 10pm we xkalos first <@{BRAMBLE}> <@{COBALT}> <@{DUNE}>"
    ));
    let actions = vec![add(call), FakeAction::Response(said(MODEL, "Noted."))];
    let (generation, ports, _) =
        ask_full_set(world, &ctx, MODEL, conversation, actions, 32768).await;
    (generation, ports)
}

fn outcome(generation: &Generation) -> &kanade::chat::tools::ToolOutcome {
    &generation
        .outcomes
        .iter()
        .find(|round| round.outcome.name == "propose_add")
        .expect("a propose_add call")
        .outcome
}

/// No card; the refusal lists the existing run and names the way forward.
fn refused(generation: &Generation, ports: &Ports, listed: &str) {
    let outcome = outcome(generation);
    assert_eq!(outcome.error, Some(REFUSED), "{}", outcome.output);
    let output = &outcome.output;
    assert!(
        output.starts_with("That would be a second run of a boss this boss week already has"),
        "{output}"
    );
    assert!(output.contains(&format!("`[{listed}]` **")), "{output}");
    assert!(output.contains("propose_move"), "{output}");
    assert!(output.contains("`extra` true"), "{output}");
    assert!(output.ends_with("No card went up."), "{output}");
    assert!(generation.created.is_empty() && generation.posted.is_empty());
    assert!(ports.posted.lock().unwrap().is_empty());
}

/// One card of `kind`.
fn carded(generation: &Generation, ports: &Ports, kind: ChangeKind) {
    let outcome = outcome(generation);
    assert!(outcome.ok, "{}", outcome.output);
    let posted = ports.posted.lock().unwrap();
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].kind, kind);
    assert_eq!(generation.created.len(), 1);
}

#[tokio::test]
async fn the_live_call_a_one_off_kalos_beside_the_weekly_one_is_refused() {
    let mut world = world(vec![kalos(HOME, "planned")]).await;
    let (generation, ports) = ask(&mut world, party_add("XKalos", "10pm")).await;
    refused(&generation, &ports, KALOS);
    let output = &outcome(&generation).output;
    assert!(output.contains("Extreme Kalos"), "{output}");
    assert!(output.contains("22:00"), "{output}");
    assert!(output.contains("`planned`"), "{output}");
    assert!(output.contains("`0/5 yes`"), "{output}");
    // Tonight's Carling shares a member but no boss.
    assert!(!output.contains("c1000002"), "{output}");
}

#[tokio::test]
async fn a_run_in_another_channel_counts_too() {
    let mut world = world(vec![kalos(OTHER, "planned")]).await;
    let (generation, ports) = ask(&mut world, party_add("XKalos", "10pm")).await;
    refused(&generation, &ports, KALOS);
}

#[tokio::test]
async fn a_different_boss_gets_its_card() {
    let mut world = world(vec![kalos(HOME, "planned")]).await;
    let (generation, ports) = ask(&mut world, party_add("HLimbo", "10pm")).await;
    carded(&generation, &ports, ChangeKind::Add);
}

#[tokio::test]
async fn the_same_boss_next_boss_week_gets_its_card() {
    let mut world = world(vec![kalos(HOME, "planned")]).await;
    let (generation, ports) = ask(&mut world, party_add("XKalos", "2026-10-15 22:00")).await;
    carded(&generation, &ports, ChangeKind::Add);
}

#[tokio::test]
async fn nobody_shared_gets_its_card() {
    let mut world = world(vec![run(
        KALOS,
        &["XKalos"],
        "2026-10-09T22:00",
        HOME,
        &[BRAMBLE, COBALT, DUNE, FENNEL],
        "planned",
    )])
    .await;
    let (generation, ports) = ask(
        &mut world,
        json!({"boss": "XKalos", "when": "10pm", "participants": format!("<@{GALE}>")}),
    )
    .await;
    carded(&generation, &ports, ChangeKind::Add);
}

#[tokio::test]
async fn an_extra_run_the_asker_clearly_wants_gets_its_card() {
    let mut world = world(vec![kalos(HOME, "planned")]).await;
    let mut call = party_add("XKalos", "10pm");
    call["extra"] = json!(true);
    let (generation, ports) = ask(&mut world, call).await;
    carded(&generation, &ports, ChangeKind::Add);
}

#[tokio::test]
async fn a_cancelled_run_is_no_duplicate() {
    let mut world = world(vec![kalos(HOME, "cancelled")]).await;
    let (generation, ports) = ask(&mut world, party_add("XKalos", "10pm")).await;
    carded(&generation, &ports, ChangeKind::Add);
}

#[tokio::test]
async fn a_weekly_add_is_unchanged() {
    let mut world = world(vec![kalos(HOME, "planned")]).await;
    let mut call = party_add("XKalos", "10pm");
    call["weekly"] = json!(true);
    let (generation, ports) = ask(&mut world, call).await;
    carded(&generation, &ports, ChangeKind::Fix);
}
