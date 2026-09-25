//! Dynamic tool loading (v5 deviation from v4's all-tools-every-round):
//! routing per intent, stable order, `request_tools`, unoffered-tool
//! refusal, read-only turns and prompt-size reduction.

use std::collections::BTreeSet;

use kanade::chat::tools::bundles::{
    Bundle, CardContext, DEFAULT_TOOL_ROUNDS, Intent, Signals, TOOL_ROUNDS, ToolOffer,
    V4_MAX_TOOL_ROUNDS, select,
};
use kanade::chat::tools::{READ_ONLY_TURN, REFUSED, ToolName, UNKNOWN};
use kanade::domain::drafts::ProposalStore;
use kanade::infrastructure::llm::identity::PassthroughSession;
use serde_json::{Value, json};

use crate::support::load;
use crate::world::World;

fn picked(text: &str, intent: Option<Intent>, card: Option<CardContext>) -> BTreeSet<Bundle> {
    select(Signals { text, intent, card })
}

#[test]
fn routing_picks_bundles_per_intent_and_errs_toward_including() {
    use Bundle::*;
    assert!(picked("what's on tonight?", None, None).is_empty());
    assert!(picked("who is on hstar", Some(Intent::Read), None).is_empty());
    assert_eq!(
        picked("can we move hstar to friday", None, None),
        [RunWrites].into()
    );
    assert_eq!(
        picked("i can't make it tonight", None, None),
        [RunWrites].into()
    );
    assert_eq!(
        picked("change the weekly kalos to thursday", None, None),
        [FixedWrites].into()
    );
    assert_eq!(
        picked("how do we dodge the phase 2 lasers", None, None),
        [Strategy].into()
    );
    assert_eq!(
        picked("hi", Some(Intent::Strategy), None),
        [Strategy].into()
    );
    assert_eq!(
        picked("hi", Some(Intent::RunChange), None),
        [RunWrites].into()
    );
    assert_eq!(
        picked("hi", Some(Intent::WeeklyChange), None),
        [FixedWrites].into()
    );
    assert_eq!(
        picked("hi", Some(Intent::Unclear), None),
        [Strategy, RunWrites, FixedWrites].into()
    );
    // A label never removes what the rules found.
    assert_eq!(
        picked("cancel tonight", Some(Intent::Read), None),
        [RunWrites].into()
    );
    assert_eq!(
        picked("ok", None, Some(CardContext::Run)),
        [RunWrites].into()
    );
    assert_eq!(
        picked("ok", None, Some(CardContext::Weekly)),
        [FixedWrites].into()
    );
}

#[test]
fn bundles_come_in_a_stable_order_with_the_hatch_after_read() {
    let read = ToolOffer::dynamic([], false);
    assert_eq!(
        read.names(),
        [
            "get_schedule",
            "get_run",
            "list_bosses",
            "get_pending",
            "list_fixed",
            "request_tools"
        ]
    );
    let everything = ToolOffer::dynamic(
        [Bundle::FixedWrites, Bundle::Strategy, Bundle::RunWrites],
        false,
    );
    let names = everything.names();
    assert_eq!(
        &names[..6],
        read.names().as_slice(),
        "READ prefix is stable"
    );
    assert_eq!(
        &names[6..],
        [
            "get_boss_strategy",
            "propose_move",
            "propose_add",
            "propose_cancel",
            "propose_rsvp",
            "propose_change_fixed",
            "propose_remove_fixed"
        ]
    );
    let surface: Value = serde_json::from_str(&everything.surface_text()).expect("JSON");
    assert_eq!(surface.as_array().map(Vec::len), Some(13));
    // Read-only turns never offer writes, whatever was chosen.
    assert_eq!(
        ToolOffer::dynamic([Bundle::RunWrites, Bundle::Strategy], true).names()[6..],
        ["get_boss_strategy"]
    );
}

#[test]
fn a_read_question_costs_far_less_prompt_than_v4s_surface() {
    let v4 = ToolOffer::full_set(false).estimated_tokens();
    let v4_read_only = ToolOffer::full_set(true).estimated_tokens();
    let read = ToolOffer::dynamic([], false).estimated_tokens();
    assert_eq!((v4, v4_read_only), (3047, 1045), "v4's measured surfaces");
    assert!(read * 2 < v4, "{read} tokens for a read question vs {v4}");
    assert!(read < v4_read_only, "{read} vs v4 read-only {v4_read_only}");
}

#[test]
fn round_caps_follow_the_user_decision() {
    assert_eq!((V4_MAX_TOOL_ROUNDS, DEFAULT_TOOL_ROUNDS), (4, 8));
    assert_eq!(TOOL_ROUNDS, 1..=12);
}

async fn world() -> World {
    let file = load("read_tools.json");
    World::new(&file["cases"][0]["input"]).await
}

async fn proposals(world: &World) -> usize {
    world
        .service
        .store()
        .list_proposals(false)
        .await
        .expect("list")
        .len()
}

#[tokio::test]
async fn unoffered_tools_are_refused_until_requested_and_requests_are_once() {
    let mut world = world().await;
    let mut session = PassthroughSession;
    let mut offer = ToolOffer::dynamic([], false);
    let (tool, cancel) = ("propose_cancel", json!({"run_query": "hstar"}));
    let ctx = world.context(&json!({"author_id": "22", "channel_id": "700"}));

    let refused = world
        .dispatch(&ctx, &mut offer, &mut session, tool, &cancel)
        .await;
    assert_eq!(refused.outcome.error, Some(REFUSED));
    assert!(
        refused.outcome.output.starts_with("propose_cancel is not available for this message. If they asked for that, call request_tools with bundle 'run_changes' first."),
        "{}",
        refused.outcome.output
    );
    assert_eq!(
        proposals(&world).await,
        0,
        "an unoffered call is never executed"
    );

    let asked = world
        .dispatch(
            &ctx,
            &mut offer,
            &mut session,
            "request_tools",
            &json!({"bundle": "run_changes"}),
        )
        .await;
    assert!(asked.outcome.ok, "{}", asked.outcome.output);
    assert_eq!(
        asked.requested,
        Some(Bundle::RunWrites),
        "the loop charges one round"
    );
    // Offered from the next round, not the one that asked.
    assert!(offer.requested() && !offer.offers(ToolName::ProposeCancel));
    offer.begin_round();
    assert!(offer.offers(ToolName::ProposeCancel));

    let again = world
        .dispatch(
            &ctx,
            &mut offer,
            &mut session,
            "request_tools",
            &json!({"bundle": "strategy"}),
        )
        .await;
    assert_eq!(
        (again.outcome.error, again.requested),
        (Some(REFUSED), None)
    );
    assert!(!offer.offers(ToolName::GetBossStrategy));

    let posted = world
        .dispatch(&ctx, &mut offer, &mut session, tool, &cancel)
        .await;
    assert!(posted.outcome.ok, "{}", posted.outcome.output);
    assert_eq!(posted.outcome.cards.len(), 1);
    assert_eq!(proposals(&world).await, 1);

    // Offered or not, authority is still the dispatcher's.
    let outsider = world.context(&json!({"author_id": "33", "channel_id": "700"}));
    let denied = world
        .dispatch(&outsider, &mut offer, &mut session, tool, &cancel)
        .await;
    assert_eq!(denied.outcome.error, Some(REFUSED));
    assert!(
        denied
            .outcome
            .output
            .starts_with("They are not on run a1a1a1a1")
    );
    assert_eq!(proposals(&world).await, 1);
}

#[tokio::test]
async fn read_only_turns_refuse_writes_and_write_bundles() {
    let mut world = world().await;
    let mut session = PassthroughSession;
    let mut offer = ToolOffer::dynamic([Bundle::RunWrites], true);
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "700", "read_only": true}));
    let asked = world
        .dispatch(
            &ctx,
            &mut offer,
            &mut session,
            "request_tools",
            &json!({"bundle": "run_changes"}),
        )
        .await;
    assert_eq!(asked.outcome.output, READ_ONLY_TURN);
    let write = world
        .dispatch(
            &ctx,
            &mut offer,
            &mut session,
            "propose_cancel",
            &json!({"run_query": "hstar"}),
        )
        .await;
    assert_eq!(
        (write.outcome.output.as_str(), write.outcome.error),
        (READ_ONLY_TURN, Some(REFUSED))
    );
    let bad = world
        .dispatch(
            &ctx,
            &mut offer,
            &mut session,
            "request_tools",
            &json!({"bundle": "everything"}),
        )
        .await;
    assert_eq!(
        bad.outcome.output,
        "bundle must be one of: strategy, run_changes, weekly_changes."
    );
    assert_eq!(proposals(&world).await, 0);
}

#[tokio::test]
async fn unknown_tools_list_what_was_offered_and_full_set_has_no_hatch() {
    let mut world = world().await;
    let mut session = PassthroughSession;
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "700"}));
    let mut dynamic = ToolOffer::dynamic([], false);
    let unknown = world
        .dispatch(&ctx, &mut dynamic, &mut session, "delete_run", &json!({}))
        .await;
    assert_eq!(unknown.outcome.error, Some(UNKNOWN));
    assert_eq!(
        unknown.outcome.output,
        "There is no tool called delete_run. The tools you have are: get_schedule, get_run, list_bosses, get_pending, list_fixed, request_tools. Use one of those when it can answer the request."
    );
    let mut full = ToolOffer::full_set(false);
    let hatch = world
        .dispatch(
            &ctx,
            &mut full,
            &mut session,
            "request_tools",
            &json!({"bundle": "strategy"}),
        )
        .await;
    assert_eq!(
        hatch.outcome.error,
        Some(UNKNOWN),
        "v4 has no request_tools"
    );
}
