//! The Models section: role aliases and reasoning levels checked against the
//! live catalog (admin-api.md "Config semantics"), and the per-group capacity check
//! of the groups the governor runs (limits-contract.md).

use std::collections::BTreeSet;

use serde_json::Value;

use super::patch::{PatchError, field_error, object};
use crate::{
    api::dto::config::CapacityCheck,
    domain::settings::{Models, Reasoning, RoleModel},
    infrastructure::llm::{
        Effort,
        setup::{CapacityGroup, CatalogModel, CatalogSnapshot},
    },
};

/// Matches the model stack's one backend group (`setup::build`).
pub const GROUP: &str = "gateway";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Extraction,
    Chat,
    Rewrite,
}

impl Role {
    const ALL: [Self; 3] = [Self::Extraction, Self::Chat, Self::Rewrite];

    fn name(self) -> &'static str {
        match self {
            Self::Extraction => "extraction",
            Self::Chat => "chat",
            Self::Rewrite => "rewrite",
        }
    }

    fn of(self, models: &Models) -> &RoleModel {
        match self {
            Self::Extraction => &models.extraction,
            Self::Chat => &models.chat,
            Self::Rewrite => &models.rewrite,
        }
    }

    fn of_mut(self, models: &mut Models) -> &mut RoleModel {
        match self {
            Self::Extraction => &mut models.extraction,
            Self::Chat => &mut models.chat,
            Self::Rewrite => &mut models.rewrite,
        }
    }
}

fn find<'a>(catalog: &'a CatalogSnapshot, alias: &str) -> Option<&'a CatalogModel> {
    catalog.models.iter().find(|model| model.alias == alias)
}

/// `off` unless the alias requires reasoning (a published list without
/// `none`); otherwise the alias must be listed and publish the level (`null`
/// efforts: Kanata restricts nothing). An unlisted alias takes only `off`.
fn legal(model: Option<&CatalogModel>, level: Reasoning) -> bool {
    let Some(model) = model else {
        return level == Reasoning::Off;
    };
    if level == Reasoning::Off {
        return model.off_allowed();
    }
    model.reasoning_efforts.as_ref().is_none_or(|efforts| {
        efforts
            .iter()
            .any(|effort| effort.as_str() == level.as_str())
    })
}

/// Published levels other than `off`, as "a, b or c".
fn required_levels(model: &CatalogModel) -> String {
    let levels: Vec<&str> = model
        .reasoning_efforts
        .iter()
        .flatten()
        .filter(|effort| **effort != Effort::Off)
        .map(|effort| effort.as_str())
        .collect();
    match levels.split_last() {
        Some((last, [])) => (*last).to_owned(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => "a level".to_owned(),
    }
}

fn accepted(alias: &str, model: Option<&CatalogModel>) -> String {
    let Some(model) = model else {
        return format!("Kanata does not list {alias}, so only off is accepted");
    };
    match &model.reasoning_efforts {
        Some(efforts) => {
            let off = model.off_allowed().then_some("off");
            let levels: Vec<&str> = off
                .into_iter()
                .chain(
                    efforts
                        .iter()
                        .filter(|effort| **effort != Effort::Off)
                        .map(|effort| effort.as_str()),
                )
                .collect();
            format!("{alias} accepts reasoning {}", levels.join(", "))
        }
        None => format!("{alias} accepts every reasoning level"),
    }
}

/// What a stranded level becomes: `off`, or the lowest published level where
/// the alias requires reasoning.
fn reset_level(model: &CatalogModel) -> Reasoning {
    model
        .reasoning_floor()
        .and_then(|floor| Reasoning::parse(floor.as_str()))
        .unwrap_or(Reasoning::Off)
}

/// What a role's requests would send: `Inherit` resolved to extraction's level.
fn resolved(models: &Models, role: Role) -> Reasoning {
    match Role::of(role, models).reasoning {
        Reasoning::Inherit => models.extraction.reasoning,
        level => level,
    }
}

/// `models.roles` merged onto `current`: explicit levels validated (422),
/// stranded levels the request did not set reset (`off`, or the alias's
/// lowest published level where it requires reasoning) with a notice.
pub fn apply_roles(
    current: &Models,
    roles: &Value,
    catalog: &CatalogSnapshot,
) -> Result<(Models, Vec<String>), PatchError> {
    let roles = object(roles, "models.roles")?;
    let mut next = current.clone();
    let mut asked = BTreeSet::new();
    for (key, body) in roles {
        let role = Role::ALL
            .into_iter()
            .find(|role| role.name() == key)
            .ok_or_else(|| PatchError::unknown(format!("models.roles.{key}")))?;
        let path = format!("models.roles.{key}");
        for (field, value) in object(body, &path)? {
            let slot = role.of_mut(&mut next);
            match field.as_str() {
                "alias" => {
                    let alias = value
                        .as_str()
                        .map(str::trim)
                        .filter(|alias| !alias.is_empty())
                        .ok_or_else(|| field_error(&format!("{path}.alias"), "a listed model"))?;
                    if slot.alias.as_deref() == Some(alias) {
                        continue;
                    }
                    // Only a changed alias is checked, so a vanished saved alias
                    // never blocks saving the other roles.
                    let model = find(catalog, alias).ok_or_else(|| {
                        PatchError::invalid(format!("Kanata does not list {alias}."))
                    })?;
                    if role == Role::Chat && !model.function_tools {
                        return Err(PatchError::invalid(format!(
                            "{alias} cannot call tools, which the chatbot needs."
                        )));
                    }
                    slot.alias = Some(alias.to_owned());
                }
                "reasoning" => {
                    let text = value
                        .as_str()
                        .ok_or_else(|| field_error(&format!("{path}.reasoning"), "a level"))?;
                    let level = Reasoning::parse(text).ok_or_else(|| {
                        PatchError::invalid(format!("{text:?} is not a reasoning level."))
                    })?;
                    if role == Role::Extraction && level == Reasoning::Inherit {
                        return Err(PatchError::invalid(
                            "Extraction sets its own reasoning; it cannot inherit.",
                        ));
                    }
                    slot.reasoning = level;
                    asked.insert(role.name());
                }
                other => return Err(PatchError::unknown(format!("{path}.{other}"))),
            }
        }
    }

    let mut notices = Vec::new();
    // A `<base>:<level>` variant bakes its level in: the stored level follows
    // it (never a 422, the app sends every role on each save). First, so
    // inheritors see extraction's fixed level.
    let mut fixed = BTreeSet::new();
    for role in Role::ALL {
        let Some(alias) = role.of(&next).alias.clone() else {
            continue;
        };
        let Some(level) = catalog
            .variant(&alias)
            .and_then(|variant| Reasoning::parse(variant.effort.as_str()))
        else {
            continue;
        };
        fixed.insert(role.name());
        let slot = role.of_mut(&mut next);
        if slot.reasoning == level {
            continue;
        }
        let was = slot.reasoning;
        slot.reasoning = level;
        notices.push(if asked.contains(role.name()) {
            let requested = match was {
                Reasoning::Inherit => "inherit".to_owned(),
                other => other.as_str().to_owned(),
            };
            format!(
                "{} reasoning is fixed at {} by {alias}; the requested {requested} is ignored.",
                role.name(),
                level.as_str()
            )
        } else {
            format!(
                "{} reasoning set to {}: {alias} fixes it.",
                role.name(),
                level.as_str()
            )
        });
    }

    // Checked after every role is applied: inheritance resolves against the
    // final extraction level and each role's final alias.
    for role in Role::ALL {
        let Some(alias) = role.of(&next).alias.clone() else {
            continue;
        };
        if fixed.contains(role.name()) {
            continue;
        }
        let model = find(catalog, &alias);
        let level = resolved(&next, role);
        if legal(model, level) {
            continue;
        }
        let requires = model.is_some_and(|model| !model.off_allowed());
        if asked.contains(role.name()) {
            let inherits = role.of(&next).reasoning == Reasoning::Inherit;
            return Err(PatchError::invalid(match (model, inherits) {
                (Some(model), true) if requires && level == Reasoning::Off => format!(
                    "{} inherits off from extraction, but {alias} requires reasoning: pick {}.",
                    role.name(),
                    required_levels(model)
                ),
                (Some(model), true) if requires => format!(
                    "{} inherits {} from extraction, which {alias} does not publish; pick {}.",
                    role.name(),
                    level.as_str(),
                    required_levels(model)
                ),
                (_, true) => format!(
                    "{} inherits {} from extraction, which {alias} does not publish; pick a level or turn reasoning off.",
                    role.name(),
                    level.as_str()
                ),
                (Some(model), false) if level == Reasoning::Off => format!(
                    "{alias} requires reasoning: pick {}.",
                    required_levels(model)
                ),
                _ => format!("{}, not {}.", accepted(&alias, model), level.as_str()),
            }));
        }
        // A saved alias Kanata no longer lists is shown as it is, not reset.
        let Some(model) = model else {
            continue;
        };
        let reset = reset_level(model);
        role.of_mut(&mut next).reasoning = reset;
        notices.push(if level == Reasoning::Off {
            format!(
                "{} reasoning set to {}: {alias} requires reasoning.",
                role.name(),
                reset.as_str()
            )
        } else {
            format!(
                "{} reasoning reset to {}: {alias} does not publish {}.",
                role.name(),
                reset.as_str(),
                level.as_str()
            )
        });
    }
    Ok((next, notices))
}

fn distinct_aliases(models: &Models) -> Vec<String> {
    let aliases: BTreeSet<&String> = Role::ALL
        .into_iter()
        .filter_map(|role| role.of(models).alias.as_ref())
        .collect();
    aliases.into_iter().cloned().collect()
}

/// The groups the governor runs: the declared ones, else one `gateway` group
/// of `permits` over the distinct role aliases (none without a role alias).
pub fn effective_groups(
    models: &Models,
    permits: u32,
    declared: &[CapacityGroup],
) -> Vec<CapacityGroup> {
    if !declared.is_empty() {
        return declared.to_vec();
    }
    let aliases = distinct_aliases(models);
    if aliases.is_empty() {
        return Vec::new();
    }
    vec![CapacityGroup {
        name: GROUP.into(),
        permits,
        aliases,
    }]
}

/// Per group: its permits must fit the smallest published admission of its
/// aliases. With `declared` groups a role alias outside them is refused by
/// the governor, so it is reported. `None` catalog: unreachable.
pub fn capacity(
    models: &Models,
    groups: &[CapacityGroup],
    declared: bool,
    catalog: Option<&CatalogSnapshot>,
) -> Vec<CapacityCheck> {
    let check = |level, message: String| CapacityCheck { level, message };
    let mut out = Vec::new();
    if declared {
        for role in Role::ALL {
            let Some(alias) = role.of(models).alias.as_ref() else {
                continue;
            };
            if !groups.iter().any(|group| group.aliases.contains(alias)) {
                out.push(check(
                    "warning",
                    format!(
                        "The {} model {alias} is in no capacity group; its calls are refused.",
                        role.name()
                    ),
                ));
            }
        }
    }
    if groups.is_empty() {
        return out;
    }
    let Some(catalog) = catalog else {
        out.push(check(
            "warning",
            "Kanata is unreachable; capacity is checked again once it lists its models.".into(),
        ));
        return out;
    };
    let mut unlisted = BTreeSet::new();
    for group in groups {
        let (name, permits) = (&group.name, group.permits);
        let mut cap: Option<(u32, &str)> = None;
        for alias in &group.aliases {
            match find(catalog, alias) {
                None => {
                    if unlisted.insert(alias) {
                        out.push(check("error", format!("Kanata does not list {alias}.")));
                    }
                }
                Some(model) => match model.admission {
                    Some(limits) => {
                        let each = limits.concurrency();
                        if cap.is_none_or(|(least, _)| each < least) {
                            cap = Some((each, alias));
                        }
                    }
                    None => out.push(check(
                        "warning",
                        format!(
                            "Kanata publishes no limit for {alias}; its calls queue at the gateway."
                        ),
                    )),
                },
            }
        }
        if let Some((cap, by)) = cap {
            out.push(if permits > cap {
                check(
                    "error",
                    format!(
                        "Group {name} declares {permits} permits but Kanata admits at most {cap} (capped by {by}); the bot refuses to start."
                    ),
                )
            } else if permits < cap {
                check(
                    "warning",
                    format!("Group {name} uses {permits} of the {cap} permits Kanata admits."),
                )
            } else {
                check(
                    "ok",
                    format!("Group {name}: {permits} permits, matching Kanata's limit."),
                )
            });
        }
    }
    out
}

/// Errors the change introduces; ones already present before it never block
/// an unrelated save.
pub fn new_errors(before: &[CapacityCheck], after: &[CapacityCheck]) -> Vec<String> {
    after
        .iter()
        .filter(|check| check.level == "error" && !before.contains(check))
        .map(|check| check.message.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::llm::AdmissionLimits;

    fn listed(alias: &str, limit: Option<u32>) -> CatalogModel {
        CatalogModel {
            alias: alias.into(),
            published: true,
            trust_zone: None,
            leaves_homelab: false,
            reasoning_control: false,
            reasoning_efforts: None,
            structured_output: true,
            sampling_controls: true,
            function_tools: true,
            context_tokens: None,
            admission: limit.map(|max| AdmissionLimits {
                max_in_flight: max,
                max_queue: None,
                queue_ms: None,
                adapter_max_in_flight: None,
            }),
        }
    }

    fn snapshot() -> CatalogSnapshot {
        CatalogSnapshot {
            listed: true,
            models: vec![
                listed("a", Some(2)),
                listed("b", Some(4)),
                listed("c", None),
            ],
        }
    }

    fn roles(extraction: &str, chat: &str) -> Models {
        let mut models = Models::default();
        models.extraction.alias = Some(extraction.into());
        models.chat.alias = Some(chat.into());
        models
    }

    fn group(name: &str, permits: u32, aliases: &[&str]) -> CapacityGroup {
        CapacityGroup {
            name: name.into(),
            permits,
            aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
        }
    }

    fn messages(checks: &[CapacityCheck]) -> Vec<(&str, &str)> {
        checks
            .iter()
            .map(|check| (check.level, check.message.as_str()))
            .collect()
    }

    #[test]
    fn default_group_covers_distinct_role_aliases_and_declared_groups_replace_it() {
        let models = roles("a", "a");
        assert_eq!(
            effective_groups(&models, 3, &[]),
            [group("gateway", 3, &["a"])]
        );
        assert!(effective_groups(&Models::default(), 3, &[]).is_empty());
        let declared = [group("local", 1, &["b"])];
        assert_eq!(effective_groups(&models, 3, &declared), declared);
    }

    #[test]
    fn each_group_is_checked_against_its_least_admitting_alias() {
        let models = roles("a", "b");
        let groups = [
            group("over", 3, &["a", "b"]),
            group("under", 1, &["b"]),
            group("equal", 2, &["a"]),
            group("open", 1, &["c"]),
        ];
        let checks = capacity(&models, &groups, true, Some(&snapshot()));
        assert_eq!(
            messages(&checks),
            [
                (
                    "error",
                    "Group over declares 3 permits but Kanata admits at most 2 (capped by a); the bot refuses to start."
                ),
                (
                    "warning",
                    "Group under uses 1 of the 4 permits Kanata admits."
                ),
                ("ok", "Group equal: 2 permits, matching Kanata's limit."),
                (
                    "warning",
                    "Kanata publishes no limit for c; its calls queue at the gateway."
                ),
            ]
        );
    }

    #[test]
    fn an_ungrouped_role_alias_warns_only_when_groups_are_declared() {
        let models = roles("a", "b");
        let groups = [group("local", 2, &["a"])];
        let declared = capacity(&models, &groups, true, Some(&snapshot()));
        assert_eq!(
            declared[0].message,
            "The chat model b is in no capacity group; its calls are refused."
        );
        let default = capacity(&models, &groups, false, Some(&snapshot()));
        assert!(
            default
                .iter()
                .all(|check| !check.message.contains("no capacity group"))
        );
    }
}
