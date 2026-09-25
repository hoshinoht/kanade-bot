//! Builds provider → governor → [`ModelClient`] from plain settings, so this
//! layer never reads runtime config, env or key files itself.

mod ca;
mod catalog;
mod effort;
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
    OpenAiCompatibleProvider, RetryPolicy, TrustRoots,
    governor::{
        ConfigError, Governor, GovernorConfig, GroupConfig, ModelClient, Random, Role, RoleConfig,
    },
};

pub use catalog::{CatalogModel, CatalogSnapshot, Variant, leaves_homelab, variant_of};
pub use effort::EffortStatus;
pub use probe::{PROBE_TIMEOUT, ProbeOutcome, ProbeResult};
pub use startup::{Listing, StartupReport, StartupWarning};

use catalog::CatalogState;

const GROUP: &str = "gateway";
/// Rate ceiling per permit; the gateway enforces its own admission on top.
const REQUESTS_PER_MIN_PER_PERMIT: u32 = 60;

pub type GatewayClient = ModelClient<OpenAiCompatibleProvider>;

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
    /// Lets external routes run without pseudonymization (provider testing).
    pub allow_external_unmasked: bool,
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
            .field("allow_external_unmasked", &self.allow_external_unmasked)
            .finish()
    }
}

#[derive(Debug)]
pub enum SetupError {
    Http(HttpConfigError),
    CaFile,
    ExtractionInherits,
    Governor(ConfigError),
    Client(LlmError),
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => error.fmt(f),
            Self::CaFile => {
                f.write_str("model CA file must be a readable PEM bundle or DER certificate")
            }
            Self::ExtractionInherits => f.write_str("extraction reasoning cannot inherit"),
            Self::Governor(error) => error.fmt(f),
            Self::Client(error) => write!(f, "model client: {error:?}"),
        }
    }
}

impl std::error::Error for SetupError {}

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
    roles: ModelRoles,
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
/// reported ungrouped at startup and its calls are refused.
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
    let mut config = HttpProviderConfig::new(base_url);
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
    let config = governor_config(&aliases, setup.permits, groups);
    let governor = Arc::new(Governor::new(&config, random).map_err(SetupError::Governor)?);
    governor.allow_external_unmasked(setup.allow_external_unmasked);
    let catalog = Arc::new(CatalogState::default());
    {
        let (catalog, governor) = (catalog.clone(), governor.clone());
        provider.observe_listings(move |listed| catalog.apply(listed, &governor, &aliases));
    }
    let client = ModelClient::new(
        governor.clone(),
        provider.clone(),
        ExecutionLimits::default(),
        RetryPolicy::default(),
    )
    .map_err(SetupError::Client)?;
    Ok(Models::Ready(Box::new(ModelStack {
        provider,
        governor,
        client: Arc::new(client),
        config,
        roles: setup.roles,
        catalog,
    })))
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
        vec![GroupConfig {
            name: GROUP.into(),
            backend: "model gateway".into(),
            permits,
            requests_per_min: permits.saturating_mul(REQUESTS_PER_MIN_PER_PERMIT),
            burst: None,
            aliases: distinct.into_iter().cloned().collect(),
        }]
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

    pub fn roles(&self) -> &ModelRoles {
        &self.roles
    }

    /// The last successful listing; empty and `listed: false` before one.
    pub fn catalog(&self) -> CatalogSnapshot {
        self.catalog.snapshot()
    }

    /// Effective level per routed role against the current listing: inherit
    /// resolved, a variant's fixed level applied, a stranded level reset to
    /// `off` or the lowest published level (reported in `stranded`).
    pub fn efforts(&self) -> BTreeMap<Role, EffortStatus> {
        let listing = self.catalog.listing();
        let listed = |alias: &str| {
            listing
                .as_deref()
                .is_some_and(|models| models.iter().any(|model| model.id == alias))
        };
        effort::resolve(
            &self.roles,
            |alias| catalog::published(listing.as_deref()?, alias).cloned(),
            |alias| variant_of(alias, listed).map(|variant| variant.effort),
        )
    }

    /// What a role's requests should send as `reasoning`.
    pub fn effort(&self, role: Role) -> Option<Effort> {
        self.efforts().get(&role).map(|status| status.effort)
    }
}
