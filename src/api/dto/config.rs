//! `config.json#/$defs/ConfigView` and its projections from runtime settings,
//! the live model catalog, the persona files and deployment facts.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use ring::digest::{SHA256, digest};
use serde::Serialize;

use crate::{
    api::state::{ChannelEntry, RoleEntry},
    chat::persona::{PersonaSnapshot, ProfileId},
    domain::notify::WeeklyDigest,
    domain::settings::{
        ContextRole as StoredContextRole, Rate as StoredRate, RoleModel as StoredRole,
        RoleProfileAssignment as StoredRoleProfile, RunLengthOverride as StoredRunLengthOverride,
        RunLengths as StoredRunLengths, RuntimeSettings,
    },
    domain::time::ZonedDateTime,
    infrastructure::llm::{
        TrustZone,
        governor::Role,
        setup::{CatalogModel, CatalogSnapshot, RunningRole, resolve_context},
    },
};

const SUMMARY_CHARS: usize = 140;

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ConfigView {
    pub pings: Pings,
    pub watching: Watching,
    pub chatbot: Chatbot,
    pub notifications: Notifications,
    pub self_service: SelfService,
    pub persona: Persona,
    pub models: Models,
    pub run_lengths: RunLengths,
    pub manage_messages: ManageMessages,
    pub notices: Vec<String>,
    pub env: Vec<EnvRow>,
    /// `null` when no digest is active or the journal could not be read.
    pub last_digest: Option<LastDigest>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Pings {
    pub day_of_ping_time: String,
    pub countdown_minutes: Vec<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Watching {
    pub paused: bool,
    pub extract_enabled: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
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
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Chatbot {
    pub enabled: bool,
    pub configured: bool,
    pub missing_env: Vec<String>,
    pub member_rate: Rate,
    pub guild_rate: Rate,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Notifications {
    pub quiet_mode: bool,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunLengthOverride {
    pub boss: String,
    #[cfg_attr(test, ts(type = "Difficulty"))]
    pub difficulty: String,
    pub minutes: u32,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RunLengths {
    pub default_minutes: u32,
    pub overrides: Vec<RunLengthOverride>,
}

pub fn run_lengths(settings: &StoredRunLengths) -> RunLengths {
    let override_ = |override_: &StoredRunLengthOverride| RunLengthOverride {
        boss: override_.boss.clone(),
        difficulty: override_.difficulty.clone(),
        minutes: override_.minutes,
    };
    RunLengths {
        default_minutes: settings.default_minutes,
        overrides: settings.overrides.iter().map(override_).collect(),
    }
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "SelfServiceSettings"))]
pub struct SelfService {
    #[cfg_attr(test, ts(type = "SelfServiceMode"))]
    pub mode: &'static str,
    #[cfg_attr(test, ts(type = "SelfServiceMode"))]
    pub effective_mode: &'static str,
    pub public_portal: bool,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PersonaEntry {
    pub key: String,
    pub name: String,
    pub bundle: String,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReplyProfile {
    pub key: String,
    pub name: String,
    pub public: bool,
    pub voice: String,
    pub prompt_summary: String,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "RoleProfileView"))]
pub struct RoleProfile {
    pub role_id: String,
    pub role_name: Option<String>,
    pub profile: String,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PersonaSettings"))]
pub struct Persona {
    pub active: String,
    pub personas: Vec<PersonaEntry>,
    pub profiles: Vec<ReplyProfile>,
    pub role_profiles: Vec<RoleProfile>,
    pub role_profiles_digest: String,
}

/// Stable, order-sensitive revision for only the saved role/profile pairs.
pub fn role_profiles_digest(assignments: &[StoredRoleProfile]) -> String {
    let mut bytes = b"kanade.role-profiles.v1\0".to_vec();
    for assignment in assignments {
        for value in [&assignment.role_id, &assignment.profile] {
            bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
            bytes.extend_from_slice(value.as_bytes());
        }
    }
    let hex = digest(&SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sha256-v1:{hex}")
}

fn role_profiles(assignments: &[StoredRoleProfile], roles: &[RoleEntry]) -> Vec<RoleProfile> {
    assignments
        .iter()
        .map(|assignment| RoleProfile {
            role_id: assignment.role_id.clone(),
            role_name: roles
                .iter()
                .find(|role| role.id == assignment.role_id)
                .map(|role| role.name.clone()),
            profile: assignment.profile.clone(),
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Admission {
    pub max_in_flight: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub adapter_max_in_flight: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ModelInfo {
    pub id: String,
    #[cfg_attr(test, ts(type = "'homelab' | 'external' | 'unknown'"))]
    pub trust_zone: &'static str,
    pub leaves_homelab: bool,
    pub function_tools: bool,
    pub structured_output: bool,
    pub sampling_controls: bool,
    pub reasoning_control: bool,
    pub reasoning_efforts: Option<Vec<&'static str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub context_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub max_output_tokens: Option<u32>,
    /// False when the alias requires reasoning (a published list without `none`).
    pub off_allowed: bool,
    pub admission: Option<Admission>,
    /// Set on a listed `<base>:<level>` alias: the picker lists the base only.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub variant_of: Option<String>,
    /// The variant's baked-in level in the `reasoning` vocabulary (`:none` is `off`).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub fixed_effort: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RoleModel {
    pub alias: String,
    pub reasoning: &'static str,
    /// A stored variant alias: shown as "`variant_of` (fixed: `fixed_effort`)".
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub variant_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub fixed_effort: Option<&'static str>,
    /// What the role's next session opens with; absent while unrouted.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub running: Option<Running>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub context: Option<EffectiveContext>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EffectiveContext {
    pub window: u32,
    pub reserve: u32,
    pub prompt_budget: u32,
    #[cfg_attr(test, ts(type = "ContextSource"))]
    pub source: &'static str,
    pub clamped_by_published: bool,
    pub clamped_by_hard_cap: bool,
    pub clamped_by_role_cap: bool,
    pub local_warning: bool,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ContextRole {
    pub reserve: u32,
    pub cap: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ContextSettings {
    pub cloud_default: u32,
    pub local_default: u32,
    pub chat: ContextRole,
    pub extraction: ContextRole,
    pub rewrite: ContextRole,
    pub overrides: BTreeMap<String, u32>,
}

/// The running alias and the level requests send (inherit and floors resolved).
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "RunningRole"))]
pub struct Running {
    pub alias: String,
    pub reasoning: Option<&'static str>,
}

fn role_model(
    role: &StoredRole,
    context_role: Role,
    settings: &RuntimeSettings,
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
        context: role.alias.as_deref().map(|alias| {
            let resolved = resolve_context(&settings.models.context, catalog, context_role, alias);
            EffectiveContext {
                window: resolved.window,
                reserve: resolved.reserve,
                prompt_budget: resolved.prompt_budget,
                source: resolved.source.as_str(),
                clamped_by_published: resolved.clamped_by_published,
                clamped_by_hard_cap: resolved.clamped_by_hard_cap,
                clamped_by_role_cap: resolved.clamped_by_role_cap,
                local_warning: resolved.local_warning,
            }
        }),
    }
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "ModelRoles"))]
pub struct Roles {
    pub extraction: RoleModel,
    pub chat: RoleModel,
    pub rewrite: RoleModel,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CapacityGroup {
    pub model: String,
    pub group: String,
    pub permits: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct AliasLimit {
    pub alias: String,
    pub max_in_flight: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(test, ts(optional))]
    pub adapter_max_in_flight: Option<u32>,
    #[cfg_attr(test, ts(type = "'published' | 'declared'"))]
    pub source: &'static str,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct KeyLimits {
    /// `None`: Kanata publishes no per-key limit.
    pub max_in_flight: Option<u32>,
    pub shared: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CapacityCheck {
    #[cfg_attr(test, ts(type = "'ok' | 'warning' | 'error'"))]
    pub level: &'static str,
    pub message: String,
    /// The capacity group the check is about (`models.groups[].group`);
    /// `null` for checks that span groups.
    pub group: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "ModelSettings"))]
pub struct Models {
    pub reachable: bool,
    pub catalog: Vec<ModelInfo>,
    pub roles: Roles,
    pub groups: Vec<CapacityGroup>,
    /// `default` (the one `gateway` group) or `config` (`[[models.groups]]`).
    #[cfg_attr(test, ts(type = "'default' | 'config'"))]
    pub groups_source: &'static str,
    pub alias_limits: Vec<AliasLimit>,
    pub key_limits: KeyLimits,
    pub capacity_check: Vec<CapacityCheck>,
    pub pii_pseudonymise: bool,
    pub context: ContextSettings,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ManageMessages {
    pub missing: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EnvRow {
    pub key: &'static str,
    pub label: &'static str,
    pub value: String,
    pub reason: &'static str,
    /// The raw value to paste into the deployment env for `key`; `null` when
    /// unset or when the row has no single env value. Never a secret.
    pub copy: Option<String>,
}

/// The most recent active weekly digest card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct LastDigest {
    /// RFC 3339 in the guild's offset, whole seconds.
    pub posted_at: String,
    /// Guild-local boss-week start date (`YYYY-MM-DD`).
    pub week_start: String,
    pub this_week: bool,
    pub channel_id: String,
    pub channel_name: Option<String>,
    pub url: Option<String>,
}

/// The newest non-retired digest by `posted_at`, in the guild's zone;
/// `current_week` is the running boss week's start instant.
pub fn last_digest(
    digests: &[WeeklyDigest],
    zone: Tz,
    current_week: DateTime<Utc>,
    channels: &[ChannelEntry],
    guild_id: Option<&str>,
) -> Option<LastDigest> {
    let digest = digests
        .iter()
        .filter(|digest| digest.retired_at.is_none())
        .max_by_key(|digest| (digest.posted_at, digest.week_start))?;
    let posted = digest
        .posted_at
        .with_nanosecond(0)
        .unwrap_or(digest.posted_at);
    let posted_at = ZonedDateTime::from_instant(&posted, zone).ok()?.isoformat();
    let week = ZonedDateTime::from_instant(&digest.week_start, zone).ok()?;
    Some(LastDigest {
        posted_at,
        week_start: super::iso_date(week.wall().date()),
        this_week: digest.week_start == current_week,
        channel_name: channels
            .iter()
            .find(|channel| channel.id == digest.channel_id)
            .map(|channel| channel.name.clone()),
        url: guild_id.map(|guild| {
            format!(
                "https://discord.com/channels/{guild}/{}/{}",
                digest.channel_id, digest.message_id
            )
        }),
        channel_id: digest.channel_id.clone(),
    })
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
    let role = |stored, role| role_model(stored, role, settings, catalog, running.get(&role));
    Roles {
        extraction: role(&settings.models.extraction, Role::Extraction),
        chat: role(&settings.models.chat, Role::Chat),
        rewrite: role(&settings.models.rewrite, Role::Rewrite),
    }
}

pub fn context_settings(settings: &crate::domain::settings::ContextSettings) -> ContextSettings {
    let role = |role: &StoredContextRole| ContextRole {
        reserve: role.reserve,
        cap: role.cap,
    };
    ContextSettings {
        cloud_default: settings.cloud_default,
        local_default: settings.local_default,
        chat: role(&settings.chat),
        extraction: role(&settings.extraction),
        rewrite: role(&settings.rewrite),
        overrides: settings.overrides.clone(),
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
        context_tokens: model.context_tokens,
        max_output_tokens: model.max_output_tokens,
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

/// Catalog entries and readable profiles, with publication from saved settings.
pub fn persona(
    active: &str,
    snapshot: Option<&PersonaSnapshot>,
    selectable: &BTreeSet<ProfileId>,
    assignments: &[StoredRoleProfile],
    roles: &[RoleEntry],
) -> Persona {
    let mut personas = Vec::new();
    let mut profiles = Vec::new();
    let mut active = active.to_owned();
    if let Some(loaded) = snapshot.and_then(PersonaSnapshot::active) {
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
        profiles.extend(loaded.profiles.readable.values().map(|profile| {
            let profile = &profile.value;
            ReplyProfile {
                key: profile.id.to_string(),
                name: profile.label.clone(),
                public: selectable.contains(&profile.id),
                voice: profile.voice.clone().unwrap_or_default(),
                prompt_summary: summary(&profile.prompt),
            }
        }));
    }
    Persona {
        active,
        personas,
        profiles,
        role_profiles: role_profiles(assignments, roles),
        role_profiles_digest: role_profiles_digest(assignments),
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
    use chrono::TimeZone;

    use super::*;

    fn digest(week: u32, posted: (u32, u32, u32), retired: bool) -> WeeklyDigest {
        let at = |day, hour, minute| Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0).unwrap();
        WeeklyDigest {
            week_start: at(week, 16, 0),
            channel_id: "11".into(),
            message_id: format!("m{week}"),
            posted_at: at(posted.0, posted.1, posted.2) + chrono::Duration::microseconds(250),
            retired_at: retired.then(|| at(30, 0, 0)),
        }
    }

    #[test]
    fn last_digest_is_the_newest_active_card_in_guild_time() {
        let zone: Tz = "Asia/Kuala_Lumpur".parse().unwrap();
        // Boss weeks start Thu 00:00 +08:00 (Wed 16:00 UTC).
        let digests = [
            digest(16, (16, 16, 15), false),
            digest(23, (23, 16, 15), false),
            digest(30, (30, 16, 15), true),
        ];
        let channels = [ChannelEntry {
            id: "11".into(),
            name: "boss-chat".into(),
            watched: false,
        }];
        let current = Utc.with_ymd_and_hms(2026, 9, 23, 16, 0, 0).unwrap();
        let last = last_digest(&digests, zone, current, &channels, Some("9")).unwrap();
        assert_eq!(
            last,
            LastDigest {
                posted_at: "2026-09-24T00:15:00+08:00".into(),
                week_start: "2026-09-24".into(),
                this_week: true,
                channel_id: "11".into(),
                channel_name: Some("boss-chat".into()),
                url: Some("https://discord.com/channels/9/11/m23".into()),
            }
        );
        let earlier = Utc.with_ymd_and_hms(2026, 9, 30, 16, 0, 0).unwrap();
        let last = last_digest(&digests[..1], zone, earlier, &[], None).unwrap();
        assert!(!last.this_week);
        assert_eq!((last.channel_name, last.url), (None, None));
        assert_eq!(last_digest(&digests[2..], zone, current, &[], None), None);
    }

    #[test]
    fn summaries_take_the_first_line_and_cut_long_ones() {
        assert_eq!(summary("\n  Plain answers.\nMore."), "Plain answers.");
        let long = "é".repeat(200);
        let cut = summary(&long);
        assert_eq!(cut.chars().count(), 140);
        assert!(cut.ends_with('…'));
    }
}
