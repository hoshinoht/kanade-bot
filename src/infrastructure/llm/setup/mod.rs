//! Builds provider → governor → [`ModelClient`] from plain settings, so this
//! layer never reads runtime config, env or key files itself.

mod ca;
mod catalog;
mod context;
mod effort;
mod live;
mod probe;
mod settings;
mod startup;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::PathBuf,
    sync::Arc,
};

use super::{
    BearerKey, Effort, ExecutionLimits, HttpConfigError, HttpProviderConfig, LlmError,
    ModelCapabilities, OpenAiCompatibleProvider, RetryPolicy, TrustRoots,
    governor::{
        ConfigError, Governor, GovernorConfig, GroupConfig, ModelClient, Random, Role, RoleConfig,
    },
};

pub use catalog::{CatalogModel, CatalogSnapshot, Variant, leaves_homelab, variant_of};
pub use context::{ContextResolution, ContextSource, resolve_context};
pub use effort::EffortStatus;
pub use live::{RoleSwap, RunningRole};
pub use probe::{PROBE_TIMEOUT, ProbeOutcome, ProbeResult};
pub use startup::{Listing, StartupReport, StartupWarning};

use catalog::CatalogState;
use live::LiveRoles;

const GROUP: &str = "gateway";
/// Rate ceiling per permit; the gateway enforces its own admission on top.
const REQUESTS_PER_MIN_PER_PERMIT: u32 = 60;

pub type GatewayClient = ModelClient<OpenAiCompatibleProvider>;

/// What a candidate gateway key got from the model list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyCheck {
    /// Accepted; the models listed (0 when the gateway serves no list).
    Accepted(usize),
    /// The gateway refused the key.
    Refused,
    /// No answer, or one that says nothing about the key.
    Unreachable,
}

/// One `GET /models` to `base_url` with a candidate `key` (and the CA bundle
/// the gateway is trusted with), never kept: Setup's check before a save.
pub async fn check_key(base_url: &str, ca_file: Option<&std::path::Path>, key: &str) -> KeyCheck {
    let mut config = HttpProviderConfig::new(base_url);
    let Ok(bearer) = BearerKey::from_bytes(key.as_bytes()) else {
        return KeyCheck::Refused;
    };
    config.bearer_key = Some(bearer);
    if let Some(path) = ca_file {
        match ca::read(path) {
            Ok(roots) => config.trust_roots = TrustRoots::Custom(roots),
            Err(_) => return KeyCheck::Unreachable,
        }
    }
    let Ok(provider) = OpenAiCompatibleProvider::new(config) else {
        return KeyCheck::Unreachable;
    };
    match provider.list_models().await {
        Ok(models) => KeyCheck::Accepted(models.len()),
        Err(failure) if failure.reason_code == super::NO_LISTING => KeyCheck::Accepted(0),
        Err(failure) if matches!(failure.kind, super::ProviderFailureKind::Authentication) => {
            KeyCheck::Refused
        }
        Err(_) => KeyCheck::Unreachable,
    }
}

/// A role's reasoning level; `Inherit` (chat and rewrite only) takes the
/// extraction role's level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoleEffort {
    #[default]
    Inherit,
    Level(Effort),
}

/// `alias` `None` leaves the role unrouted: its sessions are refused.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleModel {
    pub alias: Option<String>,
    pub effort: RoleEffort,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRoles {
    pub extraction: RoleModel,
    pub chat: RoleModel,
    pub rewrite: RoleModel,
}

impl Default for ModelRoles {
    fn default() -> Self {
        Self {
            extraction: RoleModel {
                alias: None,
                effort: RoleEffort::Level(Effort::Off),
            },
            chat: RoleModel::default(),
            rewrite: RoleModel::default(),
        }
    }
}

impl ModelRoles {
    pub const ALL: [Role; 3] = [Role::Extraction, Role::Chat, Role::Rewrite];

    pub fn get(&self, role: Role) -> &RoleModel {
        match role {
            Role::Extraction => &self.extraction,
            Role::Chat => &self.chat,
            Role::Rewrite => &self.rewrite,
        }
    }

    fn aliases(&self) -> BTreeMap<Role, String> {
        Self::ALL
            .into_iter()
            .filter_map(|role| self.get(role).alias.clone().map(|alias| (role, alias)))
            .collect()
    }
}

/// Plain inputs, already loaded by the caller.
pub struct ModelSetup {
    /// `None` disables every model feature.
    pub base_url: Option<String>,
    pub key: Option<Vec<u8>>,
    /// PEM bundle (or one DER certificate) replacing the compiled roots.
    pub ca_file: Option<PathBuf>,
    pub roles: ModelRoles,
    pub permits: u32,
    /// Operator capability declarations per alias, used where the listing
    /// publishes no metadata for it (published metadata wins).
    pub capabilities: BTreeMap<String, ModelCapabilities>,
    /// The deployment's guild id, seeding each role's `prompt_cache_key`;
    /// `None` sends no key.
    pub cache_scope: Option<u64>,
}

/// Operator-declared backend group; aliases sharing hardware share its permits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapacityGroup {
    pub name: String,
    pub permits: u32,
    pub aliases: Vec<String>,
}

impl fmt::Debug for ModelSetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelSetup")
            .field("base_url", &self.base_url)
            .field("has_key", &self.key.is_some())
            .field("ca_file", &self.ca_file)
            .field("roles", &self.roles)
            .field("permits", &self.permits)
            .field("capabilities", &self.capabilities)
            .field("cache_scope", &self.cache_scope)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error("{0}")]
    Http(HttpConfigError),
    #[error("model CA file must be a readable PEM bundle or DER certificate")]
    CaFile,
    #[error("extraction reasoning cannot inherit")]
    ExtractionInherits,
    #[error("{0}")]
    Governor(ConfigError),
    #[error("model client: {0:?}")]
    Client(LlmError),
}

#[derive(Debug)]
pub enum Models {
    /// No base URL: extraction, chat and rewrites report unavailable.
    Unavailable,
    Ready(Box<ModelStack>),
}

#[derive(Debug)]
pub struct ModelStack {
    pub provider: Arc<OpenAiCompatibleProvider>,
    pub governor: Arc<Governor>,
    pub client: Arc<GatewayClient>,
    config: GovernorConfig,
    roles: Arc<LiveRoles>,
    /// Roles routed when the stack was built.
    started: BTreeSet<Role>,
    catalog: Arc<CatalogState>,
}

/// `https` needs `runtime::tls::install_ring_provider` first. The base URL goes
/// through the provider's own endpoint parser. Every route starts external
/// (fail closed) until a listing shows its trust zone.
pub fn build(setup: ModelSetup, random: Arc<dyn Random>) -> Result<Models, SetupError> {
    build_with_groups(setup, &[], random)
}

/// As [`build`]; non-empty `groups` replace the single `gateway` group
/// (`setup.permits` is then unused). A role whose alias is in no group is
/// reported ungrouped at startup and its calls are refused; without declared
/// groups every alias a role is later switched to joins `gateway`.
pub fn build_with_groups(
    setup: ModelSetup,
    groups: &[CapacityGroup],
    random: Arc<dyn Random>,
) -> Result<Models, SetupError> {
    let Some(base_url) = setup.base_url else {
        return Ok(Models::Unavailable);
    };
    if setup.roles.extraction.effort == RoleEffort::Inherit {
        return Err(SetupError::ExtractionInherits);
    }
    let catalog = Arc::new(CatalogState::new(setup.capabilities.clone()));
    let mut config = HttpProviderConfig::new(base_url);
    config.declared = setup.capabilities;
    config.bearer_key = setup
        .key
        .as_deref()
        .map(BearerKey::from_bytes)
        .transpose()
        .map_err(SetupError::Http)?;
    if let Some(path) = &setup.ca_file {
        config.trust_roots = TrustRoots::Custom(ca::read(path)?);
    }
    let provider = Arc::new(OpenAiCompatibleProvider::new(config).map_err(SetupError::Http)?);
    let aliases = setup.roles.aliases();
    let started = aliases.keys().copied().collect();
    let config = governor_config(&aliases, setup.permits, groups);
    let governor = Arc::new(Governor::new(&config, random).map_err(SetupError::Governor)?);
    let open = groups
        .is_empty()
        .then(|| gateway_group(setup.permits, Vec::new()));
    let roles = Arc::new(LiveRoles::new(setup.roles, open));
    roles.refresh(&governor, &catalog);
    {
        let (catalog, governor, roles) = (catalog.clone(), governor.clone(), roles.clone());
        provider.observe_listings(move |listed| roles.observe(listed, &governor, &catalog));
    }
    let mut client = ModelClient::new(
        governor.clone(),
        provider.clone(),
        ExecutionLimits::default(),
        RetryPolicy::default(),
    )
    .map_err(SetupError::Client)?;
    if let Some(scope) = setup.cache_scope {
        client = client.with_cache_scope(scope);
    }
    Ok(Models::Ready(Box::new(ModelStack {
        provider,
        governor,
        client: Arc::new(client),
        config,
        roles,
        started,
        catalog,
    })))
}

fn gateway_group(permits: u32, aliases: Vec<String>) -> GroupConfig {
    GroupConfig {
        name: GROUP.into(),
        backend: "model gateway".into(),
        permits,
        requests_per_min: permits.saturating_mul(REQUESTS_PER_MIN_PER_PERMIT),
        burst: None,
        aliases,
    }
}

fn governor_config(
    aliases: &BTreeMap<Role, String>,
    permits: u32,
    declared: &[CapacityGroup],
) -> GovernorConfig {
    let distinct: BTreeSet<&String> = aliases.values().collect();
    let groups = if !declared.is_empty() {
        declared
            .iter()
            .map(|group| GroupConfig {
                name: group.name.clone(),
                backend: "model gateway".into(),
                permits: group.permits,
                requests_per_min: group.permits.saturating_mul(REQUESTS_PER_MIN_PER_PERMIT),
                burst: None,
                aliases: group.aliases.clone(),
            })
            .collect()
    } else if distinct.is_empty() {
        Vec::new()
    } else {
        vec![gateway_group(
            permits,
            distinct.into_iter().cloned().collect(),
        )]
    };
    let roles = aliases
        .iter()
        .map(|(&role, alias)| {
            let config = RoleConfig {
                alias: alias.clone(),
                external: true,
            };
            (role, config)
        })
        .collect();
    GovernorConfig {
        groups,
        roles,
        policy: Default::default(),
    }
}

impl ModelStack {
    pub fn has_role(&self, role: Role) -> bool {
        self.governor.route(role).is_some()
    }

    /// The running roles (the last applied, else the startup ones).
    pub fn roles(&self) -> ModelRoles {
        self.roles.get()
    }

    /// The role had a model when the stack was built (serve composes
    /// extraction and the heading rewriter only for those).
    pub fn routed_at_start(&self, role: Role) -> bool {
        self.started.contains(&role)
    }

    /// Switches the running roles; the next session of each role opens
    /// with them (routing rules: `live.rs`).
    pub fn apply_roles(&self, roles: ModelRoles) -> Result<Vec<RoleSwap>, SetupError> {
        self.roles.apply(roles, &self.governor, &self.catalog)
    }

    /// Alias and effort each routed role's next session opens with.
    pub fn running(&self) -> BTreeMap<Role, RunningRole> {
        live::running(&self.governor)
    }

    /// How a route's member data leaves: local aliases stay in the homelab;
    /// every other alias receives raw data outside it.
    pub fn route_kind(&self, role: Role) -> Option<&'static str> {
        let route = self.governor.route(role)?;
        Some(if route.external {
            "external_unmasked"
        } else {
            "homelab"
        })
    }

    /// The last successful listing; empty and `listed: false` before one.
    pub fn catalog(&self) -> CatalogSnapshot {
        self.catalog.snapshot()
    }

    /// Effective level per routed role against the current listing and the
    /// operator's declarations: inherit resolved, a variant's fixed level
    /// applied, a stranded level reset to `off` or the lowest accepted level
    /// (reported in `stranded`).
    pub fn efforts(&self) -> BTreeMap<Role, EffortStatus> {
        live::efforts(
            &self.roles.get(),
            &self.catalog,
            self.catalog.listing().as_deref(),
        )
    }

    /// What a role's requests should send as `reasoning` (its live route).
    pub fn effort(&self, role: Role) -> Option<Effort> {
        self.governor.route(role)?.effort
    }
}
