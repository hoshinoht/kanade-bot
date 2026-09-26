//! `loop.json`: `ChatPilot.generate` over a scripted provider, replayed
//! through `answer` (identity session, governed question session, real
//! runner, fake provider) in v4's full-set tool mode with the tool-round
//! setting at v4's 4.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use kanade::chat::answer::{AnswerDeps, AnswerSettings, ChatPorts, Generation, Question, answer};
use kanade::chat::context::COMPLETION_RESERVE_TOKENS;
use kanade::chat::tools::ProposalCard;
use kanade::chat::tools::bundles::ToolOffer;
use kanade::chat::tools::read::PendingCard;
use kanade::infrastructure::llm::identity::{Member, Passthrough};
use kanade::infrastructure::llm::{
    CompletionResponse, FakeAction, FakeProvider, FinishReason, wire_body,
};
use serde_json::{Value, json};

use crate::common::text;
use crate::model::{Scripted, action, capabilities, client, effort};
use crate::read_tools::outcome_json;
use crate::support::{Named, check_family, dev, unknown_op, value};
use crate::wire::{kanade, messages};
use crate::world::World;

/// v4's `MAX_TOOL_ROUNDS`, set through the admin tool-round setting.
pub const V4_TOOL_ROUNDS: u8 = 4;

/// Records posted cards; `fail` makes every post fail.
#[derive(Default)]
pub struct Ports {
    pub posted: Mutex<Vec<ProposalCard>>,
    pub pending: Vec<PendingCard>,
    pub fail: bool,
    /// Posting never completes (a stuck gateway).
    pub hang: bool,
}

impl ChatPorts for Ports {
    async fn pending(&self) -> Vec<PendingCard> {
        self.pending.clone()
    }

    async fn post_card(&self, card: &ProposalCard) -> Result<(), String> {
        if self.hang {
            std::future::pending::<()>().await;
        }
        if self.fail {
            return Err("channel refused the card".into());
        }
        self.posted.lock().expect("posted").push(card.clone());
        Ok(())
    }
}

pub fn roster(world: &World) -> Vec<Member> {
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

/// The vectors' chat settings.
pub fn settings(input: &Value, tool_rounds: u8) -> AnswerSettings {
    let raw = &input["settings"];
    AnswerSettings {
        tool_rounds,
        timeout: Duration::from_secs_f64(raw["chat_pilot_timeout"].as_f64().expect("timeout")),
        reasoning: Some(effort(text(&raw["chat_pilot_think"]))),
        temperature: raw["chat_pilot_temperature"].as_f64(),
        max_output_tokens: u32::try_from(COMPLETION_RESERVE_TOKENS).expect("u32"),
        model_context_tokens: usize::try_from(
            raw["model_context_tokens"].as_u64().expect("context"),
        )
        .expect("usize"),
        clean_retry: true,
    }
}

/// A request as the recorder saw it: the wire body with tools as names.
fn request_json(body: Value) -> Value {
    let mut body = body;
    if let Some(tools) = body.get_mut("tools") {
        *tools = json!(
            tools
                .as_array()
                .expect("tools")
                .iter()
                .map(|tool| tool["function"]["name"].clone())
                .collect::<Vec<_>>()
        );
    }
    body
}

fn generation_json(generation: &Generation) -> Value {
    json!({
        "reply": generation.reply,
        "rounds": generation.rounds,
        "tool_calls": generation.tool_calls,
        "outcomes": generation.outcomes.iter().map(|o| {
            let mut out = outcome_json(&o.outcome);
            out["posted"] = json!(o.posted);
            out["round"] = json!(o.round);
            out
        }).collect::<Vec<_>>(),
        "error": generation.failure.as_ref().map(ToString::to_string),
        "model_rounds": generation.model_rounds.iter().map(|r| json!({
            "round": r.round,
            "content": r.content,
            "thinking": null,
            "requested_tools": r.requested_tools,
        })).collect::<Vec<_>>(),
        "prompt_tokens": generation.prompt_tokens,
        "completion_tokens": generation.completion_tokens,
        "created": generation.created,
        "posted": generation.posted,
        "focus": generation.focus.clone().unwrap_or_default(),
    })
}

/// What the reserved clean retry answers in the steps that reach it.
const CLEAN_REPLY: &str = "Which one did you mean?";

/// Steps whose answer v5 retries with a clean context (`D-CLEAN-RETRY`);
/// `true` when the v5 runner rejected the whole reply as unreadable, so no
/// model round or tool call is recorded: a malformed completion body, or
/// (`D-STRICT-TOOL-CALLS`) a reply with a duplicate call id or non-JSON
/// arguments, which v4 renamed or ran with `{}`.
const CLEAN_STEPS: [(&str, usize, bool); 5] = [
    ("answers-in-words", 1, false),
    ("answers-in-words", 3, false),
    ("refused-write-claims-are-overwritten", 2, false),
    ("call-ids-duplicates-and-malformed-calls", 0, true),
    ("transport-failures", 3, true),
];

fn clean_step(case_id: &str, step: usize) -> Option<bool> {
    CLEAN_STEPS
        .iter()
        .find(|(case, index, _)| *case == case_id && *index == step)
        .map(|&(_, _, strict)| strict)
}

async fn replay(case: Value) -> Vec<Value> {
    let input = &case["input"];
    let case_id = text(&case["case_id"]).to_owned();
    let mut world = World::new(input).await;
    let caps = capabilities(&input["caps"]);
    let persona = kanade();
    let model = text(&input["settings"]["chat_pilot_model"]);
    let roster = roster(&world);
    let mut out = Vec::new();
    for (index, step) in input["steps"].as_array().expect("steps").iter().enumerate() {
        if text(&step["op"]) != "generate" {
            unknown_op("loop", text(&step["op"]));
        }
        let replies = step["replies"].as_array().expect("replies");
        let mut actions: Vec<FakeAction> =
            replies.iter().map(|reply| action(reply, model)).collect();
        // The clean retry follows the first reply the runner rejected (the
        // rest of a strict step's script is never asked for), or the loop's end.
        match clean_step(&case_id, index) {
            Some(true) => actions.insert(1, FakeAction::Response(said(model, CLEAN_REPLY))),
            Some(false) => actions.push(FakeAction::Response(said(model, CLEAN_REPLY))),
            None => {}
        }
        let scripted = actions.len();
        let provider = Arc::new(Scripted {
            fake: FakeProvider::new(actions),
            caps: caps.clone(),
        });
        let alias = Some(model).filter(|alias| !alias.is_empty());
        let (_governor, client) = client(alias, provider.clone());
        let ctx = world.context(step);
        let question = Question {
            ctx: &ctx,
            conversation: messages(&step["conversation"]),
            reminder: persona.voice_reminder(),
            offer: ToolOffer::full_set(ctx.read_only),
            settings: settings(input, V4_TOOL_ROUNDS),
        };
        let ports = Ports::default();
        let deps = AnswerDeps {
            client: &client,
            codec: &Passthrough,
            roster: &roster,
            former: &[],
        };
        let (guild, mut proposer) = world.question_parts();
        let generation = answer(&deps, question, &guild, &mut proposer, &ports).await;
        let sent = provider.fake.requests();
        let mut result = generation_json(&generation);
        result["requests"] = json!(
            sent.iter()
                .map(|request| request_json(wire_body(request, &caps).expect("sent bodies shape")))
                .collect::<Vec<_>>()
        );
        result["unused_replies"] = json!(scripted - sent.len().min(scripted));
        out.push(value(result));
    }
    out
}

/// A plain answer in words.
pub fn said(model: &str, content: &str) -> CompletionResponse {
    CompletionResponse {
        model: model.to_owned(),
        content: Some(content.to_owned()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    }
}

/// `D-SHAPING`: a sampled request carries the runner's `max_tokens`.
fn shaped(request: &Value) -> Value {
    let mut request = request.clone();
    if request.get("temperature").is_some() {
        request["max_tokens"] = json!(COMPLETION_RESERVE_TOKENS);
    }
    request
}

/// The clean-context request: system prompt, the asker's message and the
/// voice reminder (the first request's messages in these steps), no tools.
fn clean_request(first: &Value) -> Value {
    let mut request = shaped(first);
    request.as_object_mut().expect("request").remove("tools");
    let messages = request["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 3, "system, question, reminder");
    request
}

fn clean_round(round: u64) -> Value {
    json!({"round": round, "content": CLEAN_REPLY, "thinking": null, "requested_tools": []})
}

/// The v5 value of a `CLEAN_STEPS` step, from its v4 value and script.
fn cleaned(v4: &Value, strict: bool, scripted: usize) -> Value {
    let mut v5 = v4.clone();
    let requests = v4["requests"].as_array().expect("requests");
    let clean = clean_request(&requests[0]);
    let mut sent: Vec<Value> = if strict {
        vec![shaped(&requests[0])]
    } else {
        requests.iter().map(shaped).collect()
    };
    sent.push(clean);
    v5["requests"] = json!(sent);
    let mut rounds: Vec<Value> = if strict {
        Vec::new()
    } else {
        v4["model_rounds"].as_array().expect("rounds").clone()
    };
    for round in &mut rounds {
        // `D-NO-THINKING`: v5 replies carry no reasoning text.
        round["thinking"] = Value::Null;
    }
    let last = v4["rounds"].as_u64().expect("rounds");
    rounds.push(clean_round(if strict { 1 } else { last }));
    v5["model_rounds"] = json!(rounds);
    if strict {
        v5["rounds"] = json!(1);
        v5["outcomes"] = json!([]);
        v5["tool_calls"] = json!([]);
        v5["unused_replies"] = json!(scripted - 1);
    }
    v5["reply"] = json!(CLEAN_REPLY);
    v5["error"] = Value::Null;
    v5
}

const CASES: [&str; 12] = [
    "answers-in-words",
    "read-then-grounded-answer",
    "round-cap-withholds-tools-on-the-last-round",
    "posted-write-reserves-the-confirmation-round",
    "refused-write-claims-are-overwritten",
    "call-ids-duplicates-and-malformed-calls",
    "read-only-turn",
    "transport-failures",
    "model-without-function-tools",
    "reasoning-and-sampling-controls",
    "missing-model-alias",
    "context-budget",
];

fn named() -> Vec<Named> {
    let file = crate::support::load("loop.json");
    let mut clean = Vec::new();
    let mut shaping = Vec::new();
    let mut usage = Vec::new();
    let mut failures = Vec::new();
    let mut floor = Vec::new();
    for case in file["cases"].as_array().expect("cases") {
        let case_id = *CASES
            .iter()
            .find(|id| **id == text(&case["case_id"]))
            .expect("a known loop case");
        for (index, step) in case["expected"]["steps"]
            .as_array()
            .expect("steps")
            .iter()
            .enumerate()
        {
            let v4 = &step["value"];
            let pointer = format!("/steps/{index}/value");
            if let Some(strict) = clean_step(case_id, index) {
                let scripted = case["input"]["steps"][index]["replies"]
                    .as_array()
                    .expect("replies")
                    .len();
                clean.push(dev(
                    case_id,
                    pointer,
                    v4.clone(),
                    cleaned(v4, strict, scripted),
                ));
                continue;
            }
            for (k, request) in v4["requests"]
                .as_array()
                .expect("requests")
                .iter()
                .enumerate()
            {
                if request.get("temperature").is_some() {
                    shaping.push(dev(
                        case_id,
                        format!("{pointer}/requests/{k}"),
                        request.clone(),
                        shaped(request),
                    ));
                }
            }
            let error = |v5: &str| {
                dev(
                    case_id,
                    format!("{pointer}/error"),
                    v4["error"].clone(),
                    json!(v5),
                )
            };
            match (case_id, index) {
                // A round's usage counts only when both counts are integers
                // (the second round reported `"150"` and `null`).
                ("read-then-grounded-answer", 0) => usage.push(dev(
                    case_id,
                    format!("{pointer}/prompt_tokens"),
                    json!(250),
                    json!(100),
                )),
                ("transport-failures", 1) => failures.push(error(
                    "LLM completion failed (BackendUnavailable, digest=d3382fb842105fb8)",
                )),
                ("transport-failures", 2) => failures.push(error(
                    "LLM completion failed (ProviderPermanent, digest=117eb089f60f5085)",
                )),
                ("model-without-function-tools", 0) => failures.push(error(
                    "LLM completion failed (UnsupportedCapability, digest=71a4de261d6d72bf)",
                )),
                // User decision 2026-09-26: `off` on a model whose list lacks
                // `none` sends its lowest level (v4 omitted the field).
                ("reasoning-and-sampling-controls", 0) => {
                    for (k, request) in v4["requests"]
                        .as_array()
                        .expect("requests")
                        .iter()
                        .enumerate()
                    {
                        let mut v5 = request.clone();
                        v5["reasoning_effort"] = json!("low");
                        floor.push(dev(
                            case_id,
                            format!("{pointer}/requests/{k}"),
                            request.clone(),
                            v5,
                        ));
                    }
                }
                ("missing-model-alias", 0) => {
                    failures.push(error("role is not configured"));
                    // The question never opened a session.
                    failures.push(dev(
                        case_id,
                        format!("{pointer}/rounds"),
                        json!(1),
                        json!(0),
                    ));
                }
                _ => {}
            }
        }
    }
    vec![
        Named {
            name: "D-CLEAN-RETRY",
            entries: clean,
        },
        Named {
            name: "D-SHAPING",
            entries: shaping,
        },
        Named {
            name: "D-USAGE-PAIRS",
            entries: usage,
        },
        Named {
            name: "D-TYPED-FAILURES",
            entries: failures,
        },
        Named {
            name: "D-REASONING-FLOOR",
            entries: floor,
        },
    ]
}

#[tokio::test(start_paused = true)]
async fn the_loop_family_replays_through_the_governed_question_loop() {
    assert_eq!(check_family("loop", &named(), replay).await, (12, 22));
}
