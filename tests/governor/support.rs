use std::{collections::BTreeMap, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use kanade_llm::{
    ChatRequest, CompletionResponse, ExecutionLimits, FakeAction, FakeProvider, FinishReason,
    Message, RetryPolicy, Usage,
    governor::{
        CallKind, Governor, GovernorConfig, GovernorPolicy, GroupConfig, GroupSnapshot,
        ModelClient, Priority, Random, Role, RoleConfig, Ticket,
    },
};

pub const ALIAS: &str = "local-model";

/// Always the same draw; `1 << 63` is exactly 0.5 as a unit fraction.
pub struct FixedRandom(pub u64);

impl Random for FixedRandom {
    fn next_u64(&self) -> u64 {
        self.0
    }
}

pub const HALF: u64 = 1 << 63;

pub fn group(name: &str, permits: u32, per_min: u32, aliases: &[&str]) -> GroupConfig {
    GroupConfig {
        name: name.into(),
        backend: format!("{name} backend"),
        permits,
        requests_per_min: per_min,
        burst: None,
        aliases: aliases.iter().map(|a| a.to_string()).collect(),
    }
}

pub fn roles(alias: &str, external: bool) -> BTreeMap<Role, RoleConfig> {
    [Role::Chat, Role::Extraction, Role::Rewrite]
        .into_iter()
        .map(|role| {
            (
                role,
                RoleConfig {
                    alias: alias.into(),
                    external,
                },
            )
        })
        .collect()
}

pub fn single(permits: u32, per_min: u32) -> GovernorConfig {
    GovernorConfig {
        groups: vec![group("local", permits, per_min, &[ALIAS])],
        roles: roles(ALIAS, false),
        policy: GovernorPolicy::default(),
    }
}

pub fn build(config: &GovernorConfig) -> Arc<Governor> {
    Arc::new(Governor::new(config, Arc::new(FixedRandom(HALF))).expect("valid config"))
}

pub fn governor(permits: u32, per_min: u32) -> Arc<Governor> {
    build(&single(permits, per_min))
}

/// No rate waits; `retry_floor` retries allowed per window.
pub fn roomy(permits: u32, retry_floor: u32) -> GovernorConfig {
    let mut config = single(permits, 6_000);
    config.groups[0].burst = Some(1_000);
    config.policy.retry_floor = retry_floor;
    config
}

/// A client over a fake answering `actions`: 30 s deadline, 3 attempts, 100 ms backoff.
pub fn client(
    governor: Arc<Governor>,
    actions: impl IntoIterator<Item = FakeAction>,
) -> (Arc<FakeProvider>, ModelClient<FakeProvider>) {
    let provider = Arc::new(FakeProvider::new(actions));
    let retry = RetryPolicy {
        total_deadline: Duration::from_secs(30),
        max_attempts: 3,
        backoff: Duration::from_millis(100),
    };
    let client = ModelClient::new(
        governor,
        provider.clone(),
        ExecutionLimits::default(),
        retry,
    )
    .expect("valid client");
    (provider, client)
}

/// One user message to `ALIAS`.
pub fn ask(content: &str, max_output_tokens: u32) -> ChatRequest {
    ChatRequest {
        model: ALIAS.into(),
        messages: vec![Message::User {
            content: content.into(),
        }],
        tools: Vec::new(),
        output_schema: None,
        max_output_tokens,
        reasoning: None,
        sampling: None,
        prompt_cache_key: None,
    }
}

/// A `Stop` reply from `ALIAS` with one prompt and one completion token.
pub fn reply(content: &str) -> CompletionResponse {
    CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: ALIAS.into(),
        content: Some(content.into()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: Some(Usage {
            prompt_tokens: 1,
            completion_tokens: 1,
            cached_tokens: None,
        }),
    }
}

pub fn ticket(priority: Priority, who: &str) -> Ticket {
    let kind = match priority {
        Priority::Admin | Priority::ChatRound | Priority::ChatNew => CallKind::Chat,
        Priority::Extraction => CallKind::Extraction,
        Priority::FollowUp => CallKind::FollowUp,
    };
    Ticket {
        priority,
        kind,
        who: who.into(),
    }
}

pub fn wall() -> DateTime<Utc> {
    DateTime::from_timestamp(1_790_000_000, 0).expect("valid timestamp")
}

pub fn snap(governor: &Governor) -> GroupSnapshot {
    governor.snapshot(wall()).remove(0)
}

/// Lets spawned tasks run without letting paused time auto-advance.
pub async fn settle() {
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

pub const LONG: Duration = Duration::from_secs(3_600);
