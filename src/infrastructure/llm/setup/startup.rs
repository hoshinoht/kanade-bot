use std::{fmt, time::Duration};

use tokio::task::JoinHandle;

use super::super::{
    Effort,
    governor::{ConfigWarning, Role},
};
use super::{ModelStack, catalog};

const STARTUP_LISTING_TIMEOUT: Duration = Duration::from_secs(5);
/// Matches the provider's catalog TTL.
const REFRESH_EVERY: Duration = Duration::from_secs(300);
/// Matches the provider's failure TTL; routes stay external meanwhile.
const RETRY_EVERY: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    Listed {
        models: usize,
    },
    /// Not fatal: capabilities fall back to declared/minimal, every route is
    /// treated as external, and later listings (calls or the refresh task)
    /// re-derive both.
    Degraded {
        reason_code: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupWarning {
    Governor(ConfigWarning),
    /// The alias does not accept `effort`; requests send `sent` instead.
    UnpublishedEffort {
        role: Role,
        alias: String,
        effort: Effort,
        sent: Effort,
    },
    /// Leaves the homelab and pseudonymization is off: the operator override
    /// lets member data through in plain text.
    ExternalUnmasked {
        role: Role,
        alias: String,
    },
    /// Leaves the homelab and pseudonymization is off: calls are refused.
    ExternalRefused {
        role: Role,
        alias: String,
    },
    /// `KANADE_ALLOW_EXTERNAL_UNMASKED` is set but has no effect: masking is
    /// on, or no route leaves the homelab.
    OverrideUnused,
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
                effort: Effort::Off,
                sent,
            } => write!(
                f,
                "{} reasoning off is not allowed: {alias} requires reasoning; sending {}",
                role.as_str(),
                sent.as_str()
            ),
            Self::UnpublishedEffort {
                role,
                alias,
                effort,
                sent,
            } => write!(
                f,
                "{} reasoning {} is not published by {alias}; sending {}",
                role.as_str(),
                effort.as_str(),
                sent.as_str()
            ),
            Self::ExternalUnmasked { role, alias } => write!(
                f,
                "UNMASKED: {} model {alias} leaves the homelab and member data is sent \
                 without pseudonymization (KANADE_ALLOW_EXTERNAL_UNMASKED)",
                role.as_str()
            ),
            Self::OverrideUnused => f.write_str(
                "KANADE_ALLOW_EXTERNAL_UNMASKED is set but unused: pseudonymization is on \
                 or no model leaves the homelab",
            ),
            Self::ExternalRefused { role, alias } => write!(
                f,
                "{} model {alias} leaves the homelab; its calls are refused while \
                 pseudonymization is off",
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

impl ModelStack {
    /// One bounded listing, then capacity, reasoning and trust-zone checks.
    /// Never fails. Reasoning is checked only against a listing; trust is
    /// reported either way (degraded ⇒ every route is external).
    pub async fn check_startup(&self) -> StartupReport {
        let mut warnings: Vec<StartupWarning> = self
            .governor
            .warnings()
            .iter()
            .cloned()
            .map(StartupWarning::Governor)
            .collect();
        let listing = match tokio::time::timeout(
            STARTUP_LISTING_TIMEOUT,
            self.provider.list_models(),
        )
        .await
        {
            Ok(Ok(listed)) => {
                let published = |alias: &str| {
                    catalog::published(&listed, alias).and_then(|caps| caps.admission)
                };
                warnings.extend(
                    self.config
                        .capacity_warnings(published)
                        .into_iter()
                        .map(StartupWarning::Governor),
                );
                Listing::Listed {
                    models: listed.len(),
                }
            }
            Ok(Err(failure)) => Listing::Degraded {
                reason_code: failure.reason_code,
            },
            Err(_) => Listing::Degraded {
                reason_code: "timeout",
            },
        };
        let efforts = self.efforts();
        let mut any_external = false;
        for role in super::ModelRoles::ALL {
            let Some(route) = self.governor.route(role) else {
                continue;
            };
            if let Some(status) = efforts.get(&role)
                && let Some(effort) = status.stranded
            {
                warnings.push(StartupWarning::UnpublishedEffort {
                    role,
                    alias: route.alias.clone(),
                    effort,
                    sent: status.effort,
                });
            }
            any_external |= route.external;
            if route.external && !self.masking {
                let (role, alias) = (role, route.alias);
                warnings.push(if route.unmasked_allowed {
                    StartupWarning::ExternalUnmasked { role, alias }
                } else {
                    StartupWarning::ExternalRefused { role, alias }
                });
            }
        }
        if self.unmasked_override && (self.masking || !any_external) {
            warnings.push(StartupWarning::OverrideUnused);
        }
        StartupReport { listing, warnings }
    }

    /// Relists every 300 s (30 s until a listing has succeeded) so trust zones
    /// and the catalog stay current even while every route is refused and no
    /// call would trigger a listing. Abort the handle on shutdown.
    pub fn spawn_catalog_refresh(&self) -> JoinHandle<()> {
        let provider = self.provider.clone();
        let catalog = self.catalog.clone();
        tokio::spawn(async move {
            loop {
                let wait = if catalog.listing().is_some() {
                    REFRESH_EVERY
                } else {
                    RETRY_EVERY
                };
                tokio::time::sleep(wait).await;
                let _ = tokio::time::timeout(STARTUP_LISTING_TIMEOUT, provider.list_models()).await;
            }
        })
    }
}
