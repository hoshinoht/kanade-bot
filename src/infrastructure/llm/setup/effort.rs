use std::collections::BTreeMap;

use super::super::{Effort, ModelCapabilities, governor::Role};
use super::{ModelRoles, RoleEffort};
use crate::extract::pipeline::check_reasoning_effort;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffortStatus {
    /// What requests send.
    pub effort: Effort,
    /// The configured (or inherited) level the alias does not publish; it
    /// is replaced by `off` so calls are not refused before sending.
    pub stranded: Option<Effort>,
}

/// `published` is `None` for an alias with nothing published (or no listing
/// yet): the level is kept, and the runner still refuses per call if needed.
pub(super) fn resolve(
    roles: &ModelRoles,
    published: impl Fn(&str) -> Option<ModelCapabilities>,
) -> BTreeMap<Role, EffortStatus> {
    let base = match roles.extraction.effort {
        RoleEffort::Level(effort) => effort,
        RoleEffort::Inherit => Effort::Off,
    };
    ModelRoles::ALL
        .into_iter()
        .filter_map(|role| {
            let model = roles.get(role);
            let alias = model.alias.as_deref()?;
            let wanted = match model.effort {
                RoleEffort::Level(effort) => effort,
                RoleEffort::Inherit => base,
            };
            let legal = published(alias)
                .is_none_or(|caps| check_reasoning_effort(alias, Some(wanted), &caps).is_ok());
            let status = if legal {
                EffortStatus {
                    effort: wanted,
                    stranded: None,
                }
            } else {
                EffortStatus {
                    effort: Effort::Off,
                    stranded: Some(wanted),
                }
            };
            Some((role, status))
        })
        .collect()
}
