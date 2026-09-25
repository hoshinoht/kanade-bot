//! `read_tools.json`: every read tool and the dispatcher boundary, through
//! `dispatch::run` in the full-set (v4) mode with a passthrough identity
//! session.

use kanade::chat::tools::ToolOutcome;
use kanade::infrastructure::llm::identity::PassthroughSession;
use serde_json::{Value, json};

use crate::common::{instant, text};
use crate::support::{check_family, unknown_op, value};
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
