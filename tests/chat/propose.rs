//! `propose.json`: every `propose_*` tool through the dispatcher and the
//! scheduler's propose API. v4's `amendments` rows are projected from the
//! v5 proposal store (status) and the cards the tools handed over (facts).

use std::collections::BTreeMap;

use kanade::chat::tools::{ProposalCard, REFUSED};
use kanade::domain::drafts::{DraftStatus, ProposalStore};
use kanade::domain::ids::IdGenerator;
use kanade::domain::proposals::Refusal;
use kanade::domain::schedule::Change;
use kanade::infrastructure::llm::identity::PassthroughSession;
use serde_json::{Value, json};

use crate::common::{iso, opt_iso, text};
use crate::read_tools::outcome_json;
use crate::support::{check_family, load, unknown_op, value};
use crate::world::World;

/// v4 `host.amendments`: every proposal in creation order.
async fn amendments(world: &World, cards: &BTreeMap<String, ProposalCard>) -> Value {
    let stored = world
        .service
        .store()
        .list_proposals(false)
        .await
        .expect("list proposals");
    let rows: Vec<Value> = stored
        .iter()
        .map(|proposal| {
            let draft = &proposal.draft;
            let card = &cards[&draft.id];
            let status = match draft.status {
                DraftStatus::Submitted => "proposed",
                DraftStatus::Discarded if draft.close_reason.as_deref() == Some("superseded") => {
                    "superseded"
                }
                other => panic!("{}: unexpected status {other}", draft.id),
            };
            let mut payload = card.payload.clone();
            // The extractor pipeline's bookkeeping key; chat names nobody else.
            payload.insert("also_mentioned".into(), json!([]));
            json!({
                "id": draft.id,
                "kind": card.kind.as_str(),
                "status": status,
                "week_start": iso(card.week_start),
                "run_id": card.run_id,
                "new_datetime": opt_iso(card.new_datetime),
                "bosses": card.bosses,
                "participants": card.participants,
                "rsvp": card.rsvp.map(|state| state.as_str()),
                "confidence": 1.0,
                "channel_id": card.channel_id,
                "is_question": false,
                "evidence_msg_ids": card.evidence_message_ids,
                "summary": card.summary,
                "payload": payload,
                // Handed to the caller for posting (delivery is not the tool's).
                "card_posted": true,
            })
        })
        .collect();
    json!(rows)
}

async fn replay(case: Value) -> Vec<Value> {
    let input = &case["input"];
    let v4_steps = case["expected"]["steps"].as_array().expect("expected");
    let mut world = World::new(input).await;
    let seeded = world.snapshot().await;
    let mut session = PassthroughSession;
    let mut cards = BTreeMap::new();
    let mut out = Vec::new();
    for (index, step) in input["steps"].as_array().expect("steps").iter().enumerate() {
        if text(&step["op"]) != "run" {
            unknown_op("propose", text(&step["op"]));
        }
        let outcome = world.run_tool(step, &mut session).await;
        // v4 drew two ids per card (row and delivery claim), v5 one per
        // proposal; burn the rest so later ids stay aligned.
        let v4_created = v4_steps[index]["value"]["outcome"]["created"]
            .as_array()
            .map_or(0, Vec::len);
        for _ in outcome.cards.len()..2 * v4_created {
            world.ids.clone().new_id();
        }
        for card in &outcome.cards {
            cards.insert(card.proposal_id.clone(), card.clone());
        }
        out.push(value(json!({
            "outcome": outcome_json(&outcome),
            "amendments": amendments(&world, &cards).await,
        })));
    }
    assert_eq!(
        world.snapshot().await,
        seeded,
        "proposals never write schedule rows"
    );
    out
}

/// Every refusal in the vectors is the tools' own pre-check, so none needs
/// `D-PROPOSE-REFUSES`; that path is covered below.
#[tokio::test]
async fn the_propose_family_replays_through_the_scheduler_propose_api() {
    assert_eq!(check_family("propose", &[], replay).await, (6, 40));
}

/// `D-PROPOSE-REFUSES`: a change the scheduler cannot apply (here a move into
/// a boss week its weekly already holds) is refused up front with v4's ✅-time
/// reason and no card, where v4 posted a card that failed at ✅.
#[tokio::test]
async fn an_unworkable_change_is_refused_up_front_without_a_card() {
    let file = load("propose.json");
    let mut world = World::new(&file["cases"][0]["input"]).await;
    let mut held = world.snapshot().await.runs[0].clone();
    assert_eq!(held.id, "b2b2b2b2-0000-4000-8000-000000000002");
    // Kalos's weekly already has next boss week's run.
    held.id = "b2b2b2b2-0000-4000-8000-00000000000f".into();
    held.week_start = crate::common::utc(&json!("2026-09-10T00:00:00+08:00"));
    held.datetime = crate::common::utc(&json!("2026-09-15T23:00:00+08:00"));
    world.put(Change::PutRun(held)).await;
    let before = world.snapshot().await;

    let step = json!({
        "author_id": "22",
        "channel_id": "901",
        "tool": "propose_move",
        "arguments": {"run_query": "b2b2b2b2-0000-4000-8000-000000000002", "to_when": "mon 23:00"},
    });
    let outcome = world.run_tool(&step, &mut PassthroughSession).await;
    assert_eq!(outcome.error, Some(REFUSED), "{}", outcome.output);
    assert_eq!(
        outcome.output,
        format!(
            "That cannot be proposed: {}. No card went up -- tell them why in your own words, and never say a card is up.",
            Refusal::WeeklyHoldsWeek
        )
    );
    assert!(outcome.cards.is_empty() && outcome.created.is_empty());
    let stored = world
        .service
        .store()
        .list_proposals(false)
        .await
        .expect("list");
    assert!(stored.is_empty(), "nothing was proposed");
    assert_eq!(world.snapshot().await, before);
}
