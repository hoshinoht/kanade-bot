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
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
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
    Charge, DEFAULT_TOOL_ROUNDS, MAX_TOOL_ROUNDS, ModelClient, QuestionLimits, SentRequest,
    Session, SessionError, SessionFailure,
};
pub use snapshot::{
    BreakerView, GroupSnapshot, HeldPermit, PermitUsage, QueuedCall, RateLevel, RetryLevel,
};

pub(in crate::infrastructure::llm) use jitter::full as full_jitter;

use group::Group;

use super::{AdmissionLimits, Effort};

/// What [`Governor::reroute`] points a role at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteTarget {
    pub alias: String,
    pub effort: Option<Effort>,
    /// `None`: no listing names the zone yet.
    pub external: Option<bool>,
}

impl RouteTarget {
    /// No effort, zone unknown.
    pub fn alias(alias: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
            effort: None,
            external: None,
        }
    }
}

/// Replaced whole on a reroute; permits already taken keep their own group
/// and alias.
struct Route {
    route: RoleRoute,
    group: Option<Arc<Group>>,
}

pub struct Governor {
    groups: RwLock<Vec<Arc<Group>>>,
    routes: RwLock<BTreeMap<Role, Route>>,
    unmasked_allowed: AtomicBool,
    policy: GovernorPolicy,
    warnings: Vec<ConfigWarning>,
    random: Arc<dyn Random>,
}

impl std::fmt::Debug for Governor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // One lock at a time: `reroute` takes routes, then groups.
        let groups = self.read_groups().len();
        let roles = self.read_routes().len();
        f.debug_struct("Governor")
            .field("groups", &groups)
            .field("roles", &roles)
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
                (route.role, Route { route, group })
            })
            .collect();
        Ok(Self {
            groups: RwLock::new(groups),
            routes: RwLock::new(routes),
            unmasked_allowed: AtomicBool::new(false),
            policy: config.policy.clone(),
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

    /// Startup warnings only; a later reroute adds none.
    pub fn warnings(&self) -> &[ConfigWarning] {
        &self.warnings
    }

    fn read_routes(&self) -> RwLockReadGuard<'_, BTreeMap<Role, Route>> {
        self.routes.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write_routes(&self) -> RwLockWriteGuard<'_, BTreeMap<Role, Route>> {
        self.routes.write().unwrap_or_else(PoisonError::into_inner)
    }

    fn read_groups(&self) -> RwLockReadGuard<'_, Vec<Arc<Group>>> {
        self.groups.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// Operator override read by the identity route guard on every session.
    pub fn allow_external_unmasked(&self, allowed: bool) {
        self.unmasked_allowed.store(allowed, Ordering::Release);
    }

    /// The role's route as of now (alias, effort and `external` may change
    /// live; a session keeps the alias it opened with).
    pub fn route(&self, role: Role) -> Option<RoleRoute> {
        self.read_routes().get(&role).map(|entry| RoleRoute {
            unmasked_allowed: self.unmasked_allowed.load(Ordering::Acquire),
            ..entry.route.clone()
        })
    }

    /// Returns false for an unconfigured role.
    pub fn set_external(&self, role: Role, external: bool) -> bool {
        self.write_routes()
            .get_mut(&role)
            .map(|entry| entry.route.external = external)
            .is_some()
    }

    /// Re-derives every route's `external` flag from its current alias in one
    /// step, so a concurrent reroute never inherits another alias's zone.
    pub fn rederive_external(&self, external: impl Fn(&str) -> bool) {
        for entry in self.write_routes().values_mut() {
            entry.route.external = external(&entry.route.alias);
        }
    }

    /// Returns false for an unconfigured role.
    pub fn set_effort(&self, role: Role, effort: Option<Effort>) -> bool {
        self.write_routes()
            .get_mut(&role)
            .map(|entry| entry.route.effort = effort)
            .is_some()
    }

    /// Installs every target in one routes write, so no reader sees a new
    /// alias with the old effort or zone (`None` unroutes a role); open
    /// sessions keep their permit's alias and group. A target's `external`
    /// `None` keeps the current zone for the same alias and is `true` (fail
    /// closed) for a new one. A new alias takes the group that lists it; one
    /// no group lists joins `open` (the default single group, created on
    /// first use) and leaves it when no role uses it; without `open` it is
    /// ungrouped and its calls are refused, as at startup. Checked before
    /// anything moves. Returns the roles whose alias changed.
    pub fn reroute(
        &self,
        targets: impl IntoIterator<Item = (Role, Option<RouteTarget>)>,
        open: Option<&GroupConfig>,
    ) -> Result<Vec<Role>, ConfigError> {
        let mut targets: Vec<(Role, Option<RouteTarget>)> = targets.into_iter().collect();
        for (role, target) in &mut targets {
            if let Some(target) = target {
                target.alias = target.alias.trim().to_owned();
                if target.alias.is_empty() {
                    return Err(ConfigError::EmptyRoleAlias { role: *role });
                }
            }
        }
        let mut routes = self.write_routes();
        let mut changed = Vec::new();
        for (role, target) in targets {
            let current = routes.get(&role);
            if current.map(|entry| &entry.route.alias) != target.as_ref().map(|t| &t.alias) {
                changed.push(role);
            }
            let Some(target) = target else {
                routes.remove(&role);
                continue;
            };
            let entry = match current {
                Some(entry) if entry.route.alias == target.alias => Route {
                    route: RoleRoute {
                        external: target.external.unwrap_or(entry.route.external),
                        effort: target.effort,
                        ..entry.route.clone()
                    },
                    group: entry.group.clone(),
                },
                _ => {
                    let group = self.group_for(&target.alias, open);
                    Route {
                        route: RoleRoute {
                            role,
                            alias: target.alias,
                            group: group.as_ref().map(|group| group.name.clone()),
                            external: target.external.unwrap_or(true),
                            unmasked_allowed: false,
                            effort: target.effort,
                        },
                        group,
                    }
                }
            };
            routes.insert(role, entry);
        }
        self.sync_open(&routes, open);
        Ok(changed)
    }

    /// The group listing `alias`, else `open` (created on first use).
    fn group_for(&self, alias: &str, open: Option<&GroupConfig>) -> Option<Arc<Group>> {
        let mut groups = self.groups.write().unwrap_or_else(PoisonError::into_inner);
        let listed = groups
            .iter()
            .find(|group| group.aliases().iter().any(|known| known == alias))
            .cloned();
        match (listed, open) {
            (Some(group), _) => Some(group),
            (None, Some(open)) => {
                let existing = groups.iter().find(|group| group.name == open.name).cloned();
                Some(existing.unwrap_or_else(|| {
                    let config = GroupConfig {
                        aliases: vec![alias.to_owned()],
                        ..open.clone()
                    };
                    let group = Arc::new(Group::new(
                        &config,
                        &self.policy,
                        self.random.clone(),
                        Instant::now(),
                    ));
                    groups.push(group.clone());
                    group
                }))
            }
            (None, None) => None,
        }
    }

    /// The open group lists exactly the aliases routed to it.
    fn sync_open(&self, routes: &BTreeMap<Role, Route>, open: Option<&GroupConfig>) {
        let Some(open) = open else {
            return;
        };
        let groups = self.read_groups();
        let Some(group) = groups.iter().find(|group| group.name == open.name) else {
            return;
        };
        let aliases: BTreeSet<&String> = routes
            .values()
            .filter(|entry| entry.route.group.as_deref() == Some(open.name.as_str()))
            .map(|entry| &entry.route.alias)
            .collect();
        group.set_aliases(aliases.into_iter().cloned().collect());
    }

    fn resolve(&self, role: Role) -> Result<(RoleRoute, Arc<Group>), Refused> {
        let routes = self.read_routes();
        let entry = routes.get(&role).ok_or(Refused::UnknownRole)?;
        let group = entry.group.clone().ok_or(Refused::Ungrouped)?;
        let route = RoleRoute {
            unmasked_allowed: self.unmasked_allowed.load(Ordering::Acquire),
            ..entry.route.clone()
        };
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

    /// As [`Self::acquire`] on a route read earlier (the one the identity
    /// guard checked): its alias and group, even if the role moved since.
    pub async fn acquire_route(
        &self,
        route: &RoleRoute,
        ticket: Ticket,
        wait: Duration,
    ) -> Result<Permit, Refused> {
        if !ticket.kind.may_wait() {
            return Err(Refused::MustNotWait);
        }
        let group = route
            .group
            .as_ref()
            .and_then(|name| {
                self.read_groups()
                    .iter()
                    .find(|group| &group.name == name)
                    .cloned()
            })
            .ok_or(Refused::Ungrouped)?;
        permit::acquire(group, route.alias.clone(), ticket, wait).await
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
        self.read_groups()
            .iter()
            .map(|group| snapshot::capture(group, now, wall_now))
            .collect()
    }
}
