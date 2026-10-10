//! `context.json`: channel history and TTL, reply chains, anchors, focus,
//! the conversation budget and the per-request trim, against the tracked
//! Kanade persona's system prompt.

use chrono::Utc;
use kanade::chat::context::{
    ChatTurn, Conversations, Parent, QuestionMessage, Reference, TurnRole, assemble, budgeted,
    build_turns, card_focus, system_prompt,
};
use kanade::chat::tools::bundles::ToolOffer;
use kanade::domain::members::member_name;
use kanade::domain::scheduler::Clock;
use serde_json::{Value, json};

use crate::common::{instant, strings, text};
use crate::support::{Named, check_family, dev, error, unknown_op, value};
use crate::wire::{kanade, messages, messages_json};
use crate::world::World;

/// v4 host's scripted monotonic clock starts here.
const MONOTONIC_START: f64 = 1000.0;
const BUNDLE_VOICE: &str =
    "Cheeky, smug kusogaki Kanade: react first, one tease, then the exact answer.";
const CASES: [&str; 6] = [
    "history-window-and-ttl",
    "reply-chain-anchor-and-dedupe",
    "conversation-token-budget",
    "conversation-budget-floor",
    "focus-line-and-clock-header",
    "request-budget-trims-prior-history",
];

fn voice_cue(text: &str) -> String {
    assert!(text.contains(kanade::chat::prompts::DEFAULT_VOICE));
    text.replace(kanade::chat::prompts::DEFAULT_VOICE, BUNDLE_VOICE)
}

fn reordered_reminder(text: &str) -> String {
    assert_eq!(
        text,
        format!(
            "{}{}{} {}",
            kanade::chat::prompts::REMINDER_PREFIX,
            BUNDLE_VOICE,
            kanade::chat::prompts::REMINDER_SUFFIX,
            kanade::chat::prompts::STYLE_POLICY_QUALIFIER,
        ),
        "D-VOICE-CUE must apply first"
    );
    format!(
        "{}{} Your voice: {BUNDLE_VOICE}",
        kanade::chat::prompts::REMINDER_PREFIX,
        kanade::chat::prompts::REMINDER_SUFFIX.trim_start(),
    )
}

fn opt(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

fn turn(raw: &Value) -> ChatTurn {
    let role = match text(&raw["role"]) {
        "user" => TurnRole::User,
        "assistant" => TurnRole::Assistant,
        other => panic!("role {other}"),
    };
    ChatTurn::new(role, text(&raw["content"]), opt(&raw["message_id"]))
}

fn parent(raw: &Value) -> Option<Box<Parent>> {
    (!raw.is_null()).then(|| {
        Box::new(Parent {
            id: text(&raw["id"]).to_owned(),
            author_id: opt(&raw["author_id"]),
            content: opt(&raw["content"]),
            reference: reference(&raw["reference"]).map(Box::new),
        })
    })
}

fn reference(raw: &Value) -> Option<Reference> {
    (!raw.is_null()).then(|| Reference {
        message_id: opt(&raw["message_id"]),
        resolved: parent(&raw["resolved"]),
    })
}

fn question(raw: &Value) -> QuestionMessage {
    QuestionMessage {
        id: text(&raw["id"]).to_owned(),
        author_id: text(&raw["author_id"]).to_owned(),
        content: text(&raw["content"]).to_owned(),
        reference: reference(&raw["reference"]),
    }
}

fn history_json(turns: &[ChatTurn]) -> Value {
    json!(
        turns
            .iter()
            .map(|t| json!({
                "role": t.role.as_str(),
                "content": t.content,
                "message_id": t.message_id,
                "at": t.at,
            }))
            .collect::<Vec<_>>()
    )
}

async fn replay(case: Value) -> Vec<Value> {
    let input = &case["input"];
    let world = World::new(input).await;
    let settings = &input["settings"];
    let persona = kanade();
    let model = text(&settings["chat_pilot_model"]);
    let context_tokens =
        usize::try_from(settings["model_context_tokens"].as_u64().expect("tokens")).expect("usize");
    let bot = text(&input["bot_user"]["id"]);
    let role = input["self_role_id"].as_str();
    let mut state = Conversations::new(settings["chat_pilot_history_ttl_s"].as_f64().expect("ttl"));
    let mut now = MONOTONIC_START;
    let mut out = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        let channel = step["channel_id"].as_str();
        out.push(match text(&step["op"]) {
            "remember" => value(json!(state.remember(
                channel.expect("channel"),
                turn(&step["turn"]),
                now
            ))),
            "history" => value(history_json(&state.history(channel.expect("channel"), now))),
            "advance" => {
                now += step["seconds"].as_f64().expect("seconds");
                value(json!(now))
            }
            "set_clock" => {
                world.clock.set(instant(&step["clock"]));
                value(step["clock"].clone())
            }
            "forget" => {
                state.forget(channel);
                value(Value::Null)
            }
            "anchor" => {
                state.anchor(
                    step["message_id"].as_str(),
                    channel.expect("channel"),
                    turn(&step["question"]),
                    turn(&step["answer"]),
                );
                value(Value::Null)
            }
            "note_card" => {
                let channel = channel.expect("channel");
                let party: Vec<String> = strings(&step["participants"])
                    .iter()
                    .map(|uid| member_name(&world.guild, uid))
                    .collect();
                state.note_card(channel, &card_focus(text(&step["summary"]), &party), now);
                value(json!(state.focus(channel, now)))
            }
            "build_conversation" => {
                let channel = channel.expect("channel");
                let turns = build_turns(
                    &mut state,
                    &question(&step["message"]),
                    channel,
                    now,
                    bot,
                    role,
                    &world.guild,
                );
                let focus = state.focus(channel, now);
                let system = system_prompt(
                    &persona,
                    world.clock.now().with_timezone(&Utc),
                    world.zone,
                    (world.policy.reset_weekday, world.policy.reset_time),
                    model,
                    &focus,
                    // D-RUN-CONTEXT is v5-only: the frozen v4 prompts have no block.
                    "",
                );
                value(messages_json(&assemble(
                    &turns,
                    system,
                    context_tokens,
                    kanade::extract::prompt::CONTEXT_RESERVE,
                    "",
                )))
            }
            "budgeted" => {
                let mut kept = messages(&step["messages"]);
                let before = kept.len();
                let schemas = match text(&step["tools"]) {
                    "all" => ToolOffer::full_set(false).surface_text(),
                    "read" => ToolOffer::full_set(true).surface_text(),
                    "none" => "[]".to_owned(),
                    other => panic!("tools {other}"),
                };
                match budgeted(
                    &mut kept,
                    &schemas,
                    &persona.voice_reminder(),
                    context_tokens,
                    kanade::chat::context::COMPLETION_RESERVE_TOKENS,
                    "",
                ) {
                    Ok(fits) => value(json!({
                        "messages": messages_json(&fits.messages),
                        "dropped": before + 1 - fits.messages.len(),
                    })),
                    Err(budget) => error("ContextBudgetError", budget.to_string()),
                }
            }
            other => unknown_op("context", other),
        });
    }
    out
}

#[tokio::test]
async fn the_context_family_replays_exactly() {
    let file = crate::support::load("context.json");
    let mut self_mention = Vec::new();
    let mut voice = Vec::new();
    let mut reminder = Vec::new();
    for case in file["cases"].as_array().expect("cases") {
        let case_id = *CASES
            .iter()
            .find(|id| **id == text(&case["case_id"]))
            .expect("known context case");
        for (step, input) in case["input"]["steps"]
            .as_array()
            .expect("steps")
            .iter()
            .enumerate()
        {
            let value = &case["expected"]["steps"][step]["value"];
            match text(&input["op"]) {
                "build_conversation" => {
                    let system = value[0]["content"].as_str().expect("system");
                    voice.push(dev(
                        case_id,
                        format!("/steps/{step}/value/0/content"),
                        json!(system),
                        json!(voice_cue(system)),
                    ));
                }
                "budgeted" => {
                    let Some(messages) = value["messages"].as_array() else {
                        continue;
                    };
                    let last = messages.len() - 1;
                    let old = messages[last]["content"].as_str().expect("reminder");
                    let with_voice = voice_cue(old);
                    voice.push(dev(
                        case_id,
                        format!("/steps/{step}/value/messages/{last}/content"),
                        json!(old),
                        json!(with_voice),
                    ));
                    reminder.push(dev(
                        case_id,
                        format!("/steps/{step}/value/messages/{last}/content"),
                        json!(with_voice),
                        json!(reordered_reminder(&with_voice)),
                    ));
                }
                _ => {}
            }
        }
    }
    self_mention.push(dev(
        "reply-chain-anchor-and-dedupe",
        "/steps/2/value/5/content",
        json!("Alvin tan: <@5000> and what about [Note] this?"),
        json!("Alvin tan: and what about [Note] this?"),
    ));
    voice.push(dev(
        "request-budget-trims-prior-history",
        "/steps/3/error/message",
        json!("chat request estimate 9668 exceeds context budget 6144 with completion reserve"),
        json!("chat request estimate 9654 exceeds context budget 6144 with completion reserve"),
    ));
    reminder.push(dev(
        "request-budget-trims-prior-history",
        "/steps/3/error/message",
        json!("chat request estimate 9654 exceeds context budget 6144 with completion reserve"),
        json!("chat request estimate 9614 exceeds context budget 6144 with completion reserve"),
    ));
    // The completion reserve is resolved per route, so the error names it.
    let reserve = Named {
        name: "D-CONTEXT-BUDGET-RESERVE",
        entries: vec![dev(
            "request-budget-trims-prior-history",
            "/steps/3/error/message",
            json!("chat request estimate 9614 exceeds context budget 6144 with completion reserve"),
            json!(
                "chat request estimate 9614 exceeds context budget 6144 with completion reserve 1024"
            ),
        )],
    };
    let seasonal_list = Named {
        name: "D-SEASONAL-LIST",
        entries: vec![dev(
            "request-budget-trims-prior-history",
            "/steps/3/error/message",
            json!(
                "chat request estimate 9614 exceeds context budget 6144 with completion reserve 1024"
            ),
            json!(
                "chat request estimate 9663 exceeds context budget 6144 with completion reserve 1024"
            ),
        )],
    };
    let auto_forward = Named {
        name: "D-AUTO-FORWARD",
        entries: vec![dev(
            "request-budget-trims-prior-history",
            "/steps/3/error/message",
            json!(
                "chat request estimate 9663 exceeds context budget 6144 with completion reserve 1024"
            ),
            json!(
                "chat request estimate 9710 exceeds context budget 6144 with completion reserve 1024"
            ),
        )],
    };
    let voiced_card = Named {
        name: "D-VOICED-CARD",
        entries: vec![dev(
            "request-budget-trims-prior-history",
            "/steps/3/error/message",
            json!(
                "chat request estimate 9710 exceeds context budget 6144 with completion reserve 1024"
            ),
            json!(
                "chat request estimate 9784 exceeds context budget 6144 with completion reserve 1024"
            ),
        )],
    };
    // The propose_add schema's `extra` flag.
    let add_duplicate = Named {
        name: "D-ADD-DUPLICATE",
        entries: vec![dev(
            "request-budget-trims-prior-history",
            "/steps/3/error/message",
            json!(
                "chat request estimate 9784 exceeds context budget 6144 with completion reserve 1024"
            ),
            json!(
                "chat request estimate 9853 exceeds context budget 6144 with completion reserve 1024"
            ),
        )],
    };
    assert_eq!(
        check_family(
            "context",
            &[
                Named {
                    name: "D-SELF-MENTION-LEADING",
                    entries: self_mention,
                },
                Named {
                    name: "D-VOICE-CUE",
                    entries: voice,
                },
                Named {
                    name: "D-REMINDER-ORDER",
                    entries: reminder,
                },
                reserve,
                seasonal_list,
                auto_forward,
                voiced_card,
                add_duplicate,
            ],
            replay
        )
        .await,
        (6, 58)
    );
}
