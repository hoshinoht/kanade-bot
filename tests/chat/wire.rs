//! v4-shaped `{role, content, ...}` dicts ↔ v5 messages, and the tracked
//! Kanade persona the chat vectors were frozen with.

use kanade::chat::persona::{CompiledPersona, PersonaId, PersonaRoot};
use kanade::infrastructure::llm::{Message, ToolCallRequest};
use serde_json::{Value, json};

use crate::common::text;

/// v4's tracked public Kanade bundle with no reply profile.
pub fn kanade() -> CompiledPersona {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/personas");
    let root = PersonaRoot::open(&root).expect("tracked personas");
    let id = PersonaId::parse("kanade").expect("persona id");
    CompiledPersona::compile(&root.load_bundle(&id).expect("kanade bundle").value, None)
}

fn call(raw: &Value) -> ToolCallRequest {
    ToolCallRequest {
        id: text(&raw["id"]).to_owned(),
        name: text(&raw["function"]["name"]).to_owned(),
        arguments: text(&raw["function"]["arguments"]).to_owned(),
    }
}

pub fn message(raw: &Value) -> Message {
    let content = || raw["content"].as_str().unwrap_or_default().to_owned();
    match text(&raw["role"]) {
        "system" => Message::System { content: content() },
        "user" => Message::User { content: content() },
        "assistant" => Message::Assistant {
            content: raw["content"].as_str().map(str::to_owned),
            tool_calls: raw
                .get("tool_calls")
                .and_then(Value::as_array)
                .map(|calls| calls.iter().map(call).collect())
                .unwrap_or_default(),
        },
        "tool" => Message::Tool {
            tool_call_id: text(&raw["tool_call_id"]).to_owned(),
            content: content(),
        },
        other => panic!("unexpected role {other}"),
    }
}

pub fn messages(raw: &Value) -> Vec<Message> {
    raw.as_array()
        .expect("messages")
        .iter()
        .map(message)
        .collect()
}

/// v4's rendering: an assistant tool turn carries `content` (`""` when none).
pub fn message_json(message: &Message) -> Value {
    match message {
        Message::System { content } => json!({"role": "system", "content": content}),
        Message::User { content } => json!({"role": "user", "content": content}),
        Message::Assistant {
            content,
            tool_calls,
        } => {
            let mut out =
                json!({"role": "assistant", "content": content.clone().unwrap_or_default()});
            if !tool_calls.is_empty() {
                out["tool_calls"] = json!(
                    tool_calls
                        .iter()
                        .map(|c| json!({
                            "id": c.id,
                            "type": "function",
                            "function": {"name": c.name, "arguments": c.arguments},
                        }))
                        .collect::<Vec<_>>()
                );
            }
            out
        }
        Message::Tool {
            tool_call_id,
            content,
        } => json!({"role": "tool", "tool_call_id": tool_call_id, "content": content}),
    }
}

pub fn messages_json(messages: &[Message]) -> Value {
    json!(messages.iter().map(message_json).collect::<Vec<_>>())
}
