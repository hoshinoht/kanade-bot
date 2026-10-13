use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, ErrorCode, ExecutionLimits, Message,
};
use serde_json::{Map, Value, json};

use super::support::{
    answer_schema, output_schema, request, response, run_once, text_reply, tiny_request,
    tiny_response,
};

/// `limits(n)` admits `input`/`output` exactly at `n` and fails with
/// `over` (after `asked` provider requests) at `n - 1`.
async fn assert_exact_boundary(
    input: &ChatRequest,
    output: CompletionResponse,
    limits: impl Fn(usize) -> ExecutionLimits,
    len: usize,
    over: (Option<ErrorCode>, usize),
) {
    assert_eq!(
        run_once(input, output.clone(), limits(len)).await,
        (None, 1)
    );
    assert_eq!(run_once(input, output, limits(len - 1)).await, over);
}

const REFUSED: (Option<ErrorCode>, usize) = (Some(ErrorCode::RequestInvalid), 0);
const REJECTED: (Option<ErrorCode>, usize) = (Some(ErrorCode::InvalidOutput), 1);

#[tokio::test]
async fn canonical_request_size_matches_serde_json_at_both_boundaries() {
    let mut input = request();
    input.messages[0] = Message::System {
        content: "quote \" slash \\ line\ncontrol\tunicode ☃".repeat(8),
    };
    input.messages[1] = Message::User {
        content: "object { array [ ] }".repeat(8),
    };
    input.tools[0].description = Some("description \" \\ \n ☃".repeat(8));
    let encoded_len = serde_json::to_vec(&input).unwrap().len();
    let limits = |max_request_bytes| ExecutionLimits {
        max_request_bytes,
        ..ExecutionLimits::default()
    };
    assert_exact_boundary(&input, response(), limits, encoded_len, REFUSED).await;
}

#[tokio::test]
async fn canonical_schema_size_matches_serde_json_at_both_boundaries() {
    let key = "quote\"\\\n☃";
    let mut properties = Map::new();
    properties.insert(
        key.into(),
        json!({"type":"array","items":{"type":"string"}}),
    );
    let schema = json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": [key],
        "additionalProperties": false
    });
    let schema_len = serde_json::to_vec(&schema).unwrap().len();
    let mut input = tiny_request();
    input.output_schema = Some(output_schema("answer", schema));
    let mut output_object = Map::new();
    output_object.insert(key.into(), json!(["ok"]));
    let output = text_reply(Some(Value::Object(output_object).to_string()));
    let limits = |max_schema_bytes| ExecutionLimits {
        max_schema_bytes,
        ..ExecutionLimits::default()
    };
    assert_exact_boundary(&input, output, limits, schema_len, REFUSED).await;
}

#[tokio::test]
async fn canonical_response_size_matches_serde_json_at_both_boundaries() {
    let content = "quote \" slash \\ line\ncontrol\tunicode ☃".repeat(16);
    let output = text_reply(Some(content));
    let response_len = serde_json::to_vec(&output).unwrap().len();
    let limits = |max_response_bytes| ExecutionLimits {
        max_response_bytes,
        ..ExecutionLimits::default()
    };
    assert_exact_boundary(&tiny_request(), output, limits, response_len, REJECTED).await;
}

#[tokio::test]
async fn canonical_structured_output_size_matches_serde_json_at_both_boundaries() {
    let mut input = tiny_request();
    input.output_schema = Some(answer_schema());
    let value = json!({"answer": "quote \" slash \\ line\ncontrol\tunicode ☃"});
    let output_len = serde_json::to_vec(&value).unwrap().len();
    let output = text_reply(Some(value.to_string()));
    let limits = |max_output_bytes| ExecutionLimits {
        max_output_bytes,
        ..ExecutionLimits::default()
    };
    assert_exact_boundary(&input, output, limits, output_len, REJECTED).await;
}

#[tokio::test]
async fn encoded_large_all_of_schema_fails_before_provider_or_compilation() {
    let schema = json!({"allOf": vec![Value::Bool(false); 11_000]});
    assert!(
        serde_json::to_vec(&schema).unwrap().len() > ExecutionLimits::default().max_schema_bytes
    );
    let mut input = tiny_request();
    input.output_schema = Some(output_schema("large", schema));
    let outcome = run_once(&input, tiny_response("m"), ExecutionLimits::default()).await;
    assert_eq!(outcome, REFUSED);
}

#[tokio::test]
async fn encoded_escaping_changes_the_reservation() {
    let mut plain = tiny_request();
    plain.messages[0] = Message::User {
        content: "a".repeat(64),
    };
    let mut escaped = tiny_request();
    escaped.messages[0] = Message::User {
        content: "\"\\\n\t".repeat(16),
    };
    let plain_len = serde_json::to_vec(&plain).unwrap().len();
    let escaped_len = serde_json::to_vec(&escaped).unwrap().len();
    assert!(escaped_len > plain_len);
    let budget = u32::try_from(plain_len.div_ceil(4) + plain.max_output_tokens as usize).unwrap();
    let limits = ExecutionLimits {
        token_budget: budget,
        ..ExecutionLimits::default()
    };

    let outcome = run_once(&plain, tiny_response("m"), limits.clone()).await;
    assert_eq!(outcome, (None, 1));
    let outcome = run_once(&escaped, tiny_response("m"), limits).await;
    assert_eq!(outcome, (Some(ErrorCode::BudgetExceeded), 0));
}
