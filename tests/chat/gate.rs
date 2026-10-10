//! `gate.json`: refusal order, channel allow-list, resolved mentions,
//! budgets (read before spend, admin exempt), admin rule and retry notes.

use kanade::chat::gate::{
    Author, Budgets, ChannelInfo, ChatDecision, IncomingMessage, RateLimiter, Summons,
    access_decide, decide, is_bot_admin, is_chat_channel, mentions_bot, retry_note,
    would_check_mention,
};
use serde_json::{Value, json};

use crate::common::{strings, text};
use crate::support::{check_family, unknown_op, value};
use crate::world::{Channels, pilot};

fn opt(value: &Value) -> Option<&str> {
    value.as_str()
}

/// The v4 family's `_channel`: an unlisted id is a bare channel.
fn channel(channels: &Channels, id: &Value) -> Option<ChannelInfo> {
    use kanade::chat::gate::ChannelDirectory;
    let id = id.as_str()?;
    Some(
        channels
            .channel(id)
            .unwrap_or_else(|| ChannelInfo::bare(id)),
    )
}

fn message(channels: &Channels, raw: &Value) -> IncomingMessage {
    IncomingMessage {
        author: (!raw["author"].is_null()).then(|| Author {
            id: text(&raw["author"]["id"]).to_owned(),
            bot: raw["author"]["bot"].as_bool().expect("bot"),
            roles: strings(&raw["author"]["roles"]),
        }),
        guild_id: opt(&raw["guild_id"]).map(str::to_owned),
        channel: channel(channels, &raw["channel_id"]),
        mentions: strings(&raw["mentions"]),
        role_mentions: strings(&raw["role_mentions"]),
    }
}

fn decision(value: &ChatDecision) -> Value {
    json!({
        "act": value.act,
        "reason": value.reason,
        "busy": value.busy,
        "retry_after_s": value.retry_after_s,
    })
}

fn summons<'a>(bot: Option<&'a str>, role: Option<&'a str>, step: &'a Value) -> Summons<'a> {
    Summons {
        bot_user_id: bot,
        self_role_id: role,
        replied_author_id: step["replied_author_id"].as_str(),
        enabled: step["enabled"].as_bool().expect("enabled"),
        is_admin: step["is_admin"].as_bool().expect("is_admin"),
    }
}

fn replay(case: &Value) -> Vec<Value> {
    let input = &case["input"];
    let channels = Channels::new(&input["channels"]);
    let settings = pilot(&input["settings"]);
    let limits = &input["limits"];
    let count = |name: &str| usize::try_from(limits[name].as_u64().expect("count")).expect("usize");
    let seconds = |name: &str| limits[name].as_f64().expect("window");
    let mut person = RateLimiter::new(count("count"), seconds("window_s"));
    let mut pool = RateLimiter::new(count("pool_count"), seconds("pool_window_s"));
    let bot = opt(&input["bot_user_id"]);
    let role = opt(&input["self_role_id"]);
    let mut out = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        let result = match text(&step["op"]) {
            "access_decide" => decision(&access_decide(
                &message(&channels, &step["message"]),
                &settings,
                &channels,
                summons(bot, role, step),
            )),
            "decide" => decision(&decide(
                &message(&channels, &step["message"]),
                &settings,
                &channels,
                summons(bot, role, step),
                Budgets {
                    person: Some(&mut person),
                    pool: Some(&mut pool),
                    now: step["now"].as_f64().expect("now"),
                },
            )),
            "is_chat_channel" => json!(is_chat_channel(
                channel(&channels, &step["channel_id"]).as_ref(),
                &channels,
                &settings,
            )),
            "mentions_bot" => json!(mentions_bot(
                &message(&channels, &step["message"]),
                opt(&step["bot_user_id"]),
                opt(&step["self_role_id"]),
                opt(&step["replied_author_id"]),
            )),
            "would_check_mention" => json!(would_check_mention(
                &message(&channels, &step["message"]),
                &settings,
                &channels,
                bot,
                step["enabled"].as_bool().expect("enabled"),
            )),
            "is_bot_admin" => json!(is_bot_admin(
                step["administrator"].as_bool().expect("administrator"),
                step["owner"].as_bool().expect("owner"),
                &strings(&step["roles"]),
                opt(&step["admin_role_id"]),
            )),
            "retry_note" => json!(retry_note(step["seconds"].as_f64().expect("seconds"))),
            other => unknown_op("gate", other),
        };
        out.push(value(result));
    }
    out
}

#[tokio::test]
async fn the_gate_family_replays_exactly() {
    let counts = check_family("gate", &[], |case| async move { replay(&case) }).await;
    assert_eq!(counts, (7, 72));
}
