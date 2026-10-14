//! A governed model client over the fake provider, with fixed capabilities
//! and scripted replies.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use kanade::infrastructure::llm::governor::{
    Governor, GovernorConfig, GovernorPolicy, GroupConfig, ModelClient, Random, Role, RoleConfig,
};
use kanade::infrastructure::llm::{
    CapabilityFuture, ChatRequest, CompletionFuture, CompletionResponse, Effort, ExecutionLimits,
    FakeAction, FakeProvider, FinishReason, LlmProvider, ModelCapabilities, RetryPolicy, ToolCall,
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

    // The fake keeps the tagged id: what the HTTP provider sends as `x-request-id`.
    fn complete_tagged(
        &self,
        request: &ChatRequest,
        capabilities: &ModelCapabilities,
        request_id: &str,
    ) -> CompletionFuture<'_> {
        self.fake.complete_tagged(request, capabilities, request_id)
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

/// The world fixture's capability flags.
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

/// A reply asking for `calls`, each `(id, name, arguments)`.
pub fn wants<'a, Id: Into<String>>(
    model: &str,
    calls: impl IntoIterator<Item = (Id, &'a str, &'a Value)>,
) -> FakeAction {
    FakeAction::Response(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: model.into(),
        content: None,
        tool_calls: calls
            .into_iter()
            .map(|(id, name, arguments)| ToolCall {
                id: id.into(),
                name: name.into(),
                arguments: arguments.to_string(),
            })
            .collect(),
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    })
}

/// [`wants`] with each call's id `call-{name}` (unique across rounds while
/// no tool repeats: a transcript never repeats an id).
pub fn calls(model: &str, calls: &[(&str, Value)]) -> FakeAction {
    wants(
        model,
        calls
            .iter()
            .map(|(name, arguments)| (format!("call-{name}"), *name, arguments)),
    )
}

/// An empty reply the provider's content filter stopped.
pub fn filtered(model: &str) -> FakeAction {
    FakeAction::Response(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: model.into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::ContentFilter,
        usage: None,
    })
}

/// The fake provider replaying `actions` behind the minimal capabilities
/// plus function tools.
pub fn tool_model(actions: impl IntoIterator<Item = FakeAction>) -> Arc<Scripted> {
    let mut caps = ModelCapabilities::minimal();
    caps.function_tools = true;
    Arc::new(Scripted {
        caps,
        fake: FakeProvider::new(actions),
    })
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
