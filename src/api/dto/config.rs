//! `config.json#/$defs/ConfigView` and its projections from runtime settings,
//! the live model catalog, the persona files and deployment facts.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    chat::persona::PersonaSnapshot,
    domain::settings::{Rate as StoredRate, RoleModel as StoredRole, RuntimeSettings},
    infrastructure::llm::{
        TrustZone,
        governor::Role,
        setup::{CatalogModel, CatalogSnapshot, RunningRole},
    },
};

const SUMMARY_CHARS: usize = 140;

#[derive(Clone, Debug, Serialize)]
pub struct ConfigView {
    pub pings: Pings,
    pub watching: Watching,
    pub chatbot: Chatbot,
    pub notifications: Notifications,
    pub self_service: SelfService,
    pub persona: Persona,
    pub models: Models,
    pub manage_messages: ManageMessages,
    pub notices: Vec<String>,
    pub env: Vec<EnvRow>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Pings {
    pub day_of_ping_time: String,
    pub countdown_minutes: Vec<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Watching {
    pub paused: bool,
    pub extract_enabled: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Rate {
    pub count: u32,
    pub window_s: u32,
}

impl From<StoredRate> for Rate {
    fn from(rate: StoredRate) -> Self {
        Self {
            count: rate.count,
            window_s: rate.window_s,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Chatbot {
    pub enabled: bool,
    pub configured: bool,
    pub missing_env: Vec<String>,
    pub member_rate: Rate,
    pub guild_rate: Rate,
}

#[derive(Clone, Debug, Serialize)]
pub struct Notifications {
    pub quiet_mode: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelfService {
    pub mode: &'static str,
    pub effective_mode: &'static str,
    pub public_portal: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct PersonaEntry {
    pub key: String,
    pub name: String,
    pub bundle: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReplyProfile {
    pub key: String,
    pub name: String,
    pub public: bool,
    pub voice: String,
    pub prompt_summary: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoleProfile {
    pub role_id: String,
    pub role_name: String,
    pub profile: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Persona {
    pub active: String,
    pub personas: Vec<PersonaEntry>,
    pub profiles: Vec<ReplyProfile>,
    pub role_profiles: Vec<RoleProfile>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Admission {
    pub max_in_flight: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_max_in_flight: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub trust_zone: &'static str,
    pub leaves_homelab: bool,
    pub function_tools: bool,
    pub structured_output: bool,
    pub sampling_controls: bool,
    pub reasoning_control: bool,
    pub reasoning_efforts: Option<Vec<&'static str>>,
    /// False when the alias requires reasoning (a published list without `none`).
    pub off_allowed: bool,
    pub admission: Option<Admission>,
    /// Set on a listed `<base>:<level>` alias: the picker lists the base only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant_of: Option<String>,
    /// The variant's baked-in level in the `reasoning` vocabulary (`:none` is `off`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_effort: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RoleModel {
    pub alias: String,
    pub reasoning: &'static str,
    /// A stored variant alias: shown as "`variant_of` (fixed: `fixed_effort`)".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_effort: Option<&'static str>,
    /// What the role's next session opens with; absent while unrouted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<Running>,
}

/// The running alias and the level requests send (inherit and floors resolved).
#[derive(Clone, Debug, Serialize)]
pub struct Running {
    pub alias: String,
    pub reasoning: Option<&'static str>,
}

fn role_model(
    role: &StoredRole,
    catalog: &CatalogSnapshot,
    running: Option<&RunningRole>,
) -> RoleModel {
    let variant = role
        .alias
        .as_deref()
        .and_then(|alias| catalog.variant(alias));
    RoleModel {
        alias: role.alias.clone().unwrap_or_default(),
        reasoning: role.reasoning.as_str(),
        fixed_effort: variant.as_ref().map(|variant| variant.effort.as_str()),
        variant_of: variant.map(|variant| variant.base),
        running: running.map(|running| Running {
            alias: running.alias.clone(),
            reasoning: running.effort.map(|effort| effort.as_str()),
        }),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Roles {
    pub extraction: RoleModel,
    pub chat: RoleModel,
    pub rewrite: RoleModel,
}

#[derive(Clone, Debug, Serialize)]
pub struct CapacityGroup {
    pub model: String,
    pub group: String,
    pub permits: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct AliasLimit {
    pub alias: String,
    pub max_in_flight: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter_max_in_flight: Option<u32>,
    pub source: &'static str,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct KeyLimits {
    /// `None`: Kanata publishes no per-key limit.
    pub max_in_flight: Option<u32>,
    pub shared: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CapacityCheck {
    pub level: &'static str,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Models {
    pub reachable: bool,
    pub catalog: Vec<ModelInfo>,
    pub roles: Roles,
    pub groups: Vec<CapacityGroup>,
    /// `default` (the one `gateway` group) or `config` (`[[models.groups]]`).
    pub groups_source: &'static str,
    pub alias_limits: Vec<AliasLimit>,
    pub key_limits: KeyLimits,
    pub capacity_check: Vec<CapacityCheck>,
    pub pii_pseudonymise: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ManageMessages {
    pub missing: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EnvRow {
    pub key: &'static str,
    pub label: &'static str,
    pub value: String,
    pub reason: &'static str,
}

pub fn pings(settings: &RuntimeSettings) -> Pings {
    Pings {
        day_of_ping_time: super::hhmm(settings.pings.day_of_ping_time),
        countdown_minutes: settings.pings.countdown_minutes.clone(),
    }
}

pub fn self_service(settings: &RuntimeSettings) -> SelfService {
    SelfService {
        mode: settings.self_service.mode.as_str(),
        effective_mode: settings.self_service.effective_mode().as_str(),
        public_portal: settings.self_service.public_portal,
    }
}

pub fn roles(
    settings: &RuntimeSettings,
    catalog: &CatalogSnapshot,
    running: &BTreeMap<Role, RunningRole>,
) -> Roles {
    let role = |stored, role| role_model(stored, catalog, running.get(&role));
    Roles {
        extraction: role(&settings.models.extraction, Role::Extraction),
        chat: role(&settings.models.chat, Role::Chat),
        rewrite: role(&settings.models.rewrite, Role::Rewrite),
    }
}

fn trust_zone(zone: Option<TrustZone>) -> &'static str {
    match zone {
        Some(TrustZone::Local | TrustZone::PrivateNetwork) => "homelab",
        Some(TrustZone::External) => "external",
        None => "unknown",
    }
}

pub fn model_info(model: &CatalogModel, catalog: &CatalogSnapshot) -> ModelInfo {
    let variant = catalog.variant(&model.alias);
    ModelInfo {
        fixed_effort: variant.as_ref().map(|variant| variant.effort.as_str()),
        variant_of: variant.map(|variant| variant.base),
        id: model.alias.clone(),
        trust_zone: trust_zone(model.trust_zone),
        leaves_homelab: model.leaves_homelab,
        function_tools: model.function_tools,
        structured_output: model.structured_output,
        sampling_controls: model.sampling_controls,
        reasoning_control: model.reasoning_control,
        reasoning_efforts: model
            .reasoning_efforts
            .as_ref()
            .map(|efforts| efforts.iter().map(|effort| effort.as_str()).collect()),
        off_allowed: model.off_allowed(),
        admission: model.admission.map(|limits| Admission {
            max_in_flight: limits.max_in_flight,
            adapter_max_in_flight: limits.adapter_max_in_flight,
        }),
    }
}

/// Kanata's published admission per alias; nothing is operator-declared yet.
pub fn alias_limits(catalog: &CatalogSnapshot) -> Vec<AliasLimit> {
    catalog
        .models
        .iter()
        .filter_map(|model| {
            let limits = model.admission?;
            Some(AliasLimit {
                alias: model.alias.clone(),
                max_in_flight: limits.max_in_flight,
                adapter_max_in_flight: limits.adapter_max_in_flight,
                source: "published",
            })
        })
        .collect()
}

/// Catalog entries (bundles) and readable reply profiles of the live snapshot.
pub fn persona(active: &str, snapshot: &PersonaSnapshot) -> Persona {
    let mut personas = Vec::new();
    let mut profiles = Vec::new();
    let mut active = active.to_owned();
    if let Some(loaded) = snapshot.active() {
        // Unset means the catalog default is in use; report that bundle.
        if active.is_empty() {
            active = loaded.bundle.value.id.to_string();
        }
        match &loaded.catalog {
            Some(catalog) => {
                personas.extend(catalog.value.entries().iter().map(|entry| PersonaEntry {
                    key: entry.id.to_string(),
                    name: entry.label.clone(),
                    bundle: format!("bundles/{}.yaml", entry.id),
                }))
            }
            None => {
                let id = loaded.bundle.value.id.to_string();
                personas.push(PersonaEntry {
                    bundle: format!("bundles/{id}.yaml"),
                    name: id.clone(),
                    key: id,
                });
            }
        }
        // Every readable profile is selectable until visibility is stored.
        profiles.extend(loaded.profiles.readable.values().map(|profile| {
            let profile = &profile.value;
            ReplyProfile {
                key: profile.id.to_string(),
                name: profile.label.clone(),
                public: true,
                voice: profile.voice.clone().unwrap_or_default(),
                prompt_summary: summary(&profile.prompt),
            }
        }));
    }
    Persona {
        active,
        personas,
        profiles,
        role_profiles: Vec::new(),
    }
}

/// The prompt's first non-empty line, cut at a character boundary.
fn summary(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    if line.chars().count() <= SUMMARY_CHARS {
        return line.to_owned();
    }
    let mut cut: String = line.chars().take(SUMMARY_CHARS - 1).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::summary;

    #[test]
    fn summaries_take_the_first_line_and_cut_long_ones() {
        assert_eq!(summary("\n  Plain answers.\nMore."), "Plain answers.");
        let long = "é".repeat(200);
        let cut = summary(&long);
        assert_eq!(cut.chars().count(), 140);
        assert!(cut.ends_with('…'));
    }
}
