//! What is registered, where, and how redelivered interactions are handled.

use serde_json::{Value, json};

use kanade::bot::commands::Disposition;
use kanade::bot::transport::{Call, DiscordTransport, Outcome};
use kanade::domain::history::ChangeHistory;

use super::super::support::{ADMIN_ROLE, BOSSING_ROLE, guild};
use super::{ALICE, KALOS, R_KALOS, Slash, opt};

const RETAINED: [&str; 13] = [
    "fixed", "debug", "schedule", "amend", "swap", "status", "rsvp", "nick", "pings", "style",
    "limits", "rescan", "say",
];

fn names(options: &Value) -> Vec<String> {
    options
        .as_array()
        .map(|options| {
            options
                .iter()
                .map(|option| option["name"].as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn only_the_retained_commands_are_registered_for_the_guild() {
    let slash = Slash::new().await;
    let definitions = slash.dispatcher.definitions();
    let payload: Vec<Value> = definitions
        .iter()
        .map(|command| serde_json::to_value(command).unwrap())
        .collect();
    let registered: Vec<String> = payload
        .iter()
        .map(|command| command["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(registered, RETAINED);
    for dropped in ["cancel", "otot", "done", "restore", "bot", "pingtime"] {
        assert!(!registered.iter().any(|name| name == dropped), "{dropped}");
    }
    for command in &payload {
        assert_eq!(command["contexts"], json!([0]), "{}", command["name"]);
        assert!(command.get("guild_id").is_none());
        let hidden = command.get("default_member_permissions") == Some(&json!("8"));
        let staff = ["debug", "nick", "say"].contains(&command["name"].as_str().unwrap());
        assert_eq!(hidden, staff, "{}", command["name"]);
    }
    let debug = &payload[1];
    assert_eq!(
        names(&debug["options"]),
        [
            "ping",
            "clear_test",
            "reminders",
            "materialise",
            "header",
            "rewrite"
        ]
    );
    let ping = &debug["options"][0]["options"];
    assert_eq!(
        names(ping),
        [
            "kind", "run_id", "channel", "style", "header", "bosses", "time", "party", "in", "out",
            "status"
        ]
    );
    assert_eq!(ping[0]["required"], json!(true));
    for option in ping.as_array().unwrap().iter().skip(1) {
        assert!(option.get("required").is_none(), "{option}");
    }
    assert_eq!(ping[6]["min_value"], json!(-1440));
    assert_eq!(
        names(&debug["options"][4]["options"]),
        ["kind", "tries", "channel"]
    );
    // Discord caps a command's names, descriptions and choice values at 4000 characters.
    fn text_size(value: &Value) -> usize {
        match value {
            Value::Object(map) => map
                .iter()
                .map(|(key, value)| match (key.as_str(), value) {
                    ("name" | "description" | "value", Value::String(text)) => text.chars().count(),
                    _ => text_size(value),
                })
                .sum(),
            Value::Array(items) => items.iter().map(text_size).sum(),
            _ => 0,
        }
    }
    assert!(text_size(debug) <= 4000, "{}", text_size(debug));
    let fixed = &payload[0];
    assert_eq!(names(&fixed["options"]), ["add", "list", "edit", "remove"]);

    // Registration is a guild bulk overwrite; there is no global path.
    assert_eq!(
        slash
            .discord
            .register_guild_commands(guild(), &definitions)
            .await,
        Outcome::Delivered(())
    );
    assert!(matches!(
        slash.discord.calls().as_slice(),
        [Call::Register { guild: g, commands, .. }] if *g == guild() && commands.len() == 13
    ));
}

#[tokio::test]
async fn status_offers_own_time_by_name() {
    let slash = Slash::new().await;
    let status = serde_json::to_value(&slash.dispatcher.definitions()[5]).unwrap();
    let state = &status["options"][1];
    assert_eq!(state["name"], "state");
    assert_eq!(
        state["choices"],
        json!([
            { "name": "planned", "value": "planned" },
            { "name": "confirmed", "value": "confirmed" },
            { "name": "own time", "value": "otot" },
            { "name": "done", "value": "done" },
            { "name": "cancelled", "value": "cancelled" },
        ])
    );
    assert_eq!(status["options"][0]["autocomplete"], true);
}

#[tokio::test]
async fn a_redelivered_interaction_is_answered_and_applied_once() {
    let slash = Slash::new().await;
    let interaction = slash.interaction(
        2,
        ALICE,
        &[BOSSING_ROLE],
        KALOS,
        "status",
        json!([opt("run_id", R_KALOS), opt("state", "cancelled")]),
    );
    let head = slash.store.history_head().await.unwrap().seq;
    let (disposition, _) = slash.send(&interaction).await;
    assert_eq!(disposition, Disposition::Ran);
    let calls = slash.discord.calls().len();
    let again = slash
        .dispatcher
        .handle(slash.discord.as_ref(), guild(), &interaction, None)
        .await;
    assert!(matches!(again, Some((Disposition::Duplicate, _))));
    assert_eq!(slash.discord.calls().len(), calls, "nothing sent twice");
    assert_eq!(slash.store.history_head().await.unwrap().seq, head + 1);
}

#[tokio::test]
async fn refusals_are_ephemeral_and_v4_worded() {
    let slash = Slash::new().await;
    let reply = slash.run_in(ALICE, &[], KALOS, "schedule", json!([])).await;
    assert!(reply.ephemeral);
    assert_eq!(
        reply.content,
        "❌ You need the bossing role to use this bot."
    );
    for (name, command) in [("say", "say"), ("nick", "nick")] {
        let reply = slash
            .run_in(ALICE, &[BOSSING_ROLE], KALOS, name, json!([]))
            .await;
        assert_eq!(
            reply.content,
            format!("❌ `/{command}` is for server admins, the server owner and the admin role.")
        );
    }
    let reply = slash
        .run_in(
            ALICE,
            &[BOSSING_ROLE],
            KALOS,
            "debug",
            super::sub("clear_test", json!([])),
        )
        .await;
    assert_eq!(
        reply.content,
        "❌ `/debug` is restricted to the server owner, the admin role, and users listed in \
         `DEBUG_USER_IDS`."
    );
    // The admin role is staff for `/nick`, but still needs the bossing role
    // for scheduling commands (v4: no staff bypass).
    let reply = slash
        .run_in(ALICE, &[ADMIN_ROLE], KALOS, "schedule", json!([]))
        .await;
    assert_eq!(
        reply.content,
        "❌ You need the bossing role to use this bot."
    );
}
