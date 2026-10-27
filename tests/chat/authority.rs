//! The authority boundary and the party validation shared by the proposal
//! tools, on the synthetic chat world.

use kanade_chat::authority::{Asker, Subject, require_authority};
use kanade_chat::tools::read::participants::validate_participants;
use serde_json::json;

use crate::support::world_input;
use crate::world::World;

#[tokio::test]
async fn only_the_party_or_an_admin_may_change_a_run_from_its_channel() {
    let world = World::new(&world_input()).await;
    let snapshot = world.snapshot().await;
    let run = |id: &str| Subject::Run(snapshot.runs.iter().find(|run| run.id == id).unwrap());
    let check = |author_id: &str, channel_id: &str, is_admin: bool, subject| {
        let asker = Asker {
            author_id,
            channel_id,
            is_admin,
            channels: &world.channels,
            pilot: &world.pilot,
        };
        require_authority(asker, subject, &snapshot).map_err(|refusal| refusal.0)
    };
    let star = "a1a1a1a1-0000-4000-8000-000000000001";
    assert_eq!(check("11", "900", false, run(star)), Ok(()));
    assert_eq!(
        check("33", "700", false, run(star)),
        Err("They are not on run a1a1a1a1 and do not own the weekly timing behind it, so it is not theirs to change. Say that only the people on a run -- or the owner of the weekly timing it comes from -- can propose a change to it, and that putting somebody on a run is not something you can do. Do not name anybody on it.".into())
    );
    assert_eq!(check("33", "701", true, run(star)), Ok(()));
    let elsewhere = "c3c3c3c3-0000-4000-8000-000000000003";
    assert_eq!(
        check("33", "700", false, run(elsewhere)),
        Err("That run lives in <#702>, and changes to it are proposed from its own channel. Tell them which channel it lives in and to ask there. Say nothing else about it.".into())
    );
    assert_eq!(check("33", "702", false, run(elsewhere)), Ok(()));
}

#[tokio::test]
async fn a_party_defaults_to_the_asker_and_reads_names_and_mentions() {
    let world = World::new(&world_input()).await;
    let snapshot = world.snapshot().await;
    let tools = world.tool_world(&snapshot);
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "700"}));
    let party = |text: serde_json::Value| {
        validate_participants(&tools, &ctx, Some(&text)).map_err(|refusal| refusal.0)
    };
    assert_eq!(party(json!("")), Ok(vec!["11".to_owned()]));
    assert_eq!(
        party(json!("me and kanon")),
        Ok(vec!["11".into(), "22".into()])
    );
    assert_eq!(
        party(json!("me, kanon & Priya please")),
        Ok(vec!["11".into(), "22".into(), "33".into()])
    );
    assert_eq!(
        party(json!(["me", "kanon"])),
        Ok(vec!["11".into(), "22".into()])
    );
    assert_eq!(party(json!("<@!33>")), Ok(vec!["33".into()]));
}
