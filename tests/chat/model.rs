//! A governed model client over the fake provider, with fixed capabilities,
//! and v4 scripted completions read the way the v5 wire parser reads them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use kanade::infrastructure::llm::governor::{
    Governor, GovernorConfig, GovernorPolicy, GroupConfig, ModelClient, Random, Role, RoleConfig,
};
use kanade::infrastructure::llm::{
    CapabilityFuture, ChatRequest, CompletionFuture, CompletionResponse, Effort, ExecutionLimits,
    FakeAction, FakeProvider, FinishReason, LlmProvider, ModelCapabilities, RetryPolicy, ToolCall,
    Usage,
};
use serde_json::Value;

use crate::common::strings;

struct Fixed;

impl Random for Fixed {
    fn next_u64(&self) -> u64 {
        1 << 63
    }
}

/// The fake provider behind published capabilities.
pub struct Scripted {
    pub fake: FakeProvider,
    pub caps: ModelCapabilities,
}

impl LlmProvider for Scripted {
    fn complete(&self, request: &ChatRequest) -> CompletionFuture<'_> {
        self.fake.complete(request)
    }

    fn capabilities<'a>(
        &'a self,
        _model: &'a str,
        _deadline: tokio::time::Instant,
    ) -> CapabilityFuture<'a> {
        let caps = self.caps.clone();
        Box::pin(async move { Some(caps) })
    }
}

pub fn effort(name: &str) -> Effort {
    match name {
        "off" => Effort::Off,
        "low" => Effort::Low,
        "medium" => Effort::Medium,
        "high" => Effort::High,
        other => panic!("effort {other}"),
    }
}

/// The vectors' capability sets.
pub fn capabilities(raw: &Value) -> ModelCapabilities {
    let mut caps = ModelCapabilities::minimal();
    let flag = |name: &str| raw[name].as_bool().expect("capability flag");
    caps.structured_output = flag("structured_output");
    caps.sampling_controls = flag("sampling_controls");
    caps.reasoning_control = flag("reasoning_control");
    caps.function_tools = flag("function_tools");
    caps.reasoning_efforts = (!raw["reasoning_efforts"].is_null()).then(|| {
        strings(&raw["reasoning_efforts"])
            .iter()
            .map(|e| effort(e))
            .collect()
    });
    caps
}

/// v5 `wire::parse_completion` over one scripted body (it is crate-private):
/// missing ids become `call_<index>`, usage needs both integer counts.
pub fn completion(raw: &Value, model: &str) -> Option<CompletionResponse> {
    let choice = raw.get("choices")?.as_array()?.first()?.as_object()?;
    let message = choice.get("message")?.as_object()?;
    let mut tool_calls = Vec::new();
    let mut reserved: Vec<String> = Vec::new();
    for (index, call) in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let function = call.get("function");
        let name = function
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let arguments = match function.and_then(|f| f.get("arguments")) {
            Some(Value::String(text)) => text.clone(),
            None | Some(Value::Null) => "{}".to_owned(),
            Some(other) => other.to_string(),
        };
        let id = match call.get("id").and_then(Value::as_str) {
            Some(id) if !id.is_empty() => id.to_owned(),
            _ => {
                let mut id = format!("call_{index}");
                while reserved.contains(&id) {
                    id.push('_');
                }
                id
            }
        };
        reserved.push(id.clone());
        tool_calls.push(ToolCall {
            id,
            name,
            arguments,
        });
    }
    let finish_reason = match choice.get("finish_reason").and_then(Value::as_str) {
        Some("stop") if !tool_calls.is_empty() => FinishReason::ToolCalls,
        Some("stop") => FinishReason::Stop,
        Some("tool_calls") => FinishReason::ToolCalls,
        Some("length") => FinishReason::Length,
        Some("content_filter") => FinishReason::ContentFilter,
        Some(other) => FinishReason::Other(other.to_owned()),
        None => FinishReason::Other(String::new()),
    };
    let count = |key: &str| {
        raw.get("usage")
            .and_then(|usage| usage.get(key))
            .and_then(Value::as_u64)
            .and_then(|count| u32::try_from(count).ok())
    };
    let usage = match (count("prompt_tokens"), count("completion_tokens")) {
        (Some(prompt_tokens), Some(completion_tokens)) => Some(Usage {
            prompt_tokens,
            completion_tokens,
        }),
        _ => None,
    };
    Some(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: model.to_owned(),
        content: message
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_owned),
        tool_calls,
        finish_reason,
        usage,
    })
}

/// One scripted v4 reply as a fake provider action.
pub fn action(raw: &Value, model: &str) -> FakeAction {
    match raw.get("raise").and_then(Value::as_str) {
        Some("TimeoutError") => FakeAction::Delayed {
            delay: Duration::from_secs(3600),
            action: Box::new(FakeAction::Permanent),
        },
        Some("ModelUnavailable") => FakeAction::BackendUnavailable,
        Some("RuntimeError") => FakeAction::Permanent,
        Some(other) => panic!("unknown raise {other}"),
        None => completion(&raw["completion"], model)
            .map_or(FakeAction::Malformed, FakeAction::Response),
    }
}

/// One permit, no rate waits; `chat` routes the chat role to `alias` (none
/// when `None`).
pub fn client(
    alias: Option<&str>,
    provider: Arc<Scripted>,
) -> (Arc<Governor>, ModelClient<Scripted>) {
    let name = alias.unwrap_or("unrouted");
    let config = GovernorConfig {
        groups: vec![GroupConfig {
            name: "local".into(),
            backend: "local backend".into(),
            permits: 1,
            requests_per_min: 6_000,
            burst: Some(1_000),
            aliases: vec![name.into()],
        }],
        roles: alias
            .map(|alias| {
                (
                    Role::Chat,
                    RoleConfig {
                        alias: alias.into(),
                        external: false,
                    },
                )
            })
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        policy: GovernorPolicy::default(),
    };
    let governor = Arc::new(Governor::new(&config, Arc::new(Fixed)).expect("valid config"));
    let retry = RetryPolicy {
        total_deadline: Duration::from_secs(60),
        max_attempts: 3,
        backoff: Duration::from_millis(100),
    };
    let client = ModelClient::new(
        governor.clone(),
        provider,
        ExecutionLimits::default(),
        retry,
    )
    .expect("valid client");
    (governor, client)
}
