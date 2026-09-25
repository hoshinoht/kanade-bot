//! Runtime settings (v4 /config) plus v5's model roles, capacity groups,
//! self-service mode, public-portal switch, persona catalog and channel
//! access. Env-only settings are reported read-only with the reason.
//!
//! Capacity follows the recorded Kanata admission decisions: Kanata admits
//! per route (alias + operation); limits come per alias from /v1/models
//! `kanata.admission` metadata (route max_in_flight, adapter_max_in_flight)
//! or are declared by the operator. Each group's N must stay within the
//! minimum over its aliases, and the groups' summed concurrency within the
//! key-level max_in_flight while the deployment shares its key.

use super::seed;
use super::{MoveError, Store};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Clone, Serialize)]
pub struct RoleModel {
    pub alias: String,
    /// `off`, a published effort (or low/medium/high when the model decides),
    /// or `""` = the extraction role's effort.
    pub reasoning: String,
}

#[derive(Clone, Serialize)]
pub struct Group {
    pub model: String,
    pub group: String,
    pub permits: u32,
}

#[derive(Clone, Serialize)]
pub struct RoleProfile {
    pub role_id: String,
    pub role_name: String,
    pub profile: String,
}

#[derive(Clone, Serialize)]
pub struct ProfileVisibility {
    pub key: String,
    pub public: bool,
}

#[derive(Clone, Serialize)]
pub struct Config {
    pub day_of_ping_time: String,
    pub countdown_minutes: Vec<u32>,
    pub paused: bool,
    pub extract_enabled: bool,
    pub chat_enabled: bool,
    pub member_rate: (u32, u32),
    pub guild_rate: (u32, u32),
    pub quiet_mode: bool,
    pub self_service_mode: String,
    pub public_portal: bool,
    pub persona: String,
    pub role_profiles: Vec<RoleProfile>,
    pub profile_visibility: Vec<ProfileVisibility>,
    pub extraction: RoleModel,
    pub chat: RoleModel,
    pub rewrite: RoleModel,
    pub groups: Vec<Group>,
}

struct ModelInfo {
    id: &'static str,
    trust: &'static str,
    tools: bool,
    json: bool,
    sampling: bool,
    /// Published reasoning efforts; None = the model decides (v4 offered
    /// low/medium/high for these); empty = no reasoning control.
    efforts: Option<&'static [&'static str]>,
    /// Published route max_in_flight from Kanata's admission metadata.
    route_max: Option<u32>,
    /// Published adapter_max_in_flight, capping every route on the adapter.
    adapter_max: Option<u32>,
    /// Operator-declared limit, used when Kanata publishes nothing.
    declared_max: Option<u32>,
}

impl ModelInfo {
    /// The concurrency Kanata admits for this alias, if any is known.
    fn cap(&self) -> Option<u32> {
        let route = self.route_max.or(self.declared_max)?;
        Some(self.adapter_max.map_or(route, |a| route.min(a)))
    }

    fn source(&self) -> &'static str {
        if self.route_max.is_some() {
            "published"
        } else {
            "declared"
        }
    }
}

/// Kanata's live alias list (synthetic): capabilities and admission metadata
/// as its catalog publishes them. `kanata/rewrite-cloud` exercises the
/// suffix rule: the Ollama cloud proxy reports `local`, so an alias ending
/// in `-cloud` leaves the homelab whatever its trust zone says.
const MODELS: [ModelInfo; 6] = [
    ModelInfo {
        id: "kanata/extract",
        trust: "homelab",
        tools: false,
        json: true,
        sampling: true,
        efforts: Some(&["low", "medium", "high"]),
        route_max: Some(1),
        adapter_max: Some(2),
        declared_max: None,
    },
    ModelInfo {
        id: "kanata/chat",
        trust: "homelab",
        tools: true,
        json: true,
        sampling: true,
        efforts: Some(&["low", "medium"]),
        route_max: Some(4),
        adapter_max: Some(8),
        declared_max: None,
    },
    ModelInfo {
        id: "kanata/rewrite-small",
        trust: "homelab",
        tools: false,
        json: false,
        sampling: true,
        efforts: Some(&[]),
        route_max: None,
        adapter_max: None,
        declared_max: Some(1),
    },
    ModelInfo {
        id: "kanata/chat-cloud",
        trust: "external",
        tools: true,
        json: true,
        sampling: false,
        efforts: Some(&["minimal", "low", "medium", "high"]),
        route_max: Some(4),
        adapter_max: Some(8),
        declared_max: None,
    },
    ModelInfo {
        id: "kanata/legacy",
        trust: "unknown",
        tools: false,
        json: true,
        sampling: true,
        efforts: None,
        route_max: None,
        adapter_max: None,
        declared_max: Some(2),
    },
    ModelInfo {
        id: "kanata/rewrite-cloud",
        trust: "homelab",
        tools: false,
        json: false,
        sampling: true,
        efforts: Some(&[]),
        route_max: Some(2),
        adapter_max: Some(4),
        declared_max: None,
    },
];

/// Key-level admission for this deployment's key (synthetic). The key is
/// shared with the owner's other clients, so its limits are sized for both.
const KEY_MAX_IN_FLIGHT: u32 = 8;
/// Upper bound on one row's permits; larger values are refused, not clamped.
const MAX_PERMITS: u64 = 64;
const KEY_SHARED: bool = true;

/// `leaves_homelab`, failing closed: external trust, an unknown zone, or the
/// `-cloud` alias suffix all count as leaving.
fn leaves_homelab(id: &str, trust: &str) -> bool {
    trust == "external" || trust != "homelab" || id.ends_with("-cloud")
}

const PERSONAS: [(&str, &str, &str); 3] = [
    ("kanade", "Kanade", "bundles/kanade.yaml"),
    ("yuuki", "YuukiSakuna", "bundles/yuuki.yaml"),
    ("plain", "Plain (no persona)", "bundles/plain.yaml"),
];

/// Reply profiles live as files under `config/personas/profiles/`; the app
/// shows them read-only and only publishes them or assigns them to roles.
const PROFILES: [(&str, &str, bool, &str, &str); 4] = [
    (
        "default",
        "Default",
        true,
        "Warm and plain",
        "Answers plainly in the guild's voice, names the run facts first and keeps it short.",
    ),
    (
        "terse",
        "Terse",
        true,
        "Short to the point of blunt",
        "One or two short sentences, schedule facts only, no small talk and no emoji.",
    ),
    (
        "kanade",
        "Kanade",
        true,
        "Cheeky and smug, earnest underneath",
        "Teases lightly in the persona's voice while keeping every schedule fact exact.",
    ),
    (
        "sparkly",
        "Sparkly",
        false,
        "Overexcited kouhai",
        "Bursts with enthusiasm and exclamation, still lands the facts. Private while it settles in.",
    ),
];

pub const PII_PSEUDONYMISE: bool = true;

pub fn defaults() -> Config {
    Config {
        day_of_ping_time: "09:00".into(),
        countdown_minutes: vec![60, 15],
        paused: false,
        extract_enabled: true,
        chat_enabled: true,
        member_rate: (4, 300),
        guild_rate: (12, 900),
        quiet_mode: false,
        self_service_mode: "cards_and_link".into(),
        public_portal: true,
        persona: "kanade".into(),
        role_profiles: vec![
            RoleProfile {
                role_id: "300001".into(),
                role_name: "@staff".into(),
                profile: "terse".into(),
            },
            RoleProfile {
                role_id: "300002".into(),
                role_name: "@newbies".into(),
                profile: "default".into(),
            },
        ],
        profile_visibility: PROFILES
            .iter()
            .map(|(key, _, public, _, _)| ProfileVisibility {
                key: (*key).into(),
                public: *public,
            })
            .collect(),
        extraction: RoleModel {
            alias: "kanata/extract".into(),
            reasoning: "medium".into(),
        },
        chat: RoleModel {
            alias: "kanata/chat".into(),
            reasoning: String::new(),
        },
        rewrite: RoleModel {
            alias: "kanata/rewrite-small".into(),
            reasoning: "off".into(),
        },
        groups: vec![
            Group {
                model: "kanata/extract".into(),
                group: "extract".into(),
                permits: 1,
            },
            Group {
                model: "kanata/chat".into(),
                group: "chat".into(),
                permits: 2,
            },
            Group {
                model: "kanata/chat-cloud".into(),
                group: "chat".into(),
                permits: 2,
            },
            Group {
                model: "kanata/rewrite-small".into(),
                group: "rewrite".into(),
                permits: 1,
            },
        ],
    }
}

fn model(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}

fn profile_visible(c: &Config, key: &str) -> bool {
    c.profile_visibility
        .iter()
        .find(|v| v.key == key)
        .map(|v| v.public)
        .unwrap_or(false)
}

/// The startup check, also run on every save: row problems, duplicate
/// aliases, per-group admission caps, role models without a group (a warning:
/// they run with no permits), and the key-level sum. Messages name their row
/// so two identical rows never produce the same message.
pub fn capacity_check(c: &Config) -> Vec<Value> {
    let mut out = Vec::new();
    for (i, g) in c.groups.iter().enumerate() {
        let row = i + 1;
        if g.group.is_empty() {
            out.push(json!({ "level": "error", "message": format!("Row {row}: every row needs a group name.") }));
        }
        if model(&g.model).is_none() {
            out.push(json!({ "level": "error", "message": format!("Row {row}: Kanata does not list {}.", g.model) }));
        }
        if g.permits == 0 {
            out.push(json!({ "level": "error", "message": format!("Row {row}: {} in group {} declares 0 permits.", g.model, g.group) }));
        }
    }
    // An alias belongs to exactly one group, listed once.
    let mut seen: Vec<(&str, &str)> = Vec::new();
    for (i, g) in c.groups.iter().enumerate() {
        let row = i + 1;
        for (j, (alias, group)) in seen.iter().enumerate() {
            if *alias == g.model.as_str() {
                if *group == g.group.as_str() {
                    out.push(json!({ "level": "error", "message": format!("Rows {} and {row}: {} is in group {} twice; list it once.", j + 1, g.model, g.group) }));
                } else {
                    out.push(json!({ "level": "error", "message": format!("{} is in groups {group} and {}; an alias belongs to exactly one group.", g.model, g.group) }));
                }
                break;
            }
        }
        seen.push((&c.groups[i].model, &c.groups[i].group));
    }
    for (role, m) in [
        ("extraction", &c.extraction),
        ("chat", &c.chat),
        ("rewrite", &c.rewrite),
    ] {
        if !c.groups.iter().any(|g| g.model == m.alias) {
            out.push(json!({ "level": "warning", "message": format!("The {role} model {} is in no capacity group, so it runs with no permits.", m.alias) }));
        }
    }
    let mut names: Vec<&str> = c.groups.iter().map(|g| g.group.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let rows: Vec<&Group> = c.groups.iter().filter(|g| g.group == name).collect();
        let total: u32 = rows.iter().map(|g| g.permits).sum();
        let mut cap: Option<(u32, &str)> = None;
        for g in &rows {
            match model(&g.model).and_then(ModelInfo::cap) {
                Some(limit) => {
                    if cap.is_none_or(|(c, _)| limit < c) {
                        cap = Some((limit, &g.model));
                    }
                }
                None => {
                    cap = None;
                    out.push(json!({ "level": "error", "message": format!("Kanata publishes no limit for {} and none is declared; declare one for it first.", g.model) }));
                    break;
                }
            }
        }
        if let Some((limit, capped_by)) = cap {
            if total > limit {
                out.push(json!({ "level": "error", "message": format!("Group {name} declares {total} permits but Kanata admits at most {limit} (capped by {capped_by}); the bot refuses to start.") }));
            } else if total < limit {
                out.push(json!({ "level": "warning", "message": format!("Group {name} uses {total} of the {limit} permits Kanata admits.") }));
            } else {
                out.push(json!({ "level": "ok", "message": format!("Group {name}: {total} permits, matching Kanata's limit.") }));
            }
        }
    }
    let sum: u32 = c.groups.iter().map(|g| g.permits).sum();
    if sum > KEY_MAX_IN_FLIGHT {
        out.push(json!({ "level": "error", "message": format!("All groups declare {sum} permits but the key admits {KEY_MAX_IN_FLIGHT}; the bot refuses to start.") }));
    }
    if KEY_SHARED {
        out.push(json!({ "level": "warning", "message": "This key is shared with the owner's other clients; size its limits for both.".to_owned() }));
    }
    out
}

/// `adapter_max_in_flight` is optional in the DTO: absent, never null.
fn admission(max: u32, adapter: Option<u32>) -> Value {
    let mut out = json!({ "max_in_flight": max });
    if let Some(a) = adapter {
        out["adapter_max_in_flight"] = json!(a);
    }
    out
}

fn effective_mode(c: &Config) -> &str {
    if c.public_portal {
        &c.self_service_mode
    } else {
        "cards_only"
    }
}

/// Whether `effort` is a legal reasoning level for this alias: `off` always
/// is; a model that decides for itself takes v4's low/medium/high.
fn valid_reasoning(info: &ModelInfo, effort: &str) -> bool {
    if effort == "off" {
        return true;
    }
    match info.efforts {
        None => matches!(effort, "low" | "medium" | "high"),
        Some(published) => published.contains(&effort),
    }
}

fn resolve<'a>(reasoning: &'a str, extraction: &'a str) -> &'a str {
    if reasoning.is_empty() {
        extraction
    } else {
        reasoning
    }
}

impl Store {
    pub fn config_view(&self) -> Value {
        let c = &self.config;
        let models: Vec<Value> = MODELS
            .iter()
            .map(|m| {
                json!({
                    "id": m.id, "trust_zone": m.trust, "leaves_homelab": leaves_homelab(m.id, m.trust),
                    "function_tools": m.tools, "structured_output": m.json, "sampling_controls": m.sampling,
                    "reasoning_control": m.efforts.is_none_or(|e| !e.is_empty()),
                    "reasoning_efforts": m.efforts,
                    "admission": m.cap().map(|max| admission(max, m.adapter_max)),
                })
            })
            .collect();
        let missing_manage: Vec<&str> = seed::CHANNELS
            .iter()
            .filter(|(id, _, _)| *id == "seren-trio" || *id == "bm-trio")
            .map(|(_, name, _)| *name)
            .collect();
        json!({
            "pings": { "day_of_ping_time": c.day_of_ping_time, "countdown_minutes": c.countdown_minutes },
            "watching": { "paused": c.paused, "extract_enabled": c.extract_enabled },
            "chatbot": {
                "enabled": c.chat_enabled,
                "configured": true,
                "missing_env": Vec::<String>::new(),
                "member_rate": { "count": c.member_rate.0, "window_s": c.member_rate.1 },
                "guild_rate": { "count": c.guild_rate.0, "window_s": c.guild_rate.1 },
            },
            "notifications": { "quiet_mode": c.quiet_mode },
            "self_service": { "mode": c.self_service_mode, "effective_mode": effective_mode(c), "public_portal": c.public_portal },
            "persona": {
                "active": c.persona,
                "personas": PERSONAS.iter().map(|(key, name, bundle)| json!({ "key": key, "name": name, "bundle": bundle })).collect::<Vec<_>>(),
                "profiles": PROFILES.iter().map(|(key, name, _, voice, summary)| json!({
                    "key": key, "name": name, "public": profile_visible(c, key),
                    "voice": voice, "prompt_summary": summary,
                })).collect::<Vec<_>>(),
                "role_profiles": c.role_profiles,
            },
            "models": {
                "reachable": true,
                "catalog": models,
                "roles": { "extraction": c.extraction, "chat": c.chat, "rewrite": c.rewrite },
                "groups": c.groups,
                "alias_limits": MODELS.iter().filter_map(|m| m.cap().map(|max| {
                    let mut limit = admission(max, m.adapter_max);
                    limit["alias"] = json!(m.id);
                    limit["source"] = json!(m.source());
                    limit
                })).collect::<Vec<_>>(),
                "key_limits": { "max_in_flight": KEY_MAX_IN_FLIGHT, "shared": KEY_SHARED },
                "capacity_check": capacity_check(c),
                "pii_pseudonymise": PII_PSEUDONYMISE,
            },
            "manage_messages": { "missing": missing_manage },
            "env": [
                { "key": "KANADE_TIMEZONE", "label": "Timezone", "value": "Asia/Kuala_Lumpur", "reason": "Every stored time is converted with it; a change needs a restart." },
                { "key": "KANADE_RESET", "label": "Boss week starts", "value": "Thu 00:00", "reason": "Defines boss-week boundaries for every stored run." },
                { "key": "KANATA_BASE_URL", "label": "Model gateway", "value": "https://kanata.example.internal", "reason": "The gateway address is deployment wiring; repointing it would redirect the bearer key, so only the operator changes it." },
                { "key": "KANADE_PII_PSEUDONYMISE", "label": "PII pseudonymisation", "value": if PII_PSEUDONYMISE { "on" } else { "off" }, "reason": "A privacy control; only the operator may change it, in the environment." },
                { "key": "KANADE_MIN_CONFIDENCE", "label": "Minimum confidence", "value": "0.6", "reason": "Tuned with the extraction vectors, not at runtime." },
                { "key": "KANADE_DIGEST_CHANNEL", "label": "Digest channel", "value": "#boss-schedule", "reason": "Set with the guild's channel layout." },
                { "key": "KANADE_WATCHED", "label": "Watched channels", "value": seed::CHANNELS.iter().filter(|c| c.2).map(|c| c.1).collect::<Vec<_>>().join(", "), "reason": "Watching a new channel is a deliberate deploy." },
                { "key": "KANADE_WATCHED_CATEGORIES", "label": "Watched categories", "value": "none", "reason": "Watching a whole category is a deliberate deploy, like channels." },
            ],
            "notices": Vec::<String>::new(),
        })
    }

    /// One section per request, each partial; arrays are replaced whole.
    /// Unknown or read-only keys are refused with 422, like the backend.
    pub fn patch_config(&mut self, patch: &Value) -> Result<Value, MoveError> {
        check_patch_keys(patch)?;
        let mut next = self.config.clone();
        let mut notices: Vec<String> = Vec::new();
        let bad = |m: &str| MoveError::invalid(m.to_owned());
        if let Some(p) = patch.get("pings") {
            if let Some(t) = p.get("day_of_ping_time").and_then(Value::as_str) {
                if !super::clock::valid_time(t) {
                    return Err(bad("The morning ping is HH:MM, for example 09:00."));
                }
                next.day_of_ping_time = t.into();
            }
            if let Some(list) = p.get("countdown_minutes").and_then(Value::as_array) {
                let mins: Option<Vec<u32>> = list
                    .iter()
                    .map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()))
                    .collect();
                let mins = mins
                    .filter(|m| m.len() <= 4 && m.iter().all(|n| (5..=24 * 60).contains(n)))
                    .ok_or(bad(
                        "Countdowns are up to four whole minutes between 5 and 1440.",
                    ))?;
                next.countdown_minutes = mins;
            }
        }
        if let Some(p) = patch.get("watching") {
            if let Some(v) = p.get("paused").and_then(Value::as_bool) {
                next.paused = v;
            }
            if let Some(v) = p.get("extract_enabled").and_then(Value::as_bool) {
                next.extract_enabled = v;
            }
        }
        if let Some(p) = patch.get("chatbot") {
            if let Some(v) = p.get("enabled").and_then(Value::as_bool) {
                next.chat_enabled = v;
            }
            for (key, slot) in [
                ("member_rate", &mut next.member_rate),
                ("guild_rate", &mut next.guild_rate),
            ] {
                if let Some(r) = p.get(key) {
                    let count = r
                        .get("count")
                        .and_then(Value::as_u64)
                        .filter(|n| (1..=100).contains(n));
                    let window = r
                        .get("window_s")
                        .and_then(Value::as_u64)
                        .filter(|n| (10..=86_400).contains(n));
                    let (Some(count), Some(window)) = (count, window) else {
                        return Err(bad("A rate is 1-100 answers per 10-86400 seconds."));
                    };
                    *slot = (count as u32, window as u32);
                }
            }
        }
        if let Some(v) = patch
            .pointer("/notifications/quiet_mode")
            .and_then(Value::as_bool)
        {
            next.quiet_mode = v;
        }
        if let Some(p) = patch.get("self_service") {
            if let Some(mode) = p.get("mode").and_then(Value::as_str) {
                if !matches!(mode, "cards_and_link" | "link_first" | "cards_only") {
                    return Err(bad(
                        "Self-service is cards and link, link first, or cards only.",
                    ));
                }
                next.self_service_mode = mode.into();
            }
            if let Some(v) = p.get("public_portal").and_then(Value::as_bool) {
                next.public_portal = v;
            }
        }
        if let Some(p) = patch.get("persona") {
            if let Some(active) = p.get("active").and_then(Value::as_str) {
                if !PERSONAS.iter().any(|(k, _, _)| *k == active) {
                    return Err(bad("No such persona in the catalog."));
                }
                next.persona = active.into();
            }
            if let Some(list) = p.get("visibility").and_then(Value::as_array) {
                for item in list {
                    let key = item
                        .get("key")
                        .and_then(Value::as_str)
                        .filter(|k| PROFILES.iter().any(|(known, _, _, _, _)| *known == *k));
                    let public = item.get("public").and_then(Value::as_bool);
                    let (Some(key), Some(public)) = (key, public) else {
                        return Err(bad(
                            "Each visibility change needs a known profile and true or false.",
                        ));
                    };
                    if let Some(slot) = next.profile_visibility.iter_mut().find(|v| v.key == key) {
                        slot.public = public;
                    }
                }
            }
            if let Some(list) = p.get("role_profiles").and_then(Value::as_array) {
                let mut roles = Vec::new();
                for item in list {
                    let role_id = item
                        .get("role_id")
                        .and_then(Value::as_str)
                        .filter(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()));
                    let profile = item
                        .get("profile")
                        .and_then(Value::as_str)
                        .filter(|p| PROFILES.iter().any(|(known, _, _, _, _)| *known == *p));
                    let (Some(role_id), Some(profile)) = (role_id, profile) else {
                        return Err(bad(
                            "Each assignment needs a numeric Discord role id and a known profile.",
                        ));
                    };
                    let role_name = item
                        .get("role_name")
                        .and_then(Value::as_str)
                        .unwrap_or(role_id);
                    roles.push(RoleProfile {
                        role_id: role_id.into(),
                        role_name: role_name.into(),
                        profile: profile.into(),
                    });
                }
                next.role_profiles = roles;
            }
        }
        if let Some(p) = patch.get("models") {
            let roles = p.get("roles");
            for (role, slot) in [
                ("extraction", &mut next.extraction),
                ("chat", &mut next.chat),
                ("rewrite", &mut next.rewrite),
            ] {
                let Some(r) = roles.and_then(|r| r.get(role)) else {
                    continue;
                };
                if let Some(alias) = r.get("alias").and_then(Value::as_str) {
                    let info = model(alias).ok_or_else(|| {
                        MoveError::Invalid(format!("Kanata does not list {alias}."))
                    })?;
                    if role == "chat" && !info.tools {
                        return Err(MoveError::Invalid(format!(
                            "{alias} cannot call tools, which the chatbot needs."
                        )));
                    }
                    slot.alias = alias.into();
                }
            }
            // Explicit levels first; inheritance is checked in a final pass
            // against the FINAL extraction effort and each role's final alias,
            // so one request can move everything together.
            for (role, slot) in [
                ("extraction", &mut next.extraction),
                ("chat", &mut next.chat),
                ("rewrite", &mut next.rewrite),
            ] {
                let patched = roles
                    .and_then(|r| r.get(role))
                    .and_then(|r| r.get("reasoning"))
                    .and_then(Value::as_str);
                let Some(level) = patched else { continue };
                let info = model(&slot.alias).ok_or(bad("Unknown model."))?;
                if level.is_empty() {
                    if role == "extraction" {
                        return Err(bad("Extraction sets its own reasoning; it cannot inherit."));
                    }
                } else if !valid_reasoning(info, level) {
                    let offered = match info.efforts {
                        None => vec!["off", "low", "medium", "high"],
                        Some(published) => std::iter::once("off")
                            .chain(published.iter().copied())
                            .collect(),
                    };
                    return Err(MoveError::Invalid(format!(
                        "{} accepts reasoning {}, not {level}.",
                        slot.alias,
                        offered.join(", ")
                    )));
                }
                slot.reasoning = level.into();
            }
            // An inherit the request asked for must resolve to a legal level:
            // an explicit request is refused (422), never silently reset.
            let extraction = next.extraction.reasoning.clone();
            for (role, slot) in [("chat", &next.chat), ("rewrite", &next.rewrite)] {
                let asked = roles
                    .and_then(|r| r.get(role))
                    .and_then(|r| r.get("reasoning"))
                    .and_then(Value::as_str)
                    == Some("");
                let Some(info) = model(&slot.alias) else {
                    continue;
                };
                if asked && !valid_reasoning(info, &extraction) {
                    return Err(MoveError::Invalid(format!(
                        "{role} inherits {extraction} from extraction, which {} does not publish; pick a level or turn reasoning off.",
                        slot.alias,
                    )));
                }
            }
            // A side effect of an alias or extraction change can strand a
            // role whose reasoning was not part of this request: reset it to
            // off and say so, rather than refuse the whole save.
            let stranded: Vec<(&str, String, String)> = ["extraction", "chat", "rewrite"]
                .into_iter()
                .filter_map(|role| {
                    let slot = match role {
                        "extraction" => &next.extraction,
                        "chat" => &next.chat,
                        _ => &next.rewrite,
                    };
                    let touched = roles
                        .and_then(|r| r.get(role))
                        .and_then(|r| r.get("reasoning"))
                        .is_some();
                    if touched {
                        return None;
                    }
                    let info = model(&slot.alias)?;
                    let effective = resolve(&slot.reasoning, &next.extraction.reasoning);
                    if valid_reasoning(info, effective) {
                        None
                    } else {
                        Some((role, slot.alias.clone(), effective.to_owned()))
                    }
                })
                .collect();
            for (role, alias, from) in stranded {
                match role {
                    "extraction" => &mut next.extraction,
                    "chat" => &mut next.chat,
                    _ => &mut next.rewrite,
                }
                .reasoning = "off".into();
                notices.push(format!(
                    "{role} reasoning reset to off: {alias} does not publish {from}."
                ));
            }
            if let Some(list) = p.get("groups").and_then(Value::as_array) {
                let mut groups = Vec::new();
                for (i, g) in list.iter().enumerate() {
                    let row = i + 1;
                    let m = g
                        .get("model")
                        .and_then(Value::as_str)
                        .filter(|m| !m.is_empty() && model(m).is_some());
                    let name = g
                        .get("group")
                        .and_then(Value::as_str)
                        .filter(|n| !n.is_empty());
                    let permits = g.get("permits").and_then(Value::as_u64);
                    let Some(m) = m else {
                        return Err(MoveError::invalid(format!(
                            "Row {row}: each row needs a listed model."
                        )));
                    };
                    let Some(name) = name else {
                        return Err(MoveError::invalid(format!(
                            "Row {row}: every row needs a group name."
                        )));
                    };
                    let Some(permits) = permits else {
                        // A cleared number input arrives as null.
                        return Err(MoveError::invalid(format!(
                            "Row {row}: permits must be a whole number."
                        )));
                    };
                    if permits > MAX_PERMITS {
                        return Err(MoveError::invalid(format!(
                            "Row {row}: permits are at most {MAX_PERMITS}."
                        )));
                    }
                    groups.push(Group {
                        model: m.into(),
                        group: name.into(),
                        permits: permits as u32,
                    });
                }
                next.groups = groups;
            }
            // The startup check is also the save check: nothing that would stop the bot is saved.
            let errors: Vec<String> = capacity_check(&next)
                .into_iter()
                .filter(|c| c["level"] == "error")
                .filter_map(|c| c["message"].as_str().map(str::to_owned))
                .collect();
            if !errors.is_empty() {
                return Err(MoveError::Invalid(errors.join(" ")));
            }
        }
        self.config = next;
        let mut view = self.config_view();
        view["notices"] = json!(notices);
        Ok(view)
    }

    pub fn reload_profiles(&self) -> Value {
        json!({ "message": "Reloaded 4 reply profiles from config/personas/profiles/.", "reloaded": 4 })
    }

    pub fn post_digest(&self, week: &str, channel: Option<&str>) -> Result<Value, MoveError> {
        let label = match week {
            "this" => "this week's",
            "next" => "next week's",
            _ => return Err(MoveError::invalid("Week is this or next.")),
        };
        let name = match channel {
            None => "#boss-schedule",
            Some(id) => seed::channel(id)
                .map(|c| c.1)
                .ok_or(MoveError::invalid("No such channel."))?,
        };
        Ok(
            json!({ "message": format!("Posted {label} digest in {name}; people are named, not pinged.") }),
        )
    }

    /// v4 access.html: the bot's role permissions per channel.
    pub fn access(&self) -> Value {
        let rows: Vec<Value> = seed::CHANNELS
            .iter()
            .map(|(id, name, watched)| {
                let missing_send = *id == "bm-trio";
                let missing_manage = *id == "seren-trio" || *id == "bm-trio";
                json!({
                    "id": id, "name": name, "watched": watched, "digest": false,
                    "view": true, "send": !missing_send, "history": true, "embed": true, "react": true,
                    "manage_messages": !missing_manage,
                })
            })
            .chain(std::iter::once(json!({
                "id": "boss-schedule", "name": "#boss-schedule", "watched": false, "digest": true,
                "view": true, "send": true, "history": true, "embed": true, "react": true, "manage_messages": true,
            })))
            .collect();
        json!({ "connected": true, "checked_at": Self::when(Self::now_minute()), "rows": rows })
    }
}

/// One section per request; only the writable fields below. Anything else —
/// a read-only derivation (`effective_mode`, `capacity_check`, `env`, …) or
/// an unknown key — is refused with 422, like the backend.
fn check_patch_keys(patch: &Value) -> Result<(), MoveError> {
    let bad = |path: &str| MoveError::invalid(format!("Unknown or read-only setting: {path}."));
    let obj = patch.as_object().ok_or_else(|| bad("(request)"))?;
    for (section, body) in obj {
        let keys: &[&str] = match section.as_str() {
            "pings" => &["day_of_ping_time", "countdown_minutes"],
            "watching" => &["paused", "extract_enabled"],
            "chatbot" => &["enabled", "member_rate", "guild_rate"],
            "persona" => &["active", "role_profiles", "visibility"],
            "models" => &["roles", "groups"],
            "self_service" => &["mode", "public_portal"],
            "notifications" => &["quiet_mode"],
            _ => return Err(bad(section)),
        };
        let body = body.as_object().ok_or_else(|| bad(section))?;
        for (key, value) in body {
            if !keys.contains(&key.as_str()) {
                return Err(bad(&format!("{section}.{key}")));
            }
            match (section.as_str(), key.as_str()) {
                ("chatbot", "member_rate" | "guild_rate") => {
                    for nested in ["count", "window_s"] {
                        if value.get(nested).is_none() {
                            return Err(bad(&format!("{section}.{key}.{nested}")));
                        }
                    }
                    if !value.as_object().is_some_and(|o| {
                        o.keys()
                            .all(|k| ["count", "window_s"].contains(&k.as_str()))
                    }) {
                        return Err(bad(&format!("{section}.{key}")));
                    }
                }
                ("persona", "role_profiles") => {
                    let list = value
                        .as_array()
                        .ok_or_else(|| bad(&format!("{section}.{key}")))?;
                    for item in list {
                        if !item.as_object().is_some_and(|o| {
                            o.keys()
                                .all(|k| ["role_id", "role_name", "profile"].contains(&k.as_str()))
                        }) {
                            return Err(bad(&format!("{section}.{key}[]")));
                        }
                    }
                }
                ("persona", "visibility") => {
                    let list = value
                        .as_array()
                        .ok_or_else(|| bad(&format!("{section}.{key}")))?;
                    for item in list {
                        if !item.as_object().is_some_and(|o| {
                            o.keys().all(|k| ["key", "public"].contains(&k.as_str()))
                        }) {
                            return Err(bad(&format!("{section}.{key}[]")));
                        }
                    }
                }
                ("models", "roles") => {
                    let roles = value
                        .as_object()
                        .ok_or_else(|| bad(&format!("{section}.{key}")))?;
                    for (role, fields) in roles {
                        if !["extraction", "chat", "rewrite"].contains(&role.as_str()) {
                            return Err(bad(&format!("{section}.{key}.{role}")));
                        }
                        if !fields.as_object().is_some_and(|o| {
                            o.keys()
                                .all(|k| ["alias", "reasoning"].contains(&k.as_str()))
                        }) {
                            return Err(bad(&format!("{section}.{key}.{role}")));
                        }
                    }
                }
                ("models", "groups") => {
                    let list = value
                        .as_array()
                        .ok_or_else(|| bad(&format!("{section}.{key}")))?;
                    for item in list {
                        if !item.as_object().is_some_and(|o| {
                            o.keys()
                                .all(|k| ["model", "group", "permits"].contains(&k.as_str()))
                        }) {
                            return Err(bad(&format!("{section}.{key}[]")));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::store;
    use super::leaves_homelab;
    use serde_json::json;

    #[test]
    fn capacity_follows_per_alias_admission() {
        let mut s = store();
        // extract admits 1: a second permit is refused like startup.
        let over = json!({ "models": { "groups": [
            { "model": "kanata/extract", "group": "extract", "permits": 2 },
            { "model": "kanata/chat", "group": "chat", "permits": 4 },
            { "model": "kanata/rewrite-small", "group": "rewrite", "permits": 1 },
        ] } });
        let err = s.patch_config(&over).unwrap_err();
        assert!(err.to_string().contains("admits at most 1"), "{err}");
        // Two identical zero-permit rows name their rows, not one shared message.
        let zeroes = json!({ "models": { "groups": [
            { "model": "kanata/extract", "group": "extract", "permits": 0 },
            { "model": "kanata/extract", "group": "extract", "permits": 0 },
        ] } });
        let err = s.patch_config(&zeroes).unwrap_err().to_string();
        assert!(err.contains("Row 1") && err.contains("Row 2"), "{err}");
        // One alias in two groups, and twice in one group, are both refused.
        let split = json!({ "models": { "groups": [
            { "model": "kanata/chat", "group": "chat", "permits": 2 },
            { "model": "kanata/chat", "group": "chat-2", "permits": 1 },
            { "model": "kanata/extract", "group": "extract", "permits": 1 },
            { "model": "kanata/rewrite-small", "group": "rewrite", "permits": 1 },
        ] } });
        assert!(s.patch_config(&split).is_err());
        let twice = json!({ "models": { "groups": [
            { "model": "kanata/chat", "group": "chat", "permits": 2 },
            { "model": "kanata/chat", "group": "chat", "permits": 1 },
            { "model": "kanata/extract", "group": "extract", "permits": 1 },
            { "model": "kanata/rewrite-small", "group": "rewrite", "permits": 1 },
        ] } });
        assert!(s.patch_config(&twice).is_err());
        // A cleared permits input arrives as null.
        let nulls = json!({ "models": { "groups": [
            { "model": "kanata/extract", "group": "extract", "permits": null },
        ] } });
        let err = s.patch_config(&nulls).unwrap_err().to_string();
        assert!(err.contains("Row 1") && err.contains("permits"), "{err}");
        // An unknown alias and an empty group name name their rows.
        let unknown = json!({ "models": { "groups": [
            { "model": "kanata/gone", "group": "extract", "permits": 1 },
        ] } });
        assert!(
            s.patch_config(&unknown)
                .unwrap_err()
                .to_string()
                .contains("Row 1")
        );
        let empty = json!({ "models": { "groups": [
            { "model": "kanata/extract", "group": "", "permits": 1 },
        ] } });
        assert!(
            s.patch_config(&empty)
                .unwrap_err()
                .to_string()
                .contains("Row 1")
        );
    }

    #[test]
    fn role_without_a_group_warns_but_saves() {
        let mut s = store();
        let view = s
            .patch_config(&json!({ "models": { "groups": [
                { "model": "kanata/extract", "group": "extract", "permits": 1 },
                { "model": "kanata/chat", "group": "chat", "permits": 2 },
                { "model": "kanata/chat-cloud", "group": "chat", "permits": 2 },
            ] } }))
            .ok()
            .unwrap();
        let warnings: Vec<&str> = view["models"]["capacity_check"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["level"] == "warning")
            .filter_map(|c| c["message"].as_str())
            .collect();
        assert!(
            warnings
                .iter()
                .any(|m| m.contains("rewrite") && m.contains("no capacity group")),
            "{warnings:?}"
        );
    }

    #[test]
    fn key_sum_over_the_key_limit_is_refused() {
        let mut s = store();
        let err = s
            .patch_config(&json!({ "models": { "groups": [
                { "model": "kanata/extract", "group": "extract", "permits": 1 },
                { "model": "kanata/chat", "group": "chat", "permits": 4 },
                { "model": "kanata/chat-cloud", "group": "chat-2", "permits": 4 },
                { "model": "kanata/rewrite-small", "group": "rewrite", "permits": 1 },
            ] } }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("key admits 8"), "{err}");
    }

    #[test]
    fn reasoning_resolution_follows_published_efforts() {
        let mut s = store();
        // The model-decides alias takes v4's low/medium/high, not minimal.
        let ok = s
            .patch_config(&json!({ "models": { "roles": { "rewrite": { "alias": "kanata/legacy", "reasoning": "high" } } } }))
            .ok()
            .unwrap();
        assert_eq!(ok["models"]["roles"]["rewrite"]["reasoning"], "high");
        assert!(
            s.patch_config(
                &json!({ "models": { "roles": { "rewrite": { "reasoning": "minimal" } } } })
            )
            .is_err(),
            "model-decides takes low/medium/high"
        );
        // Inherit is fine while extraction's medium is published for chat…
        let ok = s
            .patch_config(&json!({ "models": { "roles": { "chat": { "reasoning": "" } } } }))
            .ok()
            .unwrap();
        assert_eq!(ok["models"]["roles"]["chat"]["reasoning"], "");
        // …but moving extraction to high strands it: reset to off, with a notice.
        let view = s
            .patch_config(
                &json!({ "models": { "roles": { "extraction": { "reasoning": "high" } } } }),
            )
            .ok()
            .unwrap();
        assert_eq!(view["models"]["roles"]["chat"]["reasoning"], "off");
        assert!(
            view["notices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n.as_str().is_some_and(|n| n.contains("reset to off"))),
            "{}",
            view["notices"]
        );
        // An explicit level the new alias does not publish is rejected.
        assert!(
            s.patch_config(&json!({ "models": { "roles": { "chat": { "reasoning": "high" } } } }))
                .is_err()
        );
        // Extraction cannot inherit.
        assert!(
            s.patch_config(
                &json!({ "models": { "roles": { "extraction": { "reasoning": "" } } } })
            )
            .is_err()
        );
        // Chat still needs tools.
        assert!(
            s.patch_config(
                &json!({ "models": { "roles": { "chat": { "alias": "kanata/extract" } } } })
            )
            .is_err(),
            "chat needs tools"
        );
    }

    #[test]
    fn inherit_checks_the_final_extraction_effort_in_one_request() {
        let mut s = store();
        // The app sends all three roles at once: extraction moves to high
        // while chat still inherits; kanata/chat publishes low/medium only.
        let err = s
            .patch_config(&json!({ "models": { "roles": {
                "extraction": { "alias": "kanata/extract", "reasoning": "high" },
                "chat": { "alias": "kanata/chat", "reasoning": "" },
                "rewrite": { "alias": "kanata/rewrite-small", "reasoning": "off" },
            } } }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("inherits high"), "{err}");
        assert_eq!(s.config.chat.reasoning, "");
        assert_eq!(s.config.extraction.reasoning, "medium");
        // Same request with chat off saves.
        let view = s
            .patch_config(&json!({ "models": { "roles": {
                "extraction": { "reasoning": "high" },
                "chat": { "reasoning": "off" },
            } } }))
            .ok()
            .unwrap();
        assert_eq!(view["models"]["roles"]["chat"]["reasoning"], "off");
    }

    #[test]
    fn permits_over_the_bound_are_refused_not_clamped() {
        let mut s = store();
        let err = s
            .patch_config(&json!({ "models": { "groups": [
                { "model": "kanata/extract", "group": "extract", "permits": 65 },
            ] } }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("Row 1") && err.contains("at most 64"), "{err}");
    }

    #[test]
    fn unknown_and_readonly_patch_keys_are_422() {
        let mut s = store();
        for patch in [
            json!({ "models": { "kanata_limits": [] } }),
            json!({ "models": { "roles": { "chat": { "model": "x" } } } }),
            json!({ "self_service": { "effective_mode": "cards_only" } }),
            json!({ "env": [] }),
            json!({ "nope": {} }),
        ] {
            let err = s.patch_config(&patch).unwrap_err();
            assert!(err.to_string().contains("Unknown or read-only"), "{patch}");
        }
    }

    #[test]
    fn profile_visibility_and_reload() {
        let mut s = store();
        let view = s
            .patch_config(
                &json!({ "persona": { "visibility": [{ "key": "sparkly", "public": true }] } }),
            )
            .ok()
            .unwrap();
        let sparkly = view["persona"]["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["key"] == "sparkly")
            .unwrap();
        assert_eq!(sparkly["public"], true);
        assert!(sparkly["voice"].as_str().is_some_and(|v| !v.is_empty()));
        assert!(
            sparkly["prompt_summary"]
                .as_str()
                .is_some_and(|v| !v.is_empty())
        );
        assert!(
            s.patch_config(
                &json!({ "persona": { "visibility": [{ "key": "nope", "public": true }] } })
            )
            .is_err()
        );
        let reloaded = s.reload_profiles();
        assert!(
            reloaded["message"]
                .as_str()
                .is_some_and(|m| m.contains("4 reply profiles"))
        );
    }

    #[test]
    fn cloud_suffix_leaves_the_homelab() {
        assert!(leaves_homelab("kanata/rewrite-cloud", "homelab"));
        assert!(leaves_homelab("kanata/chat-cloud", "external"));
        assert!(!leaves_homelab("kanata/chat", "homelab"));
        assert!(leaves_homelab("kanata/legacy", "unknown"));
    }

    #[test]
    fn public_portal_off_forces_cards_only() {
        let mut s = store();
        let v = s
            .patch_config(
                &json!({ "self_service": { "mode": "link_first", "public_portal": false } }),
            )
            .ok()
            .unwrap();
        assert_eq!(v["self_service"]["mode"], "link_first");
        assert_eq!(v["self_service"]["effective_mode"], "cards_only");
    }
}
