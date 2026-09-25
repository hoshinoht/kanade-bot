//! Model traffic governor: every model call takes a permit from its backend
//! group's priority queue and admits each request through the group's rate
//! ceiling, retry budget and circuit breaker. Time is tokio's clock (paused in
//! tests); wall time is passed in only for snapshots; jitter uses an injected
//! `Random`.

mod breaker;
mod budget;
mod config;
mod group;
mod jitter;
mod permit;
mod pool;
mod rate;
mod session;
mod snapshot;

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use chrono::{DateTime, Utc};
use tokio::time::Instant;

pub use breaker::BreakerState;
pub use config::{
    ConfigError, ConfigWarning, GovernorConfig, GovernorPolicy, GroupConfig, MAX_BURST,
    MAX_PERMITS, MAX_REQUESTS_PER_MIN, MAX_RETRY_PERMILLE, Role, RoleConfig, RoleRoute,
};
pub use group::Counters;
pub use jitter::{Random, XorShift};
pub use permit::{Attempt, Outcome, Permit, Refused, Ticket};
pub use pool::{CallKind, Priority};
pub use session::{
    Charge, DEFAULT_TOOL_ROUNDS, MAX_TOOL_ROUNDS, ModelClient, QuestionLimits, Session,
    SessionError, SessionFailure,
};
pub use snapshot::{
    BreakerView, GroupSnapshot, HeldPermit, PermitUsage, QueuedCall, RateLevel, RetryLevel,
};

pub(in crate::infrastructure::llm) use jitter::full as full_jitter;

use group::Group;

use super::AdmissionLimits;

/// `external` is atomic so a refreshed listing can re-derive it in place.
struct Route {
    route: RoleRoute,
    external: AtomicBool,
    group: Option<Arc<Group>>,
}

pub struct Governor {
    groups: Vec<Arc<Group>>,
    routes: BTreeMap<Role, Route>,
    unmasked_allowed: AtomicBool,
    warnings: Vec<ConfigWarning>,
    random: Arc<dyn Random>,
}

impl std::fmt::Debug for Governor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Governor")
            .field("groups", &self.groups.len())
            .field("roles", &self.routes.len())
            .finish()
    }
}

impl Governor {
    pub fn new(config: &GovernorConfig, random: Arc<dyn Random>) -> Result<Self, ConfigError> {
        let (routes, warnings) = config.validate()?;
        let now = Instant::now();
        let groups: Vec<Arc<Group>> = config
            .groups
            .iter()
            .map(|group| Arc::new(Group::new(group, &config.policy, random.clone(), now)))
            .collect();
        let routes = routes
            .into_iter()
            .map(|route| {
                let group = route
                    .group
                    .as_ref()
                    .and_then(|name| groups.iter().find(|g| &g.name == name).cloned());
                let external = AtomicBool::new(route.external);
                (
                    route.role,
                    Route {
                        route,
                        external,
                        group,
                    },
                )
            })
            .collect();
        Ok(Self {
            groups,
            routes,
            unmasked_allowed: AtomicBool::new(false),
            warnings,
            random,
        })
    }

    /// Startup path once the gateway listing is known: `new` plus
    /// [`GovernorConfig::capacity_warnings`] in `warnings()`.
    pub fn new_checked(
        config: &GovernorConfig,
        random: Arc<dyn Random>,
        published: impl Fn(&str) -> Option<AdmissionLimits>,
    ) -> Result<Self, ConfigError> {
        let mut governor = Self::new(config, random)?;
        governor
            .warnings
            .extend(config.capacity_warnings(published));
        Ok(governor)
    }

    pub fn warnings(&self) -> &[ConfigWarning] {
        &self.warnings
    }

    /// Operator override read by the identity route guard on every session.
    pub fn allow_external_unmasked(&self, allowed: bool) {
        self.unmasked_allowed.store(allowed, Ordering::Release);
    }

    /// The role's route as of now (`external` may change with the listing).
    pub fn route(&self, role: Role) -> Option<RoleRoute> {
        self.routes.get(&role).map(|entry| RoleRoute {
            external: entry.external.load(Ordering::Acquire),
            unmasked_allowed: self.unmasked_allowed.load(Ordering::Acquire),
            ..entry.route.clone()
        })
    }

    /// Returns false for an unconfigured role.
    pub fn set_external(&self, role: Role, external: bool) -> bool {
        self.routes
            .get(&role)
            .map(|entry| entry.external.store(external, Ordering::Release))
            .is_some()
    }

    fn resolve(&self, role: Role) -> Result<(RoleRoute, Arc<Group>), Refused> {
        let route = self.route(role).ok_or(Refused::UnknownRole)?;
        let group = self.routes[&role].group.clone().ok_or(Refused::Ungrouped)?;
        Ok((route, group))
    }

    /// Queues for a permit in the role's group for at most `wait`. Refused at
    /// once while the group's breaker is open; dropping the future leaves the queue.
    pub async fn acquire(
        &self,
        role: Role,
        ticket: Ticket,
        wait: Duration,
    ) -> Result<Permit, Refused> {
        if !ticket.kind.may_wait() {
            return Err(Refused::MustNotWait);
        }
        let (route, group) = self.resolve(role)?;
        permit::acquire(group, route.alias, ticket, wait).await
    }

    /// Takes a permit only if one is free now, nobody is queued and the breaker
    /// is closed or half-open without a probe in flight (the holder's first
    /// request then probes); never waits.
    pub fn try_acquire(
        &self,
        role: Role,
        kind: CallKind,
        who: impl Into<String>,
    ) -> Result<Permit, Refused> {
        let (route, group) = self.resolve(role)?;
        if kind == CallKind::PreScreen && route.external {
            return Err(Refused::ExternalForbidden);
        }
        permit::try_acquire(group, route.alias, kind, who.into())
    }

    /// Groups in configuration order; `wall_now` anchors the timestamps.
    pub fn snapshot(&self, wall_now: DateTime<Utc>) -> Vec<GroupSnapshot> {
        let now = Instant::now();
        self.groups
            .iter()
            .map(|group| snapshot::capture(group, now, wall_now))
            .collect()
    }
}
