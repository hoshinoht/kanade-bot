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
mod snapshot;

use std::{collections::BTreeMap, sync::Arc, time::Duration};

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
pub use snapshot::{
    BreakerView, GroupSnapshot, HeldPermit, PermitUsage, QueuedCall, RateLevel, RetryLevel,
};

use group::Group;

pub struct Governor {
    groups: Vec<Arc<Group>>,
    routes: BTreeMap<Role, (RoleRoute, Option<Arc<Group>>)>,
    warnings: Vec<ConfigWarning>,
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
                (route.role, (route, group))
            })
            .collect();
        Ok(Self {
            groups,
            routes,
            warnings,
        })
    }

    pub fn warnings(&self) -> &[ConfigWarning] {
        &self.warnings
    }

    pub fn route(&self, role: Role) -> Option<&RoleRoute> {
        self.routes.get(&role).map(|(route, _)| route)
    }

    fn resolve(&self, role: Role) -> Result<(&RoleRoute, Arc<Group>), Refused> {
        let (route, group) = self.routes.get(&role).ok_or(Refused::UnknownRole)?;
        let group = group.clone().ok_or(Refused::Ungrouped)?;
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
        permit::acquire(group, route.alias.clone(), ticket, wait).await
    }

    /// Takes a permit only if one is free now, nobody is queued and the breaker
    /// is closed; never waits.
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
        permit::try_acquire(group, route.alias.clone(), kind, who.into())
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
