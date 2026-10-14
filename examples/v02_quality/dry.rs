//! `--dry-run`: a loopback stand-in for the model gateway, so the whole
//! harness runs offline. It lists the two dry aliases and plays an "ideal"
//! model from a script keyed on the case text: the tool calls and
//! amendments the draft expects. A dry run therefore checks the harness and
//! that the draft's expected outputs are reachable through the real code;
//! it says nothing about model quality.

use axum::{Json, Router, routing::get, routing::post};
use regex::Regex;
use serde_json::{Value, json};
use tokio::net::TcpListener;

pub const CHAT_ALIAS: &str = "dry-chat";
pub const EXTRACT_ALIAS: &str = "dry-extract";

/// Serves until the process exits; returns the base URL.
pub async fn start() -> std::io::Result<String> {
    let entry = |id: &str| {
        json!({
            "id": id, "object": "model",
            "kanata": {
                "operations": ["chat"], "structured_output": true,
                "sampling_controls": true, "reasoning_control": true,
                "function_tools": true, "streaming": false, "trust_zone": "local",
                "reasoning_efforts": ["none", "low", "medium", "high"],
                "context_tokens": 32768,
            },
        })
    };
    let listing = json!({"object": "list", "data": [entry(CHAT_ALIAS), entry(EXTRACT_ALIAS)]});
    let app = Router::new()
        .route("/v1/models", get(move || async move { Json(listing) }))
        .route(
            "/v1/chat/completions",
            post(|Json(body): Json<Value>| async move { Json(answer(&body)) }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(format!("http://{addr}/v1"))
}

fn answer(body: &Value) -> Value {
    let model = body["model"].as_str().unwrap_or_default().to_owned();
    // Chat's last round may offer no tools, so tell them apart by the prompt.
    if texts(body, "user")
        .iter()
        .any(|text| text.contains("NEW MESSAGES (extract from these)"))
    {
        return content(&model, &extraction(body).to_string());
    }
    let tools = body["tools"].as_array().cloned().unwrap_or_default();
    chat(&model, body, &tools)
}

fn texts(body: &Value, role: &str) -> Vec<String> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] == role)
        .filter_map(|message| message["content"].as_str().map(str::to_owned))
        .collect()
}

fn called(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|message| {
            message["tool_calls"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|call| call["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

/// The ideal calls for a question, in order, and the closing reply.
fn script(question: &str) -> (Vec<(&'static str, Value)>, &'static str) {
    let read = |args: Value| ("get_schedule", args);
    let listed = "Here's what's lined up.";
    if question.contains("bramble and i") {
        (
            vec![
                read(json!({"week": "auto", "day": "wednesday", "participant": "me"})),
                read(json!({"week": "auto", "day": "wednesday", "participant": "Bramble"})),
            ],
            "You have the Malefic Star at 21:30; Bramble has the Kalos at 23:00.",
        )
    } else if question.contains("100000000000000999") {
        (
            vec![read(
                json!({"week": "this", "participant": "<@100000000000000999>"}),
            )],
            "Whose schedule do you mean?",
        )
    } else if question.contains("push the kalos") {
        (
            vec![(
                "propose_move",
                json!({"run_query": "kalos tomorrow", "to_when": "tomorrow 23:30"}),
            )],
            "Card's up for the Kalos move; it needs a ✅ before it sticks.",
        )
    } else if question.contains("cancel the hstar") {
        (
            vec![("propose_cancel", json!({"run_query": "hstar wednesday"}))],
            "Card's up to call off the Malefic Star; it needs a ✅.",
        )
    } else if question.contains("move the wednesday run") {
        (
            vec![(
                "propose_move",
                json!({"run_query": "wednesday", "to_when": "wednesday 22:00"}),
            )],
            "Which Wednesday run, the 21:30 or the 23:00 one?",
        )
    } else if question.contains("this boss week") {
        (vec![read(json!({"week": "this_boss"}))], listed)
    } else if question.contains("next week") {
        (vec![read(json!({"week": "next"}))], listed)
    } else if question.contains("anything on friday") {
        (vec![read(json!({"week": "auto", "day": "friday"}))], listed)
    } else if question.contains("on wednesday") {
        (
            vec![read(json!({"week": "auto", "day": "wednesday"}))],
            listed,
        )
    } else if question.contains("next run for me") {
        (
            vec![read(json!({"week": "auto", "participant": "me"}))],
            listed,
        )
    } else if question.contains("does cobalt have") {
        (
            vec![read(json!({"week": "this", "participant": "Cobalt"}))],
            listed,
        )
    } else if question.contains("show my runs") {
        (
            vec![read(json!({"week": "this", "participant": "me"}))],
            listed,
        )
    } else if question.contains("this week") {
        (vec![read(json!({"week": "this"}))], listed)
    } else if question.contains("put bramble down") {
        (Vec::new(), "Bramble has to answer that one themselves.")
    } else if question.contains("essay") {
        (
            Vec::new(),
            "Mondays start the week tired. The rest is beyond my usual job.",
        )
    } else {
        (Vec::new(), "Not something I can do.")
    }
}

/// The question's ideal calls in order (a `request_tools` first when the
/// next one is not offered yet), then the closing reply.
fn chat(model: &str, body: &Value, tools: &[Value]) -> Value {
    let question = texts(body, "user")
        .into_iter()
        .rfind(|text| text.contains("<@100000000000000001>"))
        .unwrap_or_default()
        .to_lowercase();
    let (steps, reply) = script(&question);
    let offered = |name: &str| {
        tools
            .iter()
            .any(|tool| tool["function"]["name"].as_str() == Some(name))
    };
    let done = called(body);
    let made = done.iter().filter(|name| *name != "request_tools").count();
    let Some((tool, arguments)) = steps.get(made) else {
        return content(model, reply);
    };
    // Call ids must be unique within the conversation.
    let id = done.len();
    if offered(tool) {
        return tool_call(model, id, tool, arguments);
    }
    if offered("request_tools") && !done.iter().any(|name| name == "request_tools") {
        return tool_call(
            model,
            id,
            "request_tools",
            &json!({"bundle": "run_changes"}),
        );
    }
    content(model, reply)
}

/// One `NEW MESSAGES` line of the extraction prompt.
struct Said {
    id: String,
    author: String,
    text: String,
}

fn said(prompt: &str) -> Vec<Said> {
    let line =
        Regex::new(r"^\[(\d{15,20})\] \[[^\]]*\] \[\w+ <@(\d+)>\] (.*)$").expect("a valid pattern");
    prompt
        .split("NEW MESSAGES")
        .nth(1)
        .unwrap_or_default()
        .lines()
        .filter_map(|text| line.captures(text))
        .map(|found| Said {
            id: found[1].to_owned(),
            author: found[2].to_owned(),
            text: found[3].to_lowercase(),
        })
        .collect()
}

/// One ideal amendment: the line(s) it cites, then its fields.
struct Ideal {
    cites: &'static [&'static str],
    kind: &'static str,
    boss: &'static str,
    day: &'static str,
    time: Option<&'static str>,
    /// The first cited line's author is the party (add, sub).
    author_party: bool,
    question: bool,
    hint: Option<&'static str>,
}

const fn ideal(
    cites: &'static [&'static str],
    kind: &'static str,
    boss: &'static str,
    day: &'static str,
    time: Option<&'static str>,
    question: bool,
) -> Ideal {
    Ideal {
        cites,
        kind,
        boss,
        day,
        time,
        author_party: false,
        question,
        hint: None,
    }
}

/// The draft's bursts that expect a proposal (E09 and E11 expect none).
const IDEALS: [Ideal; 9] = [
    ideal(
        &["hcarl tonight can push"],
        "move",
        "HCarling",
        "tonight",
        Some("10pm"),
        true,
    ),
    Ideal {
        author_party: true,
        ..ideal(
            &["hlotus sat 9pm"],
            "add",
            "HLotus",
            "sat",
            Some("9pm"),
            true,
        )
    },
    ideal(
        &["hcarl tonight cannot"],
        "cancel",
        "HCarling",
        "tonight",
        None,
        false,
    ),
    ideal(
        &["hbellona mon shift"],
        "move",
        "HBellona",
        "mon",
        Some("2200"),
        false,
    ),
    ideal(
        &["hlimbo fri", "930pm"],
        "move",
        "HLimbo",
        "fri",
        Some("930pm"),
        false,
    ),
    ideal(
        &["kalos can push"],
        "move",
        "XKalos",
        "wed",
        Some("2330"),
        true,
    ),
    ideal(
        &["nbaldrix wed"],
        "move",
        "NBaldrix",
        "wed",
        Some("1030~11+pm"),
        true,
    ),
    Ideal {
        author_party: true,
        ..ideal(
            &["i cmi", "i can cover"],
            "sub",
            "HMaleficStar",
            "wed",
            None,
            false,
        )
    },
    Ideal {
        hint: Some("a1000003"),
        ..ideal(
            &["shift to fri 1030"],
            "move",
            "HMaleficStar",
            "fri",
            Some("1030"),
            true,
        )
    },
];

/// The ideal amendment for a draft burst; anything else is no change.
fn extraction(body: &Value) -> Value {
    let prompt = texts(body, "user").join("\n");
    let lines = said(&prompt);
    let found = IDEALS.iter().find_map(|ideal| {
        let cited: Option<Vec<&Said>> = ideal
            .cites
            .iter()
            .map(|words| lines.iter().find(|line| line.text.contains(words)))
            .collect();
        let cited = cited?;
        let party: Vec<&String> = if ideal.author_party {
            vec![&cited[0].author]
        } else {
            Vec::new()
        };
        Some(json!({
            "kind": ideal.kind, "bosses": [ideal.boss], "day_ref": ideal.day,
            "time_ref": ideal.time, "participants": party, "rsvp": null,
            "is_question": ideal.question, "confidence": 0.9,
            "evidence_message_ids": cited.iter().map(|line| &line.id).collect::<Vec<_>>(),
            "target_run_hint": ideal.hint,
        }))
    });
    json!({
        "amendments": found.into_iter().collect::<Vec<_>>(),
        "summary": "dry stand-in",
    })
}

fn content(model: &str, text: &str) -> Value {
    json!({
        "id": "chatcmpl-dry", "object": "chat.completion", "model": model,
        "choices": [{
            "index": 0, "finish_reason": "stop",
            "message": {"role": "assistant", "content": text},
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2},
    })
}

fn tool_call(model: &str, id: usize, name: &str, arguments: &Value) -> Value {
    json!({
        "id": "chatcmpl-dry-tools", "object": "chat.completion", "model": model,
        "choices": [{
            "index": 0, "finish_reason": "tool_calls",
            "message": {"role": "assistant", "content": null, "tool_calls": [{
                "id": format!("call_{id}"), "type": "function",
                "function": {"name": name, "arguments": arguments.to_string()},
            }]},
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2},
    })
}
