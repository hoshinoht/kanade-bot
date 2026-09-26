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
    /// `kanade.toml` `[[models.groups]]`; None runs the default group.
    pub declared_groups: Option<Vec<Group>>,
    /// `models.permits`: the default group's permits.
    pub permits: u32,
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
const MODELS: [ModelInfo; 7] = [
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
    // Requires reasoning: its published list has no `none`, so `off` is hidden.
    ModelInfo {
        id: "kanata/think",
        trust: "homelab",
        tools: true,
        json: true,
        sampling: true,
        efforts: Some(&["low", "high"]),
        route_max: Some(1),
        adapter_max: None,
        declared_max: None,
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

/// Kanata publishes no per-key limit; the key is shared with the owner's other clients.
const KEY_SHARED: bool = true;

/// Aliases whose published efforts lack `none`: reasoning cannot be off.
const OFF_REQUIRED: [&str; 1] = ["kanata/think"];

/// Listed `<base>:<level>` variants (Kanata names a fixed-effort route this
/// way): the picker lists the base and shows a variant only when stored.
const VARIANTS: [(&str, &str, &str); 3] = [
    ("kanata/chat:high", "kanata/chat", "high"),
    ("kanata/chat:low", "kanata/chat", "low"),
    ("kanata/extract:none", "kanata/extract", "off"),
];

fn variant(id: &str) -> Option<(&'static str, &'static str)> {
    VARIANTS
        .iter()
        .find(|(v, _, _)| *v == id)
        .map(|(_, base, effort)| (*base, *effort))
}

/// The groups the governor runs: `kanade.toml` groups, or one `gateway` group
/// of `models.permits` over the distinct role aliases.
pub fn effective_groups(c: &Config) -> Vec<Group> {
    if let Some(declared) = &c.declared_groups {
        return declared.clone();
    }
    let mut aliases: Vec<&str> = Vec::new();
    for m in [&c.extraction, &c.chat, &c.rewrite] {
        if !m.alias.is_empty() && !aliases.contains(&m.alias.as_str()) {
            aliases.push(&m.alias);
        }
    }
    aliases
        .into_iter()
        .map(|alias| Group {
            model: alias.into(),
            group: "gateway".into(),
            permits: c.permits,
        })
        .collect()
}

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
/// shows them read-only. Kanade's voice ends in a period and its summary is
/// Markdown, as a real file's can be.
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
        "comedy.",
        "## Kanade\n- **Teases** lightly in the persona's voice\n- keeps every *schedule* fact exact",
    ),
    (
        "sparkly",
        "Sparkly",
        false,
        "Overexcited kouhai",
        "Bursts with enthusiasm and exclamation, still lands the facts. Private while it settles in.",
    ),
];

/// As the server's default (`models.pseudonymize` off); read-only there too.
pub const PII_PSEUDONYMISE: bool = false;

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
        // The default: one gateway group over the role aliases.
        declared_groups: None,
        permits: 1,
    }
}

/// A listed alias, or a listed variant's base (whose capabilities it shares).
fn model(id: &str) -> Option<&'static ModelInfo> {
    let id = variant(id).map_or(id, |(base, _)| base);
    MODELS.iter().find(|m| m.id == id)
}

fn profile_visible(c: &Config, key: &str) -> bool {
    c.profile_visibility
        .iter()
        .find(|v| v.key == key)
        .map(|v| v.public)
        .unwrap_or(false)
}

/// The server's startup check (also run on every save): with declared groups,
/// a role model outside every group is warned about; each group's permits are
/// held to the least Kanata admits among its aliases. Nothing about the key:
/// Kanata publishes no per-key limit.
pub fn capacity_check(c: &Config) -> Vec<Value> {
    let groups = effective_groups(c);
    let mut out = Vec::new();
    if c.declared_groups.is_some() {
        for (role, m) in [
            ("extraction", &c.extraction),
            ("chat", &c.chat),
            ("rewrite", &c.rewrite),
        ] {
            if !m.alias.is_empty() && !groups.iter().any(|g| g.model == m.alias) {
                out.push(json!({ "level": "warning", "message": format!("The {role} model {} is in no capacity group; its calls are refused.", m.alias) }));
            }
        }
    }
    let mut names: Vec<&str> = Vec::new();
    for g in &groups {
        if !names.contains(&g.group.as_str()) {
            names.push(&g.group);
        }
    }
    for name in names {
        let rows: Vec<&Group> = groups.iter().filter(|g| g.group == name).collect();
        let permits = rows[0].permits;
        let mut cap: Option<(u32, &str)> = None;
        for g in &rows {
            match model(&g.model) {
                None => out.push(json!({ "level": "error", "message": format!("Kanata does not list {}.", g.model) })),
                Some(info) => match info.cap() {
                    Some(each) => {
                        if cap.is_none_or(|(least, _)| each < least) {
                            cap = Some((each, &g.model));
                        }
                    }
                    None => out.push(json!({ "level": "warning", "message": format!("Kanata publishes no limit for {}; its calls queue at the gateway.", g.model) })),
                },
            }
        }
        if let Some((cap, by)) = cap {
            out.push(if permits > cap {
                json!({ "level": "error", "message": format!("Group {name} declares {permits} permits but Kanata admits at most {cap} (capped by {by}); the bot refuses to start.") })
            } else if permits < cap {
                json!({ "level": "warning", "message": format!("Group {name} uses {permits} of the {cap} permits Kanata admits.") })
            } else {
                json!({ "level": "ok", "message": format!("Group {name}: {permits} permits, matching Kanata's limit.") })
            });
        }
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
/// Every stored reasoning level after `off`, in the server's order.
const ALL_EFFORTS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

fn valid_reasoning(info: &ModelInfo, effort: &str) -> bool {
    if effort == "off" {
        return true;
    }
    match info.efforts {
        // `null`: Kanata restricts nothing, so every level is accepted.
        None => ALL_EFFORTS.contains(&effort),
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
        let entry = |id: &str, m: &ModelInfo| {
            json!({
                "id": id, "trust_zone": m.trust, "leaves_homelab": leaves_homelab(id, m.trust),
                "function_tools": m.tools, "structured_output": m.json, "sampling_controls": m.sampling,
                "reasoning_control": m.efforts.is_none_or(|e| !e.is_empty()),
                "reasoning_efforts": m.efforts,
                "off_allowed": !OFF_REQUIRED.contains(&m.id),
                "admission": m.cap().map(|max| admission(max, m.adapter_max)),
            })
        };
        let mut models: Vec<Value> = MODELS.iter().map(|m| entry(m.id, m)).collect();
        for (id, base, effort) in VARIANTS {
            if let Some(info) = model(base) {
                let mut v = entry(id, info);
                v["variant_of"] = json!(base);
                v["fixed_effort"] = json!(effort);
                models.push(v);
            }
        }
        // A stored variant is shown as "<base> (fixed: <level>)".
        let role = |r: &RoleModel| {
            let mut v = json!(r);
            if let Some((base, effort)) = variant(&r.alias) {
                v["variant_of"] = json!(base);
                v["fixed_effort"] = json!(effort);
            }
            v
        };
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
                "roles": { "extraction": role(&c.extraction), "chat": role(&c.chat), "rewrite": role(&c.rewrite) },
                "groups": effective_groups(c),
                "groups_source": if c.declared_groups.is_some() { "config" } else { "default" },
                // Variants are listed too, as the server does; the app shows base models only.
                "alias_limits": MODELS.iter().map(|m| (m.id, m)).chain(VARIANTS.iter().filter_map(|(id, base, _)| model(base).map(|m| (*id, m)))).filter_map(|(id, m)| m.cap().map(|max| {
                    let mut limit = admission(max, m.adapter_max);
                    limit["alias"] = json!(id);
                    limit["source"] = json!(m.source());
                    limit
                })).collect::<Vec<_>>(),
                "key_limits": { "max_in_flight": null, "shared": KEY_SHARED },
                "capacity_check": capacity_check(c),
                "pii_pseudonymise": PII_PSEUDONYMISE,
            },
            "manage_messages": { "missing": missing_manage },
            "env": [
                { "key": "KANADE_TIMEZONE", "label": "Timezone", "value": "Asia/Kuala_Lumpur", "reason": "Every stored time is converted with it; a change needs a restart." },
                { "key": "KANADE_RESET", "label": "Boss week starts", "value": "Thu 00:00", "reason": "Defines boss-week boundaries for every stored run." },
                { "key": "KANATA_BASE_URL", "label": "Model gateway", "value": "https://kanata.example.internal", "reason": "The gateway address is deployment wiring; repointing it would redirect the bearer key, so only the operator changes it." },
                { "key": "KANADE_PSEUDONYMIZE", "label": "PII pseudonymisation", "value": if PII_PSEUDONYMISE { "on" } else { "off" }, "reason": if PII_PSEUDONYMISE { "Member names and ids reach every model as per-request fictional names; a request still carrying one is refused. A privacy control only the operator may change (kanade.toml models.pseudonymize)." } else { "Off: member names reach the model as written, so models that leave the homelab are refused. A privacy control only the operator may change (kanade.toml models.pseudonymize)." } },
                { "key": "KANADE_ALLOW_EXTERNAL_UNMASKED", "label": "Unmasked external models", "value": "off", "reason": if PII_PSEUDONYMISE { "Unused while pseudonymisation is on: models that leave the homelab only ever see masked data." } else { "Lets models that leave the homelab see member data unmasked (provider testing only); a privacy control only the operator may change." } },
                { "key": "KANADE_MODEL_GROUPS", "label": "Capacity groups", "value": if c.declared_groups.is_some() { "declared in kanade.toml" } else { "default: one gateway group" }, "reason": "Set in kanade.toml ([[models.groups]]); restart to apply." },
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
        if let Some(active) = patch.pointer("/persona/active").and_then(Value::as_str) {
            if !PERSONAS.iter().any(|(k, _, _)| *k == active) {
                return Err(bad("No such persona in the catalog."));
            }
            next.persona = active.into();
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
                    let offered: Vec<&str> = match info.efforts {
                        None => std::iter::once("off").chain(ALL_EFFORTS).collect(),
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
            // As the server: contracted as editable, but it cannot store them yet.
            if matches!(
                (section.as_str(), key.as_str()),
                ("models", "groups") | ("persona", "role_profiles" | "visibility")
            ) {
                return Err(MoveError::Coded(
                    422,
                    "read_only",
                    format!("{section}.{key}: saving it is not supported yet."),
                ));
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
    fn groups_role_profiles_and_visibility_are_read_only() {
        let mut s = store();
        for patch in [
            json!({ "models": { "groups": [] } }),
            json!({ "persona": { "role_profiles": [] } }),
            json!({ "persona": { "visibility": [{ "key": "sparkly", "public": true }] } }),
        ] {
            match s.patch_config(&patch) {
                Err(crate::mock::MoveError::Coded(422, "read_only", _)) => {}
                Err(other) => panic!("{patch}: wanted read_only, got {other}"),
                Ok(_) => panic!("{patch}: saved"),
            }
        }
        // The server's startup check still describes the seeded groups.
        let view = s.config_view();
        assert!(
            !view["models"]["capacity_check"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn reasoning_resolution_follows_published_efforts() {
        let mut s = store();
        // `null` efforts: Kanata restricts nothing, so every level is accepted.
        for level in ["minimal", "high", "xhigh", "max"] {
            let ok = s
                .patch_config(&json!({ "models": { "roles": { "rewrite": { "alias": "kanata/legacy", "reasoning": level } } } }))
                .ok()
                .unwrap();
            assert_eq!(ok["models"]["roles"]["rewrite"]["reasoning"], level);
        }
        assert!(
            s.patch_config(
                &json!({ "models": { "roles": { "rewrite": { "reasoning": "ultra" } } } })
            )
            .is_err(),
            "not a level"
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
    fn profiles_read_and_reload() {
        let s = store();
        let view = s.config_view();
        let sparkly = view["persona"]["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["key"] == "sparkly")
            .unwrap();
        assert!(sparkly["voice"].as_str().is_some_and(|v| !v.is_empty()));
        assert!(
            sparkly["prompt_summary"]
                .as_str()
                .is_some_and(|v| !v.is_empty())
        );
        let reloaded = s.reload_profiles();
        assert!(
            reloaded["message"]
                .as_str()
                .is_some_and(|m| m.contains("4 reply profiles"))
        );
    }

    #[test]
    fn capacity_reports_the_groups_the_governor_runs() {
        let mut s = store();
        let view = s.config_view();
        assert_eq!(view["models"]["groups_source"], "default");
        assert!(view["models"]["key_limits"]["max_in_flight"].is_null());
        let groups = view["models"]["groups"].as_array().unwrap();
        assert!(groups.iter().all(|g| g["group"] == "gateway"));
        assert_eq!(groups.len(), 3, "one row per distinct role alias");
        let checks: Vec<&str> = view["models"]["capacity_check"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| c["message"].as_str())
            .collect();
        assert_eq!(
            checks,
            ["Group gateway: 1 permits, matching Kanata's limit."]
        );
        // Declared groups: a role model outside every group is warned about.
        s.config.declared_groups = Some(vec![super::Group {
            model: "kanata/chat".into(),
            group: "chat".into(),
            permits: 2,
        }]);
        let view = s.config_view();
        assert_eq!(view["models"]["groups_source"], "config");
        let warnings: Vec<&str> = view["models"]["capacity_check"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["level"] == "warning")
            .filter_map(|c| c["message"].as_str())
            .collect();
        assert!(warnings.contains(
            &"The extraction model kanata/extract is in no capacity group; its calls are refused."
        ));
        assert!(warnings.contains(&"Group chat uses 2 of the 4 permits Kanata admits."));
        // Variants are listed with their base; the reasoning-only model hides `off`.
        let catalog = view["models"]["catalog"].as_array().unwrap();
        let chat_high = catalog
            .iter()
            .find(|m| m["id"] == "kanata/chat:high")
            .unwrap();
        assert_eq!(
            (
                chat_high["variant_of"].as_str(),
                chat_high["fixed_effort"].as_str()
            ),
            (Some("kanata/chat"), Some("high"))
        );
        let think = catalog.iter().find(|m| m["id"] == "kanata/think").unwrap();
        assert_eq!(think["off_allowed"], false);
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
