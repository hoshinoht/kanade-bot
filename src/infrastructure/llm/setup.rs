//! Builds provider → governor → [`ModelClient`] from plain settings, so this
//! layer never reads runtime config, env or key files itself.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::Read,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use rustls::pki_types::{CertificateDer, pem::PemObject};

use super::{
    BearerKey, Effort, ExecutionLimits, HttpConfigError, HttpProviderConfig, ListedModel, LlmError,
    OpenAiCompatibleProvider, RetryPolicy, TrustRoots,
    governor::{
        ConfigError, ConfigWarning, Governor, GovernorConfig, GroupConfig, ModelClient, Random,
        Role, RoleConfig,
    },
};

const GROUP: &str = "gateway";
/// Rate ceiling per permit; the gateway enforces its own admission on top.
const REQUESTS_PER_MIN_PER_PERMIT: u32 = 60;
const MAX_CA_BYTES: u64 = 1_048_576;
const STARTUP_LISTING_TIMEOUT: Duration = Duration::from_secs(5);

pub type GatewayClient = ModelClient<OpenAiCompatibleProvider>;

/// Plain inputs, already loaded by the caller.
pub struct ModelSetup {
    /// `None` disables every model feature.
    pub base_url: Option<String>,
    pub key: Option<Vec<u8>>,
    /// PEM bundle (or one DER certificate) replacing the compiled roots.
    pub ca_file: Option<PathBuf>,
    pub aliases: BTreeMap<Role, String>,
    pub permits: u32,
}

impl fmt::Debug for ModelSetup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelSetup")
            .field("base_url", &self.base_url)
            .field("has_key", &self.key.is_some())
            .field("ca_file", &self.ca_file)
            .field("aliases", &self.aliases)
            .field("permits", &self.permits)
            .finish()
    }
}

#[derive(Debug)]
pub enum SetupError {
    Http(HttpConfigError),
    CaFile,
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
    Ready(ModelStack),
}

#[derive(Debug)]
pub struct ModelStack {
    pub provider: Arc<OpenAiCompatibleProvider>,
    pub governor: Arc<Governor>,
    pub client: Arc<GatewayClient>,
    config: GovernorConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    Listed {
        models: usize,
    },
    /// Not fatal: capabilities fall back to declared/minimal and each call
    /// refetches the listing under the provider's cache rules.
    Degraded {
        reason_code: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupWarning {
    Governor(ConfigWarning),
    /// Every call with this effort would be refused before sending.
    UnpublishedEffort {
        role: Role,
        alias: String,
        effort: Effort,
    },
    /// Listed in an external trust zone but not routed as external (only a
    /// `-cloud` suffix marks a route external at build time).
    ExternalUnmarked {
        role: Role,
        alias: String,
    },
}

impl fmt::Display for StartupWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Governor(ConfigWarning::UngroupedRole { role, alias }) => {
                write!(f, "{} model {alias} is in no group", role.as_str())
            }
            Self::Governor(ConfigWarning::PermitsAboveGateway {
                group,
                alias,
                permits,
                gateway,
            }) => write!(
                f,
                "group {group} holds {permits} permits but the gateway admits {gateway} for {alias}"
            ),
            Self::UnpublishedEffort {
                role,
                alias,
                effort,
            } => write!(
                f,
                "{} reasoning {} is not published by {alias}",
                role.as_str(),
                effort.as_str()
            ),
            Self::ExternalUnmarked { role, alias } => write!(
                f,
                "{} model {alias} is listed as external but not routed as external",
                role.as_str()
            ),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupReport {
    pub listing: Listing,
    pub warnings: Vec<StartupWarning>,
}

/// `https` needs `runtime::tls::install_ring_provider` first. The base URL goes
/// through the provider's own endpoint parser.
pub fn build(setup: ModelSetup, random: Arc<dyn Random>) -> Result<Models, SetupError> {
    let Some(base_url) = setup.base_url else {
        return Ok(Models::Unavailable);
    };
    let mut config = HttpProviderConfig::new(base_url);
    config.bearer_key = setup
        .key
        .as_deref()
        .map(BearerKey::from_bytes)
        .transpose()
        .map_err(SetupError::Http)?;
    if let Some(path) = &setup.ca_file {
        config.trust_roots = TrustRoots::Custom(read_ca(path)?);
    }
    let provider = Arc::new(OpenAiCompatibleProvider::new(config).map_err(SetupError::Http)?);
    let config = governor_config(&setup.aliases, setup.permits);
    let governor = Arc::new(Governor::new(&config, random).map_err(SetupError::Governor)?);
    let client = ModelClient::new(
        governor.clone(),
        provider.clone(),
        ExecutionLimits::default(),
        RetryPolicy::default(),
    )
    .map_err(SetupError::Client)?;
    Ok(Models::Ready(ModelStack {
        provider,
        governor,
        client: Arc::new(client),
        config,
    }))
}

fn governor_config(aliases: &BTreeMap<Role, String>, permits: u32) -> GovernorConfig {
    let distinct: BTreeSet<&String> = aliases.values().collect();
    let groups = if distinct.is_empty() {
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
            let external = alias.ends_with("-cloud");
            let config = RoleConfig {
                alias: alias.clone(),
                external,
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

fn read_ca(path: &std::path::Path) -> Result<Vec<CertificateDer<'static>>, SetupError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(MAX_CA_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| SetupError::CaFile)?;
    if bytes.len() as u64 > MAX_CA_BYTES {
        return Err(SetupError::CaFile);
    }
    let pem: Result<Vec<_>, _> = CertificateDer::pem_slice_iter(&bytes).collect();
    match pem {
        Ok(certs) if !certs.is_empty() => Ok(certs),
        // A DER certificate starts with a SEQUENCE tag.
        Ok(_) if bytes.first() == Some(&0x30) => Ok(vec![CertificateDer::from(bytes)]),
        _ => Err(SetupError::CaFile),
    }
}

impl ModelStack {
    pub fn has_role(&self, role: Role) -> bool {
        self.governor.route(role).is_some()
    }

    /// One bounded listing, then capacity, reasoning and trust-zone checks.
    /// Never fails; reasoning is left unchecked while the listing is degraded
    /// (the runner still refuses an unpublished effort per call).
    pub async fn check_startup(&self, efforts: &BTreeMap<Role, Effort>) -> StartupReport {
        let mut warnings: Vec<StartupWarning> = self
            .governor
            .warnings()
            .iter()
            .cloned()
            .map(StartupWarning::Governor)
            .collect();
        let listed = match tokio::time::timeout(
            STARTUP_LISTING_TIMEOUT,
            self.provider.list_models(),
        )
        .await
        {
            Ok(Ok(listed)) => listed,
            Ok(Err(failure)) => {
                return StartupReport {
                    listing: Listing::Degraded {
                        reason_code: failure.reason_code,
                    },
                    warnings,
                };
            }
            Err(_) => {
                return StartupReport {
                    listing: Listing::Degraded {
                        reason_code: "timeout",
                    },
                    warnings,
                };
            }
        };
        let published = |alias: &str| find(&listed, alias).and_then(|caps| caps.admission);
        warnings.extend(
            self.config
                .capacity_warnings(published)
                .into_iter()
                .map(StartupWarning::Governor),
        );
        for role in [Role::Extraction, Role::Chat, Role::Rewrite] {
            let Some(route) = self.governor.route(role) else {
                continue;
            };
            let capabilities = self.provider.model_capabilities(&route.alias).await;
            if let Err(refused) = crate::extract::pipeline::check_reasoning_effort(
                &route.alias,
                efforts.get(&role).copied(),
                &capabilities,
            ) {
                warnings.push(StartupWarning::UnpublishedEffort {
                    role,
                    alias: refused.alias,
                    effort: refused.effort,
                });
            }
            if !route.external && capabilities.is_cloud(&route.alias) {
                warnings.push(StartupWarning::ExternalUnmarked {
                    role,
                    alias: route.alias.clone(),
                });
            }
        }
        StartupReport {
            listing: Listing::Listed {
                models: listed.len(),
            },
            warnings,
        }
    }
}

fn find<'a>(listed: &'a [ListedModel], alias: &str) -> Option<&'a super::ModelCapabilities> {
    listed
        .iter()
        .find(|model| model.id == alias)
        .and_then(|model| model.capabilities.as_ref())
}
