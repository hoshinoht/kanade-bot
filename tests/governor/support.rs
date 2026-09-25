use std::{collections::BTreeMap, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use kanade::infrastructure::llm::governor::{
    CallKind, Governor, GovernorConfig, GovernorPolicy, GroupConfig, GroupSnapshot, Priority,
    Random, Role, RoleConfig, Ticket,
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
