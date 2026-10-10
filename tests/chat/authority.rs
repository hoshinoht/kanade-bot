//! `authority.json` and `participants.json`: the authority boundary and the
//! party/boss validation shared by the proposal tools.

use kanade::chat::authority::{Asker, Subject, require_authority};
use kanade::chat::tools::read::participants::{
    is_true, new_party, validate_bosses, validate_participants,
};
use serde_json::{Value, json};

use crate::common::text;
use crate::support::{check_family, error, unknown_op, value};
use crate::world::World;

async fn authority(case: Value) -> Vec<Value> {
    let input = &case["input"];
    let world = World::new(input).await;
    let snapshot = world.snapshot().await;
    let mut out = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        assert_eq!(
            text(&step["op"]),
            "require_authority",
            "unknown authority op"
        );
        let asker = Asker {
            author_id: text(&step["author_id"]),
            channel_id: text(&step["channel_id"]),
            is_admin: step["is_admin"].as_bool().expect("is_admin"),
            channels: &world.channels,
            pilot: &world.pilot,
        };
        let subject = match (step.get("run_id"), step.get("fixed_id")) {
            (Some(run), _) => Subject::Run(
                snapshot
                    .runs
                    .iter()
                    .find(|row| row.id == text(run))
                    .expect("run"),
            ),
            (_, Some(fixed)) => Subject::Fixed(
                snapshot
                    .fixed_runs
                    .iter()
                    .find(|row| row.id == text(fixed))
                    .expect("fixed"),
            ),
            _ => panic!("require_authority names nothing"),
        };
        out.push(match require_authority(asker, subject, &snapshot) {
            Ok(()) => value(json!({ "allowed": true })),
            Err(refusal) => error("ToolError", refusal.0),
        });
    }
    out
}

async fn participants(case: Value) -> Vec<Value> {
    let input = &case["input"];
    let world = World::new(input).await;
    let snapshot = world.snapshot().await;
    let tools = world.tool_world(&snapshot);
    let mut out = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        let context = |step: &Value| {
            world.context(&json!({"author_id": step["author_id"], "channel_id": "700"}))
        };
        let result = match text(&step["op"]) {
            "validate_participants" => {
                validate_participants(&tools, &context(step), step.get("text"))
                    .map(|ids| json!(ids))
            }
            "new_party" => {
                new_party(&tools, &context(step), step.get("value")).map(|ids| json!(ids))
            }
            "validate_bosses" => validate_bosses(&tools, text(&step["text"])).map(|ids| json!(ids)),
            "is_true" => Ok(json!(is_true(step.get("value")))),
            other => unknown_op("participants", other),
        };
        out.push(match result {
            Ok(result) => value(result),
            Err(refusal) => error("ToolError", refusal.0),
        });
    }
    out
}

#[tokio::test]
async fn the_authority_family_replays_exactly() {
    assert_eq!(check_family("authority", &[], authority).await, (3, 24));
}

#[tokio::test]
async fn the_participants_family_replays_exactly() {
    assert_eq!(
        check_family("participants", &[], participants).await,
        (6, 66)
    );
}
