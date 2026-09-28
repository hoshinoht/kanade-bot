//! `read_tools.json`: every read tool and the dispatcher boundary, through
//! `dispatch::run` in the full-set (v4) mode with a passthrough identity
//! session.

use kanade::chat::sanitize::schedule_defaults;
use kanade::chat::tools::ToolOutcome;
use kanade::infrastructure::llm::identity::PassthroughSession;
use serde_json::{Value, json};

use crate::common::{instant, text};
use crate::support::{check_family, load, unknown_op, value};
use crate::world::World;

/// v4 `host.outcome`; `posted` is the cards the call handed over for posting.
pub fn outcome_json(outcome: &ToolOutcome) -> Value {
    json!({
        "name": outcome.name,
        "output": outcome.output,
        "arguments": outcome.arguments,
        "ok": outcome.ok,
        "error": outcome.error,
        "created": outcome.created,
        "posted": outcome.cards.iter().map(|card| card.proposal_id.as_str()).collect::<Vec<_>>(),
    })
}

async fn replay(case: Value) -> Vec<Value> {
    let input = &case["input"];
    let mut world = World::new(input).await;
    let mut session = PassthroughSession;
    let mut out = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        out.push(match text(&step["op"]) {
            "run" => value(outcome_json(&world.run_tool(step, &mut session).await)),
            "set_clock" => {
                world.clock.set(instant(&step["clock"]));
                value(step["clock"].clone())
            }
            other => unknown_op("read_tools", other),
        });
    }
    out
}

#[tokio::test]
async fn the_read_tools_family_replays_exactly() {
    assert_eq!(check_family("read_tools", &[], replay).await, (7, 74));
}

#[tokio::test]
async fn an_explicit_self_schedule_recovers_only_unrecognized_model_mentions() {
    let vectors = load("read_tools.json");
    let fixture = vectors["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["case_id"] == "schedule-scope-and-people")
        .unwrap();
    let mut world = World::new(&fixture["input"]).await;
    let mut session = PassthroughSession;
    let step = |participant: Option<&str>, source: &str, group: bool| {
        let mut arguments = json!({"week": "this"});
        if let Some(participant) = participant {
            arguments["participant"] = json!(participant);
        }
        json!({
            "op": "run", "tool": "get_schedule", "author_id": "22", "channel_id": "700",
            "arguments": arguments,
            "self_schedule_requested": schedule_defaults(source, Some("999"), None).self_schedule_requested,
            "force_group_schedule": group,
        })
    };
    let me = world
        .run_tool(
            &step(Some("me"), "what's for me today?", false),
            &mut session,
        )
        .await;
    assert!(me.ok);
    let invented = world
        .run_tool(
            &step(Some("<@123>"), "<@999> what's for me today?", false),
            &mut session,
        )
        .await;
    assert!(invented.ok);
    assert_eq!(invented.output, me.output);
    for bot_name in ["@Kanade", "kanade", "@kAnAdE"] {
        let copied_bot = world
            .run_tool(
                &step(Some(bot_name), "<@999> whats for me today?", false),
                &mut session,
            )
            .await;
        assert!(copied_bot.ok, "{bot_name}");
        assert_eq!(copied_bot.output, me.output, "{bot_name}");
    }
    for self_only in [
        "my schedule this week",
        "what's my schedule today?",
        "what is my schedule?",
        "what's on my schedule today?",
        "show me my schedule tomorrow",
    ] {
        let omitted = world
            .run_tool(&step(None, self_only, false), &mut session)
            .await;
        assert_eq!(omitted.output, me.output, "{self_only}");
        let unrecognized = world
            .run_tool(&step(Some("<@123>"), self_only, false), &mut session)
            .await;
        assert_eq!(unrecognized.output, me.output, "{self_only}");
    }

    let unknown = world
        .run_tool(
            &step(Some("<@123>"), "what's for Kanon today?", false),
            &mut session,
        )
        .await;
    assert!(!unknown.ok);
    assert_eq!(
        unknown.output,
        "That does not identify one person on the roster. Ask who they mean."
    );
    for unsafe_name in ["@Kanade", "@Kanade and Kanon", "@NotTheBot"] {
        let not_self = world
            .run_tool(
                &step(Some(unsafe_name), "what's for Kanon today?", false),
                &mut session,
            )
            .await;
        assert!(!not_self.ok, "{unsafe_name}");
    }
    let mixed_participant = world
        .run_tool(
            &step(Some("@Kanade and Kanon"), "what's for me today?", false),
            &mut session,
        )
        .await;
    assert!(!mixed_participant.ok);
    let other = world
        .run_tool(
            &step(Some("kanon"), "what's for me today?", false),
            &mut session,
        )
        .await;
    let other_without_self = world
        .run_tool(
            &step(Some("kanon"), "what's for Kanon today?", false),
            &mut session,
        )
        .await;
    assert!(other.ok);
    assert_eq!(other.output, other_without_self.output);
    let group = world
        .run_tool(&step(None, "what's for me today?", true), &mut session)
        .await;
    let all = world
        .run_tool(&step(None, "what's on today?", false), &mut session)
        .await;
    assert_eq!(group.output, all.output);
    for mixed in [
        "what's for me and Kanon today?",
        "my runs with Kanon today?",
        "what's for me, Kanon, today?",
        "what's my schedule and Kanon today?",
        "my schedule for Kanon today?",
        "my schedule <@5000> today?",
        "my schedule everyone today?",
        "what's for me + Kanon today?",
    ] {
        let missing = world
            .run_tool(&step(None, mixed, false), &mut session)
            .await;
        assert_eq!(missing.output, all.output, "{mixed}");
        let unrecognized = world
            .run_tool(&step(Some("<@123>"), mixed, false), &mut session)
            .await;
        assert!(!unrecognized.ok, "{mixed}");
        assert_eq!(unrecognized.output, unknown.output, "{mixed}");
    }
    let mut collision_input = fixture["input"].clone();
    collision_input["bot_user"]["name"] = json!("Kanon");
    let mut collision_world = World::new(&collision_input).await;
    let recognized_member = collision_world
        .run_tool(
            &step(Some("@Kanon"), "what's for me today?", false),
            &mut session,
        )
        .await;
    assert!(recognized_member.ok);
    assert_eq!(recognized_member.output, other.output);
}
