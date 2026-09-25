use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    time::Duration,
};

use super::super::AdmissionLimits;

pub const MAX_PERMITS: u32 = 64;
pub const MAX_REQUESTS_PER_MIN: u32 = 6_000;
pub const MAX_BURST: u32 = 1_000;
/// Retries may never exceed 20% of recent requests (user decision: ~10–20%).
pub const MAX_RETRY_PERMILLE: u16 = 200;
const MAX_RETRY_FLOOR: u32 = 10;
const MAX_COOLDOWN: Duration = Duration::from_secs(3_600);
const MAX_WINDOW: Duration = Duration::from_secs(3_600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    Extraction,
    Chat,
    Rewrite,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extraction => "extraction",
            Self::Chat => "chat",
            Self::Rewrite => "rewrite",
        }
    }
}

/// One backend capacity group: aliases sharing hardware share its permits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupConfig {
    pub name: String,
    /// Human description of the backend, shown on the Limits page.
    pub backend: String,
    pub permits: u32,
    pub requests_per_min: u32,
    /// Token-bucket depth; `None` means one token per permit.
    pub burst: Option<u32>,
    pub aliases: Vec<String>,
}

impl GroupConfig {
    pub fn effective_burst(&self) -> u32 {
        self.burst.unwrap_or(self.permits)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleConfig {
    pub alias: String,
    /// Routed outside the local trust zone (cloud).
    pub external: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GovernorPolicy {
    /// Consecutive transient failures or timeouts that open a group's breaker.
    pub breaker_threshold: u32,
    pub open_cooldown: Duration,
    pub max_open_cooldown: Duration,
    /// After recovery, one more permit is released per step until all are.
    pub drain_step: Duration,
    pub retry_permille: u16,
    /// Retries always allowed per window so a quiet group can still retry once.
    pub retry_floor: u32,
    pub retry_window: Duration,
}

impl Default for GovernorPolicy {
    fn default() -> Self {
        Self {
            breaker_threshold: 5,
            open_cooldown: Duration::from_secs(30),
            max_open_cooldown: Duration::from_secs(300),
            drain_step: Duration::from_secs(5),
            retry_permille: 150,
            retry_floor: 1,
            retry_window: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GovernorConfig {
    pub groups: Vec<GroupConfig>,
    pub roles: BTreeMap<Role, RoleConfig>,
    pub policy: GovernorPolicy,
}

/// Where a role's calls go once the configuration is resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleRoute {
    pub role: Role,
    pub alias: String,
    pub group: Option<String>,
    pub external: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    EmptyGroupName,
    DuplicateGroup(String),
    Permits { group: String, value: u32 },
    RequestsPerMin { group: String, value: u32 },
    Burst { group: String, value: u32 },
    NoAliases { group: String },
    EmptyAlias { group: String },
    DuplicateAlias { alias: String },
    EmptyRoleAlias { role: Role },
    Policy(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGroupName => f.write_str("a backend group has an empty name"),
            Self::DuplicateGroup(name) => write!(f, "backend group {name:?} is listed twice"),
            Self::Permits { group, value } => write!(
                f,
                "backend group {group:?}: permits {value} is not a whole number from 1 to {MAX_PERMITS}"
            ),
            Self::RequestsPerMin { group, value } => write!(
                f,
                "backend group {group:?}: requests per minute {value} is not from 1 to {MAX_REQUESTS_PER_MIN}"
            ),
            Self::Burst { group, value } => write!(
                f,
                "backend group {group:?}: burst {value} is not from 1 to {MAX_BURST}"
            ),
            Self::NoAliases { group } => write!(f, "backend group {group:?} has no aliases"),
            Self::EmptyAlias { group } => write!(f, "backend group {group:?} has an empty alias"),
            Self::DuplicateAlias { alias } => {
                write!(f, "alias {alias:?} is listed more than once")
            }
            Self::EmptyRoleAlias { role } => write!(f, "role {} has no alias", role.as_str()),
            Self::Policy(field) => write!(f, "governor policy field {field} is out of range"),
        }
    }
}

impl std::error::Error for ConfigError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigWarning {
    /// Contract: a role's model with no group only warns; the governor refuses its calls.
    UngroupedRole { role: Role, alias: String },
    /// A group holds more permits than the gateway admits for one of its aliases
    /// (min of route and adapter `max_in_flight`); the excess would queue or be
    /// refused at the gateway instead of in Kanade's fair queue.
    PermitsAboveGateway {
        group: String,
        alias: String,
        permits: u32,
        gateway: u32,
    },
}

impl GovernorConfig {
    /// Compares each group's permits with what the gateway publishes for its
    /// aliases. `published` returns `None` where nothing is published (public
    /// listener, plain Ollama); those limits stay operator-declared. Per-key
    /// limits are never published, so they are not checked here.
    pub fn capacity_warnings(
        &self,
        published: impl Fn(&str) -> Option<AdmissionLimits>,
    ) -> Vec<ConfigWarning> {
        let mut warnings = Vec::new();
        for group in &self.groups {
            for alias in &group.aliases {
                if let Some(limits) = published(alias)
                    && group.permits > limits.concurrency()
                {
                    warnings.push(ConfigWarning::PermitsAboveGateway {
                        group: group.name.clone(),
                        alias: alias.clone(),
                        permits: group.permits,
                        gateway: limits.concurrency(),
                    });
                }
            }
        }
        warnings
    }

    /// Checks every bound (first error wins) and resolves each role's group.
    pub fn validate(&self) -> Result<(Vec<RoleRoute>, Vec<ConfigWarning>), ConfigError> {
        self.policy.validate()?;
        let mut names = BTreeSet::new();
        let mut owner = BTreeMap::new();
        for group in &self.groups {
            if group.name.trim().is_empty() {
                return Err(ConfigError::EmptyGroupName);
            }
            if !names.insert(group.name.as_str()) {
                return Err(ConfigError::DuplicateGroup(group.name.clone()));
            }
            let named = || group.name.clone();
            if !(1..=MAX_PERMITS).contains(&group.permits) {
                return Err(ConfigError::Permits {
                    group: named(),
                    value: group.permits,
                });
            }
            if !(1..=MAX_REQUESTS_PER_MIN).contains(&group.requests_per_min) {
                return Err(ConfigError::RequestsPerMin {
                    group: named(),
                    value: group.requests_per_min,
                });
            }
            let burst = group.effective_burst();
            if !(1..=MAX_BURST).contains(&burst) {
                return Err(ConfigError::Burst {
                    group: named(),
                    value: burst,
                });
            }
            if group.aliases.is_empty() {
                return Err(ConfigError::NoAliases { group: named() });
            }
            for alias in &group.aliases {
                if alias.trim().is_empty() {
                    return Err(ConfigError::EmptyAlias { group: named() });
                }
                if owner.insert(alias.as_str(), group.name.as_str()).is_some() {
                    return Err(ConfigError::DuplicateAlias {
                        alias: alias.clone(),
                    });
                }
            }
        }
        let mut routes = Vec::new();
        let mut warnings = Vec::new();
        for (&role, config) in &self.roles {
            if config.alias.trim().is_empty() {
                return Err(ConfigError::EmptyRoleAlias { role });
            }
            let group = owner.get(config.alias.as_str()).map(|g| g.to_string());
            if group.is_none() {
                warnings.push(ConfigWarning::UngroupedRole {
                    role,
                    alias: config.alias.clone(),
                });
            }
            routes.push(RoleRoute {
                role,
                alias: config.alias.clone(),
                group,
                external: config.external,
            });
        }
        Ok((routes, warnings))
    }
}

impl GovernorPolicy {
    fn validate(&self) -> Result<(), ConfigError> {
        let cooldown = |d: Duration| !d.is_zero() && d <= MAX_COOLDOWN;
        if !(1..=100).contains(&self.breaker_threshold) {
            return Err(ConfigError::Policy("breaker_threshold"));
        }
        if !cooldown(self.open_cooldown) {
            return Err(ConfigError::Policy("open_cooldown"));
        }
        if !cooldown(self.max_open_cooldown) || self.max_open_cooldown < self.open_cooldown {
            return Err(ConfigError::Policy("max_open_cooldown"));
        }
        if !cooldown(self.drain_step) {
            return Err(ConfigError::Policy("drain_step"));
        }
        if self.retry_permille > MAX_RETRY_PERMILLE {
            return Err(ConfigError::Policy("retry_permille"));
        }
        if self.retry_floor > MAX_RETRY_FLOOR {
            return Err(ConfigError::Policy("retry_floor"));
        }
        if self.retry_window.is_zero() || self.retry_window > MAX_WINDOW {
            return Err(ConfigError::Policy("retry_window"));
        }
        Ok(())
    }
}
