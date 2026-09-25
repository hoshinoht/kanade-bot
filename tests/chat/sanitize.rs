//! `sanitize.json`: note defusing, member-facing rewrites, false-claim
//! stripping, trusted defaults, tidy bounds and schedule regrounding.

use kanade::chat::sanitize::{
    defuse_notes, ground_schedule_reply, looks_like_clarification, member_facing,
    schedule_defaults, shape_reply, strip_false_card_claim, tidy,
};
use kanade::chat::tools::ToolOutcome;
use serde_json::{Map, Value, json};

use crate::common::text;
use crate::support::{check_family, unknown_op, value};

/// The vectors' outcome descriptors as tool outcomes.
fn outcomes(raw: &Value) -> Vec<ToolOutcome> {
    raw.as_array()
        .expect("outcomes")
        .iter()
        .map(|item| {
            assert_eq!(item["posted"], json!([]), "sanitize outcomes post nothing");
            ToolOutcome {
                name: text(&item["name"]).to_owned(),
                output: text(&item["output"]).to_owned(),
                arguments: Map::new(),
                ok: item["ok"].as_bool().expect("ok"),
                error: match item["error"].as_str() {
                    None => None,
                    Some("refused") => Some(kanade::chat::tools::REFUSED),
                    Some(other) => panic!("unexpected outcome error {other}"),
                },
                created: Vec::new(),
                cards: Vec::new(),
                detail: None,
            }
        })
        .collect()
}

fn replay(case: &Value) -> Vec<Value> {
    case["input"]["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| {
            let input = || text(&step["text"]);
            value(match text(&step["op"]) {
                "defuse_notes" => json!(defuse_notes(input())),
                "member_facing" => json!(member_facing(input())),
                "strip_false_card_claim" => json!(strip_false_card_claim(input())),
                "looks_like_clarification" => json!(looks_like_clarification(input())),
                "schedule_defaults" => {
                    let found = schedule_defaults(
                        input(),
                        step["bot_user_id"].as_str(),
                        step["self_role_id"].as_str(),
                    );
                    json!({
                        "force_all_channels": found.force_all_channels,
                        "force_channel_scope": found.force_channel_scope,
                        "force_group_schedule": found.force_group_schedule,
                        "upcoming_only": found.upcoming_only,
                    })
                }
                "tidy" => json!(tidy(input(), step["protected"].as_str())),
                "ground_schedule_reply" => json!(ground_schedule_reply(
                    text(&step["reply"]),
                    &outcomes(&step["outcomes"])
                )),
                "shape_reply" => json!(shape_reply(
                    text(&step["reply"]),
                    &outcomes(&step["outcomes"])
                )),
                other => unknown_op("sanitize", other),
            })
        })
        .collect()
}

#[tokio::test]
async fn the_sanitize_family_replays_exactly() {
    let counts = check_family("sanitize", &[], |case| async move { replay(&case) }).await;
    assert_eq!(counts, (6, 64));
}
