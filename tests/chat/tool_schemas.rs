//! `tool_schemas.json`: the full-set surface is v4's bytes, and every loop,
//! context and reply constant is the crate's own.

use kanade::chat::context::{
    ANCHOR_CACHE, COMPLETION_RESERVE_TOKENS, CONVERSATION_BUDGET_TOKENS, HISTORY_EXCHANGES,
    REFERENCE_CACHE, REPLY_CHAIN_DEPTH,
};
use kanade::chat::gate::{
    CHANNEL_BUSY_REACTION, POOL_SPENT, POOL_SPENT_REPLY, RATE_LIMITED_REACTION, RATE_LIMITED_REPLY,
    SEEN_REACTION,
};
use kanade::chat::sanitize::{FAILURE_REPLY, SPOOFED_NOTE, STRATEGY_GROUNDING_FAILURE_REPLY};
use kanade::chat::tools::bundles::{DEFAULT_TOOL_ROUNDS, ToolOffer, V4_MAX_TOOL_ROUNDS};
use kanade::chat::tools::{MAX_MEMBER_REPLY, MAX_RUNS, READ_ONLY_TURN, ToolName, UNKNOWN_TOOL};
use serde_json::{Value, json};

use crate::common::text;
use crate::support::{Named, check_family, dev, unknown_op, value};

fn surface(read_only: bool) -> Value {
    let offer = ToolOffer::full_set(read_only);
    let tools = offer.tools();
    let text = offer.surface_text();
    json!({
        "names": offer.names(),
        "text": text,
        "tokens": offer.estimated_tokens(),
        "tools": serde_json::from_str::<Value>(&text).expect("surface JSON"),
        "write_names": tools.iter().filter(|t| t.is_write()).map(|t| t.as_str()).collect::<Vec<_>>(),
    })
}

fn constants() -> Value {
    json!({
        "anchor_cache": ANCHOR_CACHE,
        "completion_reserve_tokens": COMPLETION_RESERVE_TOKENS,
        "conversation_budget_tokens": CONVERSATION_BUDGET_TOKENS,
        "failure_reply": FAILURE_REPLY,
        "history_exchanges": HISTORY_EXCHANGES,
        "max_member_reply": MAX_MEMBER_REPLY,
        "max_runs": MAX_RUNS,
        "max_tool_rounds": DEFAULT_TOOL_ROUNDS,
        "pool_spent_reason": POOL_SPENT,
        "pool_spent_reply": POOL_SPENT_REPLY,
        "rate_limited_reply": RATE_LIMITED_REPLY,
        "reactions": {
            "channel_busy": CHANNEL_BUSY_REACTION,
            "rate_limited": RATE_LIMITED_REACTION,
            "seen": SEEN_REACTION,
        },
        "read_only_turn": READ_ONLY_TURN,
        "reference_cache": REFERENCE_CACHE,
        "reply_chain_depth": REPLY_CHAIN_DEPTH,
        "spoofed_note": SPOOFED_NOTE,
        "strategy_grounding_failure_reply": STRATEGY_GROUNDING_FAILURE_REPLY,
        "unknown_tool": UNKNOWN_TOOL,
    })
}

fn named() -> Vec<Named> {
    vec![Named {
        // User decision 2026-09-25: 8 tool rounds by default (v4: 4).
        name: "D-TOOL-ROUNDS",
        entries: vec![dev(
            "surface",
            "/steps/2/value/max_tool_rounds",
            json!(V4_MAX_TOOL_ROUNDS),
            json!(DEFAULT_TOOL_ROUNDS),
        )],
    }]
}

async fn replay(case: Value) -> Vec<Value> {
    case["input"]["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| match text(&step["op"]) {
            "surface" => value(surface(step["read_only"].as_bool().expect("read_only"))),
            "constants" => value(constants()),
            other => unknown_op("tool_schemas", other),
        })
        .collect()
}

#[tokio::test]
async fn the_tool_schemas_family_replays_exactly() {
    assert_eq!(check_family("tool_schemas", &named(), replay).await, (1, 3));
}

#[test]
fn the_full_set_is_v4s_twelve_in_order() {
    let names: Vec<&str> = ToolName::V4.iter().map(|tool| tool.as_str()).collect();
    assert_eq!(ToolOffer::full_set(false).names(), names);
    assert!(!ToolOffer::full_set(false).offers(ToolName::RequestTools));
}
