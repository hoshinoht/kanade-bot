//! Every tool argument is decoded and every tool result encoded through the
//! conversation's identity session: a `TaggingCodec` capture proves no
//! roster name or id reaches the model through a tool round, and that
//! tagged arguments reach the tools as real ids.

use kanade::chat::tools::REFUSED;
use kanade::chat::tools::bundles::ToolOffer;
use kanade::chat::tools::dispatch::UNKNOWN_IDENTITY;
use kanade::infrastructure::llm::identity::{IdentityCodec, Member, TaggingCodec, find_leaks};
use serde_json::json;

use crate::support::load;
use crate::world::World;

fn roster(world: &World) -> Vec<Member> {
    world
        .members()
        .iter()
        .map(|member| Member {
            user_id: member.user_id.clone(),
            display_name: member.display_name.clone().unwrap_or_default(),
            nickname: member.nickname.clone(),
            aliases: Vec::new(),
        })
        .collect()
}

async fn world() -> World {
    World::new(&load("read_tools.json")["cases"][0]["input"]).await
}

#[tokio::test]
async fn tool_results_reach_the_model_encoded() {
    let mut world = world().await;
    let roster = roster(&world);
    let mut session = TaggingCodec.open(&roster);
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "700"}));
    let mut offer = ToolOffer::full_set(false);
    for (tool, arguments) in [
        ("get_run", json!({"query": "hstar"})),
        ("list_fixed", json!({})),
        ("get_schedule", json!({"week": "this"})),
        ("propose_cancel", json!({"run_query": "hbaldrix"})),
    ] {
        let dispatched = world
            .dispatch(&ctx, &mut offer, session.as_mut(), tool, &arguments)
            .await;
        assert!(
            dispatched.outcome.ok,
            "{tool}: {}",
            dispatched.outcome.output
        );
        assert!(
            !find_leaks(&dispatched.outcome.output, &roster).is_empty(),
            "{tool}: the plain output names members"
        );
        assert_eq!(
            find_leaks(&dispatched.model_content, &roster),
            Vec::<String>::new(),
            "{tool}: {}",
            dispatched.model_content
        );
    }
}

#[tokio::test]
async fn tagged_arguments_reach_the_tools_as_ids() {
    let mut world = world().await;
    let roster = roster(&world);
    let mut session = TaggingCodec.open(&roster);
    let kanon = session.mention("22");
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "700"}));
    let mut offer = ToolOffer::full_set(false);
    // As a JSON object and as the wire's JSON text.
    for arguments in [
        json!({"participant": kanon}),
        json!(json!({"participant": kanon}).to_string()),
    ] {
        let dispatched = world
            .dispatch(
                &ctx,
                &mut offer,
                session.as_mut(),
                "get_schedule",
                &arguments,
            )
            .await;
        assert_eq!(dispatched.outcome.arguments["participant"], "<@22>");
        assert!(
            dispatched
                .outcome
                .output
                .starts_with("**kanon's 2 runs this week"),
            "{}",
            dispatched.outcome.output
        );
        assert!(
            dispatched
                .model_content
                .starts_with("**ID1's 2 runs this week")
        );
    }
    let unknown = world
        .dispatch(
            &ctx,
            &mut offer,
            session.as_mut(),
            "get_schedule",
            &json!({"participant": "<@ID9>"}),
        )
        .await;
    assert_eq!(
        (unknown.outcome.output.as_str(), unknown.outcome.error),
        (UNKNOWN_IDENTITY, Some(REFUSED))
    );
    assert!(unknown.outcome.arguments.is_empty(), "nothing ran");
}
