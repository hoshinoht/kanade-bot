use kanade::infrastructure::llm::{ErrorCode, ExecutionLimits, FinishReason};
use serde_json::{Value, json};

use super::support::{
    answer_schema, calls_reply, response_call, run_once, text_reply, tiny_request, tool_request,
};

fn limits(max_output_bytes: usize, token_budget: u32) -> ExecutionLimits {
    ExecutionLimits {
        max_output_bytes,
        token_budget,
        ..ExecutionLimits::default()
    }
}

#[tokio::test]
async fn hundred_kib_payloads_use_payload_limit_not_metadata_limit() {
    let mut plain = tiny_request();
    plain.max_output_tokens = 100_000;
    let mut structured = plain.clone();
    structured.output_schema = Some(answer_schema());
    let mut tools = tool_request();
    tools.max_output_tokens = 100_000;
    let structured_content = json!({"answer": "s".repeat(100 * 1024)}).to_string();
    let arguments = json!({"value": "a".repeat(100 * 1024)}).to_string();
    for (label, input, output) in [
        ("plain", plain, text_reply(Some("p".repeat(100 * 1024)))),
        (
            "structured",
            structured,
            text_reply(Some(structured_content)),
        ),
        (
            "tool arguments",
            tools,
            calls_reply(vec![response_call("large", "tool", arguments)]),
        ),
    ] {
        let outcome = run_once(&input, output, limits(200 * 1024, 200_000)).await;
        assert_eq!(outcome, (None, 1), "{label}");
    }
}

#[tokio::test]
async fn raw_payload_limits_are_exact_for_plain_content_and_arguments() {
    let limit = 128;
    let mut input = tiny_request();
    input.max_output_tokens = 10_000;
    for (length, expected) in [(limit, None), (limit + 1, Some(ErrorCode::InvalidOutput))] {
        let output = text_reply(Some("x".repeat(length)));
        let outcome = run_once(&input, output, limits(limit, 20_000)).await;
        assert_eq!(outcome, (expected, 1));
    }

    let empty_arguments = r#"{"value":""}"#;
    let value_length = limit - empty_arguments.len();
    for (extra, expected) in [(0, None), (1, Some(ErrorCode::InvalidOutput))] {
        let arguments = format!(r#"{{"value":"{}"}}"#, "x".repeat(value_length + extra));
        assert_eq!(arguments.len(), limit + extra);
        let output = calls_reply(vec![response_call("call", "tool", arguments)]);
        let outcome = run_once(&tool_request(), output, limits(limit, 20_000)).await;
        assert_eq!(outcome, (expected, 1));
    }
}

#[tokio::test]
async fn response_metadata_over_64_kib_still_fails() {
    let over = "x".repeat(64 * 1024 + 1);
    let ok = r#"{"value":"ok"}"#;
    let mut long_finish = text_reply(None);
    long_finish.finish_reason = FinishReason::Other(over.clone());
    let metadata_cases = [
        (
            tool_request(),
            calls_reply(vec![response_call(&over, "tool", ok)]),
        ),
        (
            tool_request(),
            calls_reply(vec![response_call("call", &over, ok)]),
        ),
        (tiny_request(), long_finish),
    ];
    for (index, (input, output)) in metadata_cases.into_iter().enumerate() {
        let outcome = run_once(&input, output, limits(200 * 1024, 20_000)).await;
        let expected = (Some(ErrorCode::InvalidOutput), 1);
        assert_eq!(outcome, expected, "metadata case {index}");
    }
}

#[tokio::test]
async fn multiple_payloads_can_exceed_the_response_aggregate() {
    let mut input = tool_request();
    input.max_output_tokens = 400_000;
    let arguments = json!({"value": "a".repeat(260 * 1024)}).to_string();
    let mut output = calls_reply(vec![response_call("aggregate", "tool", arguments)]);
    output.content = Some("c".repeat(260 * 1024));
    let outcome = run_once(&input, output, limits(300 * 1024, 500_000)).await;
    assert_eq!(outcome, (Some(ErrorCode::InvalidOutput), 1));
}

#[tokio::test]
async fn escaped_payloads_respect_raw_and_canonical_boundaries() {
    let mut input = tiny_request();
    input.max_output_tokens = 10_000;
    input.output_schema = Some(answer_schema());
    let value = json!({"answer":"quote \" slash \\ unicode ☃"});
    let canonical = serde_json::to_string(&value).unwrap();
    let escaped = r#"{"answer":"quote \u0022 slash \u005c unicode \u2603"}"#;
    let canonical_len = serde_json::to_vec(&value).unwrap().len();
    assert_eq!(serde_json::from_str::<Value>(escaped).unwrap(), value);
    assert!(escaped.len() > canonical_len);

    let cases = [(canonical, canonical_len), (escaped.into(), escaped.len())];
    for (payload, limit) in cases {
        let output = text_reply(Some(payload));
        let fits = run_once(&input, output.clone(), limits(limit, 20_000)).await;
        assert_eq!(fits, (None, 1));
        let over = run_once(&input, output, limits(limit - 1, 20_000)).await;
        assert_eq!(over, (Some(ErrorCode::InvalidOutput), 1));
    }
}
