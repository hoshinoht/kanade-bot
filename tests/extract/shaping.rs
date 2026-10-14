//! Named deviation D-SHAPING: extraction requests go through the v5 runner's
//! request shaping and wire encoding (docs/notes/provider-contract.md) instead of
//! v4's `extraction_body`. Each rewrite asserts the frozen v4 value first.
//!
//! - sampled requests carry `max_tokens` (the runner's output reservation) and
//!   a float `temperature` (`0.0`, same value);
//! - without structured output the schema instruction is the runner's ("one
//!   JSON value", keys sorted) rather than v4's ("one JSON object", pydantic
//!   key order);
//! - an effort outside a published `reasoning_efforts` list is refused before
//!   sending (`UnsupportedCapability`) where v4 silently omitted it.

use kanade::extract::prompt::CONTEXT_RESERVE;
use kanade::extract::schema::{extraction_schema, schema_text};
use kanade::infrastructure::llm::schema_instruction;
use serde_json::{Value, json};

const V4_PREFIX: &str = "OUTPUT FORMAT\nAnswer with exactly one JSON object and nothing else: \
                         no prose, no markdown. It must validate against this JSON Schema:\n";

/// v4 `llm.json_instruction()`.
pub fn v4_instruction() -> String {
    format!("{V4_PREFIX}{}", schema_text())
}

pub fn v5_instruction() -> String {
    schema_instruction(&extraction_schema()).expect("schema serializes")
}

/// One v4 wire body as the v5 runner sends it.
pub fn body(body: &mut Value) -> usize {
    let mut changed = 0;
    let object = body.as_object_mut().expect("body object");
    if let Some(temperature) = object.get_mut("temperature") {
        assert_eq!(*temperature, json!(0), "v4 temperature");
        *temperature = json!(0.0);
        assert!(!object.contains_key("max_tokens"), "v4 sends no max_tokens");
        object.insert("max_tokens".into(), json!(CONTEXT_RESERVE));
        changed += 1;
    }
    let (v4, v5) = (v4_instruction(), v5_instruction());
    for message in object["messages"].as_array_mut().expect("messages") {
        let content = message["content"].as_str().expect("content");
        if content.contains(&v4) {
            message["content"] = json!(content.replace(&v4, &v5));
            changed += 1;
        }
    }
    changed
}

/// Every request an `extract_call` step sent.
pub fn requests(value: &mut Value) -> usize {
    value["requests"]
        .as_array_mut()
        .expect("requests")
        .iter_mut()
        .map(body)
        .sum()
}

/// An effort outside the published list: v4 omitted it, v5 refuses.
pub fn refused_effort(value: &mut Value) -> usize {
    assert!(
        value.get("reasoning_effort").is_none(),
        "v4 omitted the effort"
    );
    assert!(value.get("model").is_some(), "a v4 body");
    *value = json!({ "refused": "UnsupportedCapability" });
    1
}

/// v4 `json_instruction()` becomes the runner's instruction.
pub fn instruction(value: &mut Value) -> usize {
    assert_eq!(*value, json!(v4_instruction()), "v4 instruction");
    *value = json!(v5_instruction());
    1
}

/// v4 `json_instruction_tokens()`; v5 budgets the runner's instruction.
pub fn instruction_tokens(value: &mut Value) -> usize {
    assert_eq!(*value, json!(787), "v4 instruction tokens");
    *value = json!(kanade::extract::prompt::schema_instruction_tokens(
        &extraction_schema()
    ));
    1
}
