//! `tool_schemas.json`: the full-set surface keeps v4's structure, with named
//! schema differences; every loop, context and reply constant is the crate's own.

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
use crate::support::{Named, check_family, dev, load, unknown_op, value};

const V4_LIST_BOSSES_DESCRIPTION: &str =
    "The bosses this guild runs, with their difficulties. Use it to check a name.";
const V5_LIST_BOSSES_DESCRIPTION: &str = "The bosses this guild runs, with their difficulties, plus seasonal event bosses that only have a guide. Use it to check a name or to say which guides exist.";
const V4_GET_BOSS_STRATEGY_DESCRIPTION: &str = "Source-backed local strategy notes for one boss. Use this for boss mechanics, phases, dangers, and strategy facts; it returns only checked-in guide content.";
const V5_GET_BOSS_STRATEGY_DESCRIPTION: &str = "Source-backed local strategy notes for one boss. Use this for boss mechanics, phases, dangers, and strategy facts; it returns only checked-in guide content. Also covers seasonal event bosses listed by list_bosses.";
/// D-SEASONAL-LIST's estimated-token cost on each full-set surface.
const SEASONAL_TOKENS: u64 = 49;
const V4_WEEK_DESCRIPTION: &str = "Use 'this' or 'next' for calendar Monday-Sunday weeks. Use 'this_boss' or 'next_boss' only when the member explicitly says boss week. Use 'auto' for a bare weekday or today, tonight, or tomorrow. For 'next week', set week to 'next' and omit day.";
/// D-AUTO-FORWARD (user decisions 2026-10-03): `auto` without a day reads the
/// upcoming runs left in this boss week.
const V5_WEEK_DESCRIPTION: &str = "Use 'this' or 'next' for calendar Monday-Sunday weeks. Use 'this_boss' or 'next_boss' only when the member explicitly says boss week. Use 'auto' for a bare weekday or today, tonight, or tomorrow. For 'next run' or 'when is my next' asks, use 'auto' and omit day: it lists the upcoming runs left in this boss week, earliest first. For 'next week', set week to 'next' and omit day.";

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

fn seasonal_schema_text(v4_text: &str) -> String {
    let mut text = v4_text.to_owned();
    for (v4, v5) in [
        (V4_LIST_BOSSES_DESCRIPTION, V5_LIST_BOSSES_DESCRIPTION),
        (
            V4_GET_BOSS_STRATEGY_DESCRIPTION,
            V5_GET_BOSS_STRATEGY_DESCRIPTION,
        ),
    ] {
        let old = format!("\"description\":\"{v4}\"");
        let new = format!("\"description\":\"{v5}\"");
        assert_eq!(text.matches(&old).count(), 1, "frozen schema text");
        text = text.replacen(&old, &new, 1);
    }
    text
}

fn auto_forward_text(seasonal_text: &str) -> String {
    let old = format!("\"description\":\"{V4_WEEK_DESCRIPTION}\"");
    let new = format!("\"description\":\"{V5_WEEK_DESCRIPTION}\"");
    assert_eq!(seasonal_text.matches(&old).count(), 1, "frozen schema text");
    seasonal_text.replacen(&old, &new, 1)
}

fn named() -> Vec<Named> {
    let vector = load("tool_schemas.json");
    let steps = vector["cases"][0]["expected"]["steps"].as_array().unwrap();
    let full = surface(false);
    let read_only = surface(true);
    let mut seasonal_entries = Vec::new();
    let mut forward_entries = Vec::new();
    for (step, actual) in [(0, &full), (1, &read_only)] {
        let v4_tokens = steps[step]["value"]["tokens"].as_u64().expect("tokens");
        let seasonal_tokens = v4_tokens + SEASONAL_TOKENS;
        seasonal_entries.extend([
            dev(
                "surface",
                format!("/steps/{step}/value/tools/2/function/description"),
                json!(V4_LIST_BOSSES_DESCRIPTION),
                json!(V5_LIST_BOSSES_DESCRIPTION),
            ),
            dev(
                "surface",
                format!("/steps/{step}/value/tools/3/function/description"),
                json!(V4_GET_BOSS_STRATEGY_DESCRIPTION),
                json!(V5_GET_BOSS_STRATEGY_DESCRIPTION),
            ),
            dev(
                "surface",
                format!("/steps/{step}/value/tokens"),
                json!(v4_tokens),
                json!(seasonal_tokens),
            ),
        ]);
        let v4_text = steps[step]["value"]["text"]
            .as_str()
            .expect("frozen tool schema text");
        let seasonal_text = seasonal_schema_text(v4_text);
        seasonal_entries.push(dev(
            "surface",
            format!("/steps/{step}/value/text"),
            json!(v4_text),
            json!(seasonal_text),
        ));
        forward_entries.extend([
            dev(
                "surface",
                format!(
                    "/steps/{step}/value/tools/0/function/parameters/properties/week/description"
                ),
                json!(V4_WEEK_DESCRIPTION),
                json!(V5_WEEK_DESCRIPTION),
            ),
            dev(
                "surface",
                format!("/steps/{step}/value/tokens"),
                json!(seasonal_tokens),
                actual["tokens"].clone(),
            ),
            dev(
                "surface",
                format!("/steps/{step}/value/text"),
                json!(seasonal_text),
                json!(auto_forward_text(&seasonal_text)),
            ),
        ]);
    }
    vec![
        Named {
            // User decision 2026-09-25: 8 tool rounds by default (v4: 4).
            name: "D-TOOL-ROUNDS",
            entries: vec![dev(
                "surface",
                "/steps/2/value/max_tool_rounds",
                json!(V4_MAX_TOOL_ROUNDS),
                json!(DEFAULT_TOOL_ROUNDS),
            )],
        },
        Named {
            name: "D-SEASONAL-LIST",
            entries: seasonal_entries,
        },
        Named {
            name: "D-AUTO-FORWARD",
            entries: forward_entries,
        },
    ]
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
