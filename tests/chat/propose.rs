//! `propose_*` tools through the dispatcher and the scheduler's propose API.

use kanade::chat::tools::REFUSED;
use kanade::domain::drafts::ProposalStore;
use kanade::domain::proposals::Refusal;
use kanade::domain::schedule::Change;
use kanade::infrastructure::llm::identity::PassthroughSession;
use serde_json::json;

use crate::support::world_input;
use crate::world::World;

/// `D-PROPOSE-REFUSES`: a change the scheduler cannot apply (here a move into
/// a boss week its weekly already holds) is refused up front with v4's ✅-time
/// reason and no card, where v4 posted a card that failed at ✅.
#[tokio::test]
async fn an_unworkable_change_is_refused_up_front_without_a_card() {
    let mut world = World::new(&world_input()).await;
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
