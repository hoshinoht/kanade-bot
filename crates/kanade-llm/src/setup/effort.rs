use std::collections::BTreeMap;

use super::super::{Effort, ModelCapabilities, governor::Role};
use super::{ModelRoles, RoleEffort};

/// The configured extraction effort is one the model does not publish, so
/// every call would be refused before sending (`D-SHAPING`: refused, never
/// silently dropped).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("extraction reasoning {} is not published by {alias}", .effort.as_str())]
pub struct UnpublishedEffort {
    pub alias: String,
    pub effort: Effort,
}

/// Startup/config-change check mirroring the runner's shaping rule: an effort
/// other than `off` on a model with reasoning control and a published list
/// must be in that list.
pub fn check_reasoning_effort(
    alias: &str,
    effort: Option<Effort>,
    capabilities: &ModelCapabilities,
) -> Result<(), UnpublishedEffort> {
    let Some(effort) = effort.filter(|effort| *effort != Effort::Off) else {
        return Ok(());
    };
    let refused = capabilities.reasoning_control
        && capabilities
            .reasoning_efforts
            .as_ref()
            .is_some_and(|published| !published.contains(&effort));
    if refused {
        return Err(UnpublishedEffort {
            alias: alias.to_owned(),
            effort,
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffortStatus {
    /// What requests send.
    pub effort: Effort,
    /// The configured (or inherited) level the alias does not accept; it is
    /// replaced by `off`, or by the lowest published level where the alias
    /// requires reasoning, so calls are not refused before sending.
    pub stranded: Option<Effort>,
}

/// `capabilities` is `None` for an alias with nothing published or declared
/// (or no listing yet and no declaration): the level is kept, and the runner
/// still refuses per call if needed. `fixed` is a listed `<base>:<level>`
/// variant's baked-in level, which wins over the configured one (inheritors
/// of extraction get it too).
pub(super) fn resolve(
    roles: &ModelRoles,
    capabilities: impl Fn(&str) -> Option<ModelCapabilities>,
    fixed: impl Fn(&str) -> Option<Effort>,
) -> BTreeMap<Role, EffortStatus> {
    let configured = match roles.extraction.effort {
        RoleEffort::Level(effort) => effort,
        RoleEffort::Inherit => Effort::Off,
    };
    let base = roles
        .extraction
        .alias
        .as_deref()
        .and_then(&fixed)
        .unwrap_or(configured);
    ModelRoles::ALL
        .into_iter()
        .filter_map(|role| {
            let model = roles.get(role);
            let alias = model.alias.as_deref()?;
            let wanted = fixed(alias).unwrap_or(match model.effort {
                RoleEffort::Level(effort) => effort,
                RoleEffort::Inherit => base,
            });
            let fallback = capabilities(alias).and_then(|caps| {
                let legal = if wanted == Effort::Off {
                    caps.off_allowed()
                } else {
                    check_reasoning_effort(alias, Some(wanted), &caps).is_ok()
                };
                (!legal).then(|| caps.reasoning_floor().unwrap_or(Effort::Off))
            });
            let status = match fallback {
                None => EffortStatus {
                    effort: wanted,
                    stranded: None,
                },
                Some(effort) => EffortStatus {
                    effort,
                    stranded: Some(wanted),
                },
            };
            Some((role, status))
        })
        .collect()
}
