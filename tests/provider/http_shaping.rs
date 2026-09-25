use std::{collections::BTreeMap, sync::Arc};

use kanade::infrastructure::llm::{
    ChatRequest, CompletionRunner, Effort, ErrorCode, ExecutionLimits, HttpProviderConfig, Message,
    ModelCapabilities, OpenAiCompatibleProvider, OutputSchema, OutputValidation, Sampling,
    ToolDefinition, shape_request,
};
use serde_json::{Value, json};

use super::{
    stub::{Stub, gateway, kanata_models, ollama_models},
    support::{default_retry, tool_schema},
};

const ANSWER: &str = r#"{"answer":"ok"}"#;
const SCHEMA_TEXT: &str = r#"{"additionalProperties":false,"properties":{"answer":{"type":"string"}},"required":["answer"],"type":"object"}"#;
const INSTRUCTION: &str = "OUTPUT FORMAT\nAnswer with exactly one JSON value and nothing else: no prose, no markdown. It must validate against this JSON Schema:\n";

fn answer_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"],
        "additionalProperties": false
    })
}

fn structured(model: &str) -> ChatRequest {
    ChatRequest {
        model: model.into(),
        messages: vec![
            Message::System {
                content: "sys".into(),
            },
            Message::User {
                content: "hi".into(),
            },
        ],
        tools: Vec::new(),
        output_schema: Some(OutputSchema {
            name: "answer".into(),
            schema: answer_schema(),
            strict: true,
            validation: OutputValidation::Runner,
        }),
        max_output_tokens: 64,
        reasoning: Some(Effort::High),
        sampling: Some(Sampling {
            temperature: Some(0.25),
            seed: Some(7),
            top_p: Some(0.5),
        }),
    }
}

fn provider(
    url: String,
    declared: BTreeMap<String, ModelCapabilities>,
) -> Arc<OpenAiCompatibleProvider> {
    let mut config = HttpProviderConfig::new(url);
    config.declared = declared;
    Arc::new(OpenAiCompatibleProvider::new(config).unwrap())
}

fn runner(provider: Arc<OpenAiCompatibleProvider>) -> CompletionRunner<OpenAiCompatibleProvider> {
    runner_with(provider, ExecutionLimits::default())
}

fn runner_with(
    provider: Arc<OpenAiCompatibleProvider>,
    limits: ExecutionLimits,
) -> CompletionRunner<OpenAiCompatibleProvider> {
    CompletionRunner::ungoverned(provider, limits, default_retry()).unwrap()
}

fn messages(system: &str) -> Value {
    json!([
        {"role": "system", "content": system},
        {"role": "user", "content": "hi"}
    ])
}

#[tokio::test]
async fn kanata_metadata_model_gets_every_supported_field() {
    let stub = Stub::start(gateway(kanata_models(), ANSWER)).await;
    let provider = provider(format!("{}/v1/", stub.url()), BTreeMap::new());
    let completed = runner(provider)
        .complete(&structured("sumi-structured"))
        .await
        .unwrap();
    assert_eq!(completed.content.as_deref(), Some(ANSWER));

    let chats = stub.chat_requests();
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].method, "POST");
    assert_eq!(chats[0].path, "/v1/chat/completions");
    assert_eq!(
        chats[0].body,
        json!({
            "model": "sumi-structured",
            "messages": messages("sys"),
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": "answer", "schema": answer_schema(), "strict": true}
            },
            "max_tokens": 64,
            "temperature": 0.25,
            "seed": 7,
            "top_p": 0.5,
            "reasoning_effort": "high"
        })
    );
    let listings: Vec<_> = stub
        .requests()
        .into_iter()
        .filter(|request| request.method == "GET")
        .collect();
    assert_eq!(listings.len(), 1);
    assert_eq!(listings[0].path, "/v1/models");
}

#[tokio::test]
async fn bare_ollama_model_uses_operator_declared_capabilities() {
    let stub = Stub::start(gateway(ollama_models(), ANSWER)).await;
    let declared = ModelCapabilities {
        structured_output: true,
        sampling_controls: true,
        function_tools: true,
        ..ModelCapabilities::minimal()
    };
    let provider = provider(
        stub.url(),
        BTreeMap::from([("qwen3:8b".to_owned(), declared)]),
    );
    let mut request = structured("qwen3:8b");
    request.tools = vec![ToolDefinition {
        name: "tool".into(),
        description: Some("synthetic".into()),
        input_schema: tool_schema(),
    }];
    runner(provider).complete(&request).await.unwrap();

    let chats = stub.chat_requests();
    assert_eq!(chats.len(), 1);
    assert_eq!(chats[0].path, "/v1/chat/completions");
    assert_eq!(
        chats[0].body,
        json!({
            "model": "qwen3:8b",
            "messages": messages("sys"),
            "tools": [{
                "type": "function",
                "function": {"name": "tool", "description": "synthetic", "parameters": tool_schema()}
            }],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": "answer", "schema": answer_schema(), "strict": true}
            },
            "max_tokens": 64,
            "temperature": 0.25,
            "seed": 7,
            "top_p": 0.5
        })
    );
}

#[tokio::test]
async fn minimal_profile_sends_model_and_messages_with_the_schema_instruction() {
    let stub = Stub::start(gateway(ollama_models(), ANSWER)).await;
    let provider = provider(stub.url(), BTreeMap::new());
    runner(provider)
        .complete(&structured("llama3.2:3b"))
        .await
        .unwrap();

    let chats = stub.chat_requests();
    assert_eq!(chats.len(), 1);
    assert_eq!(
        chats[0].body,
        json!({
            "model": "llama3.2:3b",
            "messages": messages(&format!("sys\n\n{INSTRUCTION}{SCHEMA_TEXT}")),
        })
    );
}

#[tokio::test]
async fn codex_like_model_gets_reasoning_only() {
    let stub = Stub::start(gateway(kanata_models(), ANSWER)).await;
    let provider = provider(stub.url(), BTreeMap::new());
    let runner = runner(provider);
    for effort in [Effort::Low, Effort::Medium, Effort::High] {
        let mut request = structured("codex-like");
        request.reasoning = Some(effort);
        runner.complete(&request).await.unwrap();
        let body = stub.chat_requests().last().unwrap().body.clone();
        assert_eq!(
            body,
            json!({
                "model": "codex-like",
                "messages": messages(&format!("sys\n\n{INSTRUCTION}{SCHEMA_TEXT}")),
                "reasoning_effort": effort.as_str()
            })
        );
    }
    // codex-like publishes no `none`: off sends its lowest level instead.
    let mut off = structured("codex-like");
    off.reasoning = Some(Effort::Off);
    runner.complete(&off).await.unwrap();
    let body = stub.chat_requests().last().unwrap().body.clone();
    assert_eq!(body["reasoning_effort"], "low");
    assert_eq!(stub.chat_requests().len(), 4);
}

#[tokio::test]
async fn cloud_model_gets_the_schema_in_the_prompt_and_replies_are_still_validated() {
    let stub = Stub::start(gateway(
        kanata_models(),
        "```json\n{\"answer\":\"fenced\"}\n```",
    ))
    .await;
    let provider = provider(stub.url(), BTreeMap::new());
    let mut request = structured("glm-cloud");
    request.messages.remove(0);
    let completed = runner(provider.clone()).complete(&request).await.unwrap();
    assert_eq!(completed.content.as_deref(), Some(r#"{"answer":"fenced"}"#));
    assert!(
        provider
            .model_capabilities("glm-cloud")
            .await
            .is_cloud("glm-cloud")
    );
    assert_eq!(
        stub.chat_requests()[0].body,
        json!({
            "model": "glm-cloud",
            "messages": [
                {"role": "system", "content": format!("{INSTRUCTION}{SCHEMA_TEXT}")},
                {"role": "user", "content": "hi"}
            ],
            "max_tokens": 64,
            "temperature": 0.25,
            "seed": 7,
            "top_p": 0.5
        })
    );

    for bad in [r#"{"answer":7}"#, "sure! {\"answer\":\"ok\"}"] {
        let stub = Stub::start(gateway(kanata_models(), bad)).await;
        let bad_provider = self::provider(stub.url(), BTreeMap::new());
        assert_eq!(
            runner(bad_provider)
                .complete(&request)
                .await
                .unwrap_err()
                .code,
            ErrorCode::InvalidOutput,
            "{bad}"
        );
        assert_eq!(stub.chat_requests().len(), 1);
    }
}

#[tokio::test]
async fn unpublished_reasoning_level_is_rejected_before_sending() {
    let stub = Stub::start(gateway(ollama_models(), ANSWER)).await;
    let declared = ModelCapabilities {
        reasoning_control: true,
        reasoning_efforts: Some(vec![Effort::Low]),
        ..ModelCapabilities::minimal()
    };
    let provider = provider(
        stub.url(),
        BTreeMap::from([("qwen3:8b".to_owned(), declared)]),
    );
    let runner = runner(provider);
    let mut request = structured("qwen3:8b");
    request.reasoning = Some(Effort::High);
    assert_eq!(
        runner.complete(&request).await.unwrap_err().code,
        ErrorCode::UnsupportedCapability
    );
    assert!(stub.chat_requests().is_empty());

    request.reasoning = Some(Effort::Off);
    runner.complete(&request).await.unwrap();
    request.reasoning = Some(Effort::Low);
    runner.complete(&request).await.unwrap();
    let chats = stub.chat_requests();
    assert_eq!(chats.len(), 2);
    assert_eq!(
        chats[0].body["reasoning_effort"], "low",
        "off → lowest level"
    );
    assert_eq!(chats[1].body["reasoning_effort"], "low");
}

#[tokio::test]
async fn tools_are_refused_for_models_without_function_tools() {
    let stub = Stub::start(gateway(ollama_models(), ANSWER)).await;
    let provider = provider(stub.url(), BTreeMap::new());
    let mut request = structured("qwen3:8b");
    request.tools = vec![ToolDefinition {
        name: "tool".into(),
        description: None,
        input_schema: tool_schema(),
    }];
    assert_eq!(
        runner(provider).complete(&request).await.unwrap_err().code,
        ErrorCode::UnsupportedCapability
    );
    assert!(stub.chat_requests().is_empty());
}

#[tokio::test]
async fn the_schema_instruction_counts_toward_the_token_budget() {
    let request = structured("llama3.2:3b");
    let shaped = shape_request(&request, &ModelCapabilities::minimal())
        .unwrap()
        .unwrap();
    let reservation = |request: &ChatRequest| {
        u32::try_from(serde_json::to_vec(request).unwrap().len().div_ceil(4)).unwrap()
            + request.max_output_tokens
    };
    let shaped_reservation = reservation(&shaped);
    assert!(shaped_reservation > reservation(&request));

    for (budget, ok) in [(shaped_reservation, true), (shaped_reservation - 1, false)] {
        let stub = Stub::start(gateway(ollama_models(), ANSWER)).await;
        let provider = provider(stub.url(), BTreeMap::new());
        let result = runner_with(
            provider,
            ExecutionLimits {
                token_budget: budget,
                ..ExecutionLimits::default()
            },
        )
        .complete(&request)
        .await;
        if ok {
            assert!(result.is_ok());
            assert_eq!(stub.chat_requests().len(), 1);
        } else {
            assert_eq!(result.unwrap_err().code, ErrorCode::BudgetExceeded);
            assert!(stub.chat_requests().is_empty());
        }
    }
}

#[test]
fn shaping_is_a_no_op_for_structured_models_and_plain_requests() {
    let full = ModelCapabilities {
        structured_output: true,
        function_tools: true,
        ..ModelCapabilities::minimal()
    };
    assert!(shape_request(&structured("m"), &full).unwrap().is_none());
    let mut plain = structured("m");
    plain.output_schema = None;
    assert!(
        shape_request(&plain, &ModelCapabilities::minimal())
            .unwrap()
            .is_none()
    );
}
