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

use super::catalog::valid_run_length_override;
use super::clock;
use super::model_context::{self, Route};
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

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct RoleProfile {
    pub role_id: String,
    pub profile: String,
}

#[derive(Clone, Serialize)]
pub struct ProfileVisibility {
    pub key: String,
    pub public: bool,
}

#[derive(Clone, Serialize)]
pub struct RunLengthOverride {
    pub boss: String,
    pub difficulty: String,
    pub minutes: u32,
}

#[derive(Clone, Serialize)]
pub struct RunLengths {
    pub default_minutes: u32,
    pub overrides: Vec<RunLengthOverride>,
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
    pub role_profiles_revision: u64,
    pub profile_visibility: Vec<ProfileVisibility>,
    pub extraction: RoleModel,
    pub chat: RoleModel,
    pub rewrite: RoleModel,
    /// `kanade.toml` `[[models.groups]]`; None runs the default group.
    pub declared_groups: Option<Vec<Group>>,
    /// `models.permits`: the default group's permits.
    pub permits: u32,
    /// `models.context`: defaults, per-role reserve/cap and alias overrides.
    pub context: model_context::Settings,
    pub run_lengths: RunLengths,
    /// Roles with a model when the bot started: extraction and heading
    /// rewrites run only for those until a restart.
    #[serde(skip)]
    pub started: Vec<&'static str>,
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
    /// Published context window and completion maximum, when listed.
    context_tokens: Option<u32>,
    max_output_tokens: Option<u32>,
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
        context_tokens: Some(16_384),
        max_output_tokens: Some(4_096),
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
        context_tokens: None,
        max_output_tokens: Some(8_192),
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
        context_tokens: Some(8_192),
        max_output_tokens: Some(512),
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
        context_tokens: Some(200_000),
        max_output_tokens: Some(16_384),
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
        context_tokens: None,
        max_output_tokens: None,
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
        context_tokens: Some(65_536),
        max_output_tokens: Some(8_192),
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
        context_tokens: Some(32_768),
        max_output_tokens: Some(1_024),
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

/// The server's env rows (`src/api/admin/config/desk.rs`): same keys, labels
/// and reasons; `copy` is the raw env form (ids comma-joined, lowercase
/// weekday), null when unset.
fn env_rows(c: &Config) -> Vec<Value> {
    let row = |key: &str, label: &str, value: String, reason: &str, copy: Option<String>| json!({ "key": key, "label": label, "value": value, "reason": reason, "copy": copy });
    let watched: Vec<_> = seed::CHANNELS.iter().filter(|c| c.2).collect();
    let mut groups: Vec<(&str, u32)> = Vec::new();
    for g in c.declared_groups.iter().flatten() {
        if !groups.iter().any(|(name, _)| *name == g.group) {
            groups.push((&g.group, g.permits));
        }
    }
    let capacity = if groups.is_empty() {
        row(
            "KANADE_MODEL_PERMITS",
            "Model permits",
            c.permits.to_string(),
            "One capacity group shared by every model role; kanade.toml [[models.groups]] replaces it after a restart.",
            Some(c.permits.to_string()),
        )
    } else {
        let each: Vec<String> = groups
            .iter()
            .map(|(name, permits)| format!("{name} {permits}"))
            .collect();
        let noun = if groups.len() == 1 { "group" } else { "groups" };
        row(
            "KANADE_MODEL_PERMITS / kanade.toml [[models.groups]]",
            "Model capacity groups",
            format!("{} {noun}: {}", groups.len(), each.join(", ")),
            "Set in kanade.toml ([[models.groups]]); restart to apply.",
            None,
        )
    };
    vec![
        row(
            "KANADE_TIMEZONE",
            "Timezone",
            "Asia/Kuala_Lumpur".into(),
            "Every stored time is converted with it; a change needs a restart.",
            Some("Asia/Kuala_Lumpur".into()),
        ),
        row(
            "KANADE_BOSS_WEEK_RESET_WEEKDAY",
            "Boss week starts",
            "Thu 00:00".into(),
            "Defines the boss-week boundaries of every stored run.",
            Some("thu".into()),
        ),
        row(
            "KANADE_POST_CHANNEL_ID",
            "Digest channel",
            "#boss-schedule".into(),
            "Set with the guild's channel layout.",
            Some("boss-schedule".into()),
        ),
        row(
            "KANADE_WATCH_CHANNEL_IDS",
            "Watched channels",
            watched.iter().map(|c| c.1).collect::<Vec<_>>().join(", "),
            "Watching a new channel is a deliberate deploy.",
            Some(watched.iter().map(|c| c.0).collect::<Vec<_>>().join(",")),
        ),
        row(
            "KANADE_WATCH_CATEGORY_IDS",
            "Watched categories",
            "none".into(),
            "Watching a whole category is a deliberate deploy, like channels.",
            None,
        ),
        row(
            "KANADE_CHAT_CATEGORY_IDS",
            "Chat categories",
            "none".into(),
            "The chatbot answers in every channel of these categories; set with the channel layout.",
            None,
        ),
        row(
            "KANADE_CHAT_PILOT_ROLE_ID",
            "Chat pilot role",
            "not set".into(),
            "Who may talk to the chatbot is a deployment decision.",
            None,
        ),
        row(
            "KANADE_MODEL_BASE_URL",
            "Model gateway",
            "https://kanata.example.internal".into(),
            "Repointing the gateway would redirect the bearer key, so only the operator changes it.",
            Some("https://kanata.example.internal".into()),
        ),
        capacity,
    ]
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

/// One governor group as Limits shows it: its aliases, its permits, and the
/// synthetic permits in flight (by position: the first full, the second
/// half-used, the rest idle). Config's `in_use` reads the same numbers.
pub struct LiveGroup {
    pub name: String,
    pub models: Vec<String>,
    pub total: u32,
    pub in_use: u32,
}

pub fn live_groups(c: &Config) -> Vec<LiveGroup> {
    let mut out: Vec<LiveGroup> = Vec::new();
    for g in effective_groups(c) {
        match out.iter_mut().find(|live| live.name == g.group) {
            Some(live) => live.models.push(g.model),
            None => out.push(LiveGroup {
                in_use: match out.len() {
                    0 => g.permits,
                    1 => g.permits / 2,
                    _ => 0,
                },
                name: g.group,
                models: vec![g.model],
                total: g.permits,
            }),
        }
    }
    out
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

/// Required ConfigView compatibility field; model requests are not pseudonymised.
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
                profile: "terse".into(),
            },
            RoleProfile {
                role_id: "300002".into(),
                profile: "default".into(),
            },
        ],
        role_profiles_revision: 1,
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
        context: model_context::Settings::default(),
        run_lengths: RunLengths {
            default_minutes: 30,
            overrides: vec![RunLengthOverride {
                boss: "BM".into(),
                difficulty: "h".into(),
                minutes: 60,
            }],
        },
        started: vec!["extraction", "chat", "rewrite"],
    }
}

/// A listed alias, or a listed variant's base (whose capabilities it shares).
fn model(id: &str) -> Option<&'static ModelInfo> {
    let id = variant(id).map_or(id, |(base, _)| base);
    MODELS.iter().find(|m| m.id == id)
}

/// What context resolution sees for an alias: unlisted routes are not local.
fn route(id: &str) -> Option<Route> {
    model(id).map(|m| Route {
        local: !leaves_homelab(id, m.trust),
        context_tokens: m.context_tokens,
        max_output_tokens: m.max_output_tokens,
    })
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
                out.push(json!({ "level": "warning", "message": format!("The {role} model {} is in no capacity group; its calls are refused.", m.alias), "group": null }));
            }
        }
    }
    let mut names: Vec<&str> = Vec::new();
    for g in &groups {
        if !names.contains(&g.group.as_str()) {
            names.push(&g.group);
        }
    }
    // An unlisted alias is reported once, under the first group naming it.
    let mut unlisted: Vec<&str> = Vec::new();
    for name in names {
        let rows: Vec<&Group> = groups.iter().filter(|g| g.group == name).collect();
        let permits = rows[0].permits;
        let mut cap: Option<(u32, &str)> = None;
        for g in &rows {
            match model(&g.model) {
                None => {
                    if !unlisted.contains(&g.model.as_str()) {
                        unlisted.push(&g.model);
                        out.push(json!({ "level": "error", "message": format!("Kanata does not list {}.", g.model), "group": name }));
                    }
                }
                Some(info) => match info.cap() {
                    Some(each) => {
                        if cap.is_none_or(|(least, _)| each < least) {
                            cap = Some((each, &g.model));
                        }
                    }
                    None => out.push(json!({ "level": "warning", "message": format!("Kanata publishes no limit for {}; its calls queue at the gateway.", g.model), "group": name })),
                },
            }
        }
        if let Some((cap, by)) = cap {
            out.push(if permits > cap {
                json!({ "level": "error", "message": format!("Group {name} declares {permits} permits but Kanata admits at most {cap} (capped by {by}); the bot refuses to start."), "group": name })
            } else if permits < cap {
                json!({ "level": "warning", "message": format!("Group {name} uses {permits} of the {cap} permits Kanata admits."), "group": name })
            } else {
                json!({ "level": "ok", "message": format!("Group {name}: {permits} permits, matching Kanata's limit."), "group": name })
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

/// Serve composes extraction and the heading rewriter at startup only.
fn awaiting_restart(c: &Config, role: &str) -> bool {
    let alias = match role {
        "extraction" => &c.extraction.alias,
        "rewrite" => &c.rewrite.alias,
        _ => return false,
    };
    !alias.is_empty() && !c.started.contains(&role)
}

fn resolve<'a>(reasoning: &'a str, extraction: &'a str) -> &'a str {
    if reasoning.is_empty() {
        extraction
    } else {
        reasoning
    }
}

impl Store {
    pub fn roles() -> Value {
        json!([
            { "id": "300001", "name": "staff", "color": "#e0a458" },
            { "id": "300003", "name": "bossers", "color": "#5b8def" },
            { "id": "300002", "name": "newbies" },
        ])
    }

    fn guild_role_name(id: &str) -> Option<&'static str> {
        match id {
            "300001" => Some("staff"),
            "300002" => Some("newbies"),
            "300003" => Some("bossers"),
            _ => None,
        }
    }

    fn role_profiles_digest(&self) -> String {
        format!("role-profiles-v{}", self.config.role_profiles_revision)
    }

    pub fn config_view(&self) -> Value {
        let c = &self.config;
        let live = live_groups(c);
        let groups: Vec<Value> = effective_groups(c)
            .into_iter()
            .map(|g| {
                let in_use = live.iter().find(|l| l.name == g.group).map(|l| l.in_use);
                json!({ "model": g.model, "group": g.group, "permits": g.permits, "in_use": in_use })
            })
            .collect();
        let entry = |id: &str, m: &ModelInfo| {
            let mut entry = json!({
                "id": id, "trust_zone": m.trust, "leaves_homelab": leaves_homelab(id, m.trust),
                "function_tools": m.tools, "structured_output": m.json, "sampling_controls": m.sampling,
                "reasoning_control": m.efforts.is_none_or(|e| !e.is_empty()),
                "reasoning_efforts": m.efforts,
                "off_allowed": !OFF_REQUIRED.contains(&m.id),
                "admission": m.cap().map(|max| admission(max, m.adapter_max)),
            });
            // Optional in the DTO: absent when Kanata publishes nothing.
            if let Some(tokens) = m.context_tokens {
                entry["context_tokens"] = json!(tokens);
            }
            if let Some(tokens) = m.max_output_tokens {
                entry["max_output_tokens"] = json!(tokens);
            }
            entry
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
        // A stored variant is shown as "<base> (fixed: <level>)"; saved roles
        // run at once (`running`: inherit and a variant's level resolved).
        let role = |name: &str, r: &RoleModel| {
            let mut v = json!(r);
            let mut running = resolve(&r.reasoning, &c.extraction.reasoning).to_owned();
            if let Some((base, effort)) = variant(&r.alias) {
                v["variant_of"] = json!(base);
                v["fixed_effort"] = json!(effort);
                running = effort.into();
            }
            if !r.alias.is_empty() && !awaiting_restart(c, name) {
                v["running"] = json!({ "alias": r.alias, "reasoning": running });
            }
            if !r.alias.is_empty() {
                v["context"] = c.context.resolve(name, &r.alias, route(&r.alias));
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
                "role_profiles": c.role_profiles.iter().map(|assignment| json!({
                    "role_id": assignment.role_id,
                    "role_name": Self::guild_role_name(&assignment.role_id),
                    "profile": assignment.profile,
                })).collect::<Vec<_>>(),
                "role_profiles_digest": self.role_profiles_digest(),
            },
            "models": {
                "reachable": true,
                "catalog": models,
                "roles": { "extraction": role("extraction", &c.extraction), "chat": role("chat", &c.chat), "rewrite": role("rewrite", &c.rewrite) },
                "groups": groups,
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
                "context": c.context,
            },
            "run_lengths": c.run_lengths,
            "manage_messages": { "missing": missing_manage },
            "env": env_rows(c),
            "last_digest": self.last_digest(),
            "notices": Vec::<String>::new(),
        })
    }

    /// One section per request, each partial; arrays are replaced whole.
    /// Unknown or read-only keys are refused with 422, like the backend.
    pub fn patch_config(&mut self, patch: &Value) -> Result<Value, MoveError> {
        check_patch_keys(patch)?;
        let persona = patch.get("persona");
        let role_profiles = persona
            .and_then(|p| p.get("role_profiles"))
            .and_then(Value::as_array);
        let role_digest = persona
            .and_then(|p| p.get("role_profiles_digest"))
            .and_then(Value::as_str);
        if role_profiles.is_some() != role_digest.is_some() {
            return Err(MoveError::invalid(
                "Role assignments and role_profiles_digest must be sent together.",
            ));
        }
        if role_profiles.is_some() && role_digest != Some(self.role_profiles_digest().as_str()) {
            return Err(MoveError::Coded(
                409,
                "conflict",
                "Role assignments changed since they were loaded; reload the latest configuration."
                    .into(),
            ));
        }
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
        if let Some(list) = patch
            .pointer("/persona/visibility")
            .and_then(Value::as_array)
        {
            let mut seen = Vec::with_capacity(list.len());
            for item in list {
                let key = item
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or(bad("Each visibility update needs a profile key."))?;
                if !PROFILES.iter().any(|(profile, ..)| *profile == key) {
                    return Err(bad(&format!("No reply profile named {key}.")));
                }
                if seen.contains(&key) {
                    return Err(bad("A visibility update cannot repeat a profile."));
                }
                seen.push(key);
                let public = item
                    .get("public")
                    .and_then(Value::as_bool)
                    .ok_or(bad("Each visibility update needs a public value."))?;
                if let Some(saved) = next.profile_visibility.iter_mut().find(|v| v.key == key) {
                    saved.public = public;
                } else {
                    next.profile_visibility.push(ProfileVisibility {
                        key: key.into(),
                        public,
                    });
                }
            }
        }
        if let Some(list) = role_profiles {
            let mut assignments = Vec::with_capacity(list.len());
            for item in list {
                let role_id = item
                    .get("role_id")
                    .and_then(Value::as_str)
                    .ok_or(bad("Each role assignment needs a role_id."))?;
                let profile = item
                    .get("profile")
                    .and_then(Value::as_str)
                    .ok_or(bad("Each role assignment needs a profile."))?;
                if assignments
                    .iter()
                    .any(|assignment: &RoleProfile| assignment.role_id == role_id)
                {
                    return Err(bad("A role can have only one reply profile assignment."));
                }
                if !PROFILES.iter().any(|(key, ..)| *key == profile) {
                    return Err(bad(&format!("No reply profile named {profile}.")));
                }
                if Self::guild_role_name(role_id).is_none()
                    && !self
                        .config
                        .role_profiles
                        .iter()
                        .any(|saved| saved.role_id == role_id && saved.profile == profile)
                {
                    return Err(bad(
                        "A new or changed role assignment must use a role in the current guild directory.",
                    ));
                }
                assignments.push(RoleProfile {
                    role_id: role_id.into(),
                    profile: profile.into(),
                });
            }
            if next.role_profiles != assignments {
                next.role_profiles = assignments;
                next.role_profiles_revision = self.config.role_profiles_revision + 1;
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
                    // Declared groups: a switch to an alias none lists is refused.
                    if let Some(declared) = &self.config.declared_groups
                        && slot.alias != alias
                        && !declared.iter().any(|g| g.model == alias)
                    {
                        return Err(MoveError::Coded(
                            422,
                            "ungrouped",
                            format!(
                                "The {role} model {alias} is in no capacity group; add it to [[models.groups]] in kanade.toml and restart, or pick a grouped model."
                            ),
                        ));
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
            if let Some(context) = p.get("context") {
                next.context = model_context::parse(context).map_err(MoveError::Invalid)?;
            }
            // Every models save re-checks the context against the final roles.
            let local_warning = next
                .context
                .validate(
                    [
                        ("extraction", next.extraction.alias.as_str()),
                        ("chat", next.chat.alias.as_str()),
                        ("rewrite", next.rewrite.alias.as_str()),
                    ],
                    route,
                )
                .map_err(MoveError::Invalid)?;
            if local_warning {
                notices.push(model_context::LOCAL_WARNING.into());
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
        if let Some(p) = patch.get("run_lengths") {
            if let Some(minutes) = p.get("default_minutes").and_then(Value::as_u64) {
                if !(5..=240).contains(&minutes) {
                    return Err(bad("The default run length is 5-240 whole minutes."));
                }
                next.run_lengths.default_minutes = minutes as u32;
            } else if p.get("default_minutes").is_some() {
                return Err(bad("The default run length is 5-240 whole minutes."));
            }
            if let Some(values) = p.get("overrides").and_then(Value::as_array) {
                let mut overrides = Vec::with_capacity(values.len());
                let mut seen = Vec::with_capacity(values.len());
                for value in values {
                    let boss = value
                        .get("boss")
                        .and_then(Value::as_str)
                        .filter(|boss| !boss.is_empty())
                        .ok_or(bad("Pick a catalog boss key."))?;
                    let difficulty = value
                        .get("difficulty")
                        .and_then(Value::as_str)
                        .filter(|difficulty| !difficulty.is_empty())
                        .ok_or(bad("Pick a difficulty that boss has."))?;
                    if !valid_run_length_override(boss, difficulty) {
                        return Err(bad("Pick a catalog boss key and difficulty."));
                    }
                    let minutes = value
                        .get("minutes")
                        .and_then(Value::as_u64)
                        .filter(|minutes| (5..=480).contains(minutes))
                        .ok_or(bad("An override run length is 5-480 whole minutes."))?
                        as u32;
                    if seen.contains(&(boss, difficulty)) {
                        return Err(bad(
                            "A boss difficulty can have only one run-length override.",
                        ));
                    }
                    seen.push((boss, difficulty));
                    overrides.push(RunLengthOverride {
                        boss: boss.into(),
                        difficulty: difficulty.into(),
                        minutes,
                    });
                }
                next.run_lengths.overrides = overrides;
            } else if p.get("overrides").is_some() {
                return Err(bad("run_lengths.overrides must be an array of overrides."));
            }
        }
        for (role, feature, before, after) in [
            (
                "extraction",
                "extraction",
                &self.config.extraction,
                &next.extraction,
            ),
            (
                "rewrite",
                "heading rewrites",
                &self.config.rewrite,
                &next.rewrite,
            ),
        ] {
            if awaiting_restart(&next, role) && before.alias != after.alias {
                notices.push(format!(
                    "The {role} model had none when the bot started: restart to start {feature} with {}.",
                    after.alias
                ));
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

    pub fn post_digest(&mut self, week: &str, channel: Option<&str>) -> Result<Value, MoveError> {
        let (label, next) = match week {
            "this" => ("this week's", false),
            "next" => ("next week's", true),
            _ => return Err(MoveError::invalid("Week is this or next.")),
        };
        if self.digest_week == Some(true) && !next {
            return Err(MoveError::invalid(
                "A newer week's digest is already posted.",
            ));
        }
        let name = match channel {
            None => "#boss-schedule",
            Some(id) => seed::channel(id)
                .map(|c| c.1)
                .ok_or(MoveError::invalid("No such channel."))?,
        };
        self.digest_week = Some(next);
        Ok(
            json!({ "message": format!("Posted {label} digest in {name}; people are named, not pinged.") }),
        )
    }

    /// The newest digest card: a manual post from this session, else the
    /// seeded Thursday 00:15 post of the current boss week in `#boss-schedule`.
    fn last_digest(&self) -> Value {
        let (today, minute) = clock::local_now();
        let this_week = clock::week_start(today);
        let (week, day, minute) = match self.digest_week {
            Some(next) => (this_week + if next { 7 } else { 0 }, today, minute),
            None => (this_week, this_week, 15),
        };
        let posted_at = format!(
            "{}T{:02}:{:02}:00+08:00",
            clock::iso_date(day),
            minute / 60,
            minute % 60
        );
        let message = format!("digest-{}", clock::iso_date(week));
        json!({
            "posted_at": posted_at,
            "week_start": clock::iso_date(week),
            "this_week": week == this_week,
            "channel_id": "boss-schedule",
            "channel_name": "#boss-schedule",
            "url": format!("https://discord.com/channels/0/boss-schedule/{message}"),
        })
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
    let (section, body) = match (obj.iter().next(), obj.len()) {
        (None, _) => return Err(MoveError::invalid("Send one settings section.")),
        (Some(_), 1) => obj.iter().next().unwrap(),
        _ => return Err(MoveError::invalid("Save one section at a time.")),
    };
    {
        let keys: &[&str] = match section.as_str() {
            "pings" => &["day_of_ping_time", "countdown_minutes"],
            "watching" => &["paused", "extract_enabled"],
            "chatbot" => &["enabled", "member_rate", "guild_rate"],
            "persona" => &[
                "active",
                "role_profiles",
                "role_profiles_digest",
                "visibility",
            ],
            "models" => &["roles", "groups", "context"],
            "self_service" => &["mode", "public_portal"],
            "notifications" => &["quiet_mode"],
            "run_lengths" => &["default_minutes", "overrides"],
            _ => return Err(bad(section)),
        };
        let body = body.as_object().ok_or_else(|| bad(section))?;
        if body.is_empty() {
            return Err(MoveError::invalid(format!(
                "Send at least one {section} setting."
            )));
        }
        for (key, value) in body {
            if !keys.contains(&key.as_str()) {
                return Err(bad(&format!("{section}.{key}")));
            }
            // As the server: contracted as editable, but it cannot store them yet.
            if matches!((section.as_str(), key.as_str()), ("models", "groups")) {
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
                                .all(|k| ["role_id", "profile"].contains(&k.as_str()))
                                && o.contains_key("role_id")
                                && o.contains_key("profile")
                        }) {
                            return Err(bad(&format!("{section}.{key}[]")));
                        }
                    }
                }
                ("persona", "role_profiles_digest") => {
                    if !value.as_str().is_some_and(|digest| !digest.is_empty()) {
                        return Err(bad(&format!("{section}.{key}")));
                    }
                }
                ("persona", "visibility") => {
                    let list = value
                        .as_array()
                        .ok_or_else(|| bad(&format!("{section}.{key}")))?;
                    for item in list {
                        if !item.as_object().is_some_and(|o| {
                            o.keys().all(|k| ["key", "public"].contains(&k.as_str()))
                                && o.contains_key("key")
                                && o.contains_key("public")
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
                ("run_lengths", "overrides") => {
                    let list = value
                        .as_array()
                        .ok_or_else(|| bad(&format!("{section}.{key}")))?;
                    for item in list {
                        if !item.as_object().is_some_and(|o| {
                            o.keys()
                                .all(|k| ["boss", "difficulty", "minutes"].contains(&k.as_str()))
                                && o.contains_key("boss")
                                && o.contains_key("difficulty")
                                && o.contains_key("minutes")
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
    fn groups_remain_read_only() {
        let mut s = store();
        let patch = json!({ "models": { "groups": [] } });
        match s.patch_config(&patch) {
            Err(crate::mock::MoveError::Coded(422, "read_only", _)) => {}
            Err(other) => panic!("{patch}: wanted read_only, got {other}"),
            Ok(_) => panic!("{patch}: saved"),
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
    fn config_view_names_check_groups_env_copies_and_the_last_digest() {
        let mut s = store();
        let view = s.config_view();
        for check in view["models"]["capacity_check"].as_array().unwrap() {
            assert_eq!(check["group"], "gateway", "{check}");
        }
        let copy = |view: &serde_json::Value, key: &str| {
            view["env"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["key"] == key)
                .unwrap()["copy"]
                .clone()
        };
        assert_eq!(copy(&view, "KANADE_BOSS_WEEK_RESET_WEEKDAY"), "thu");
        assert_eq!(copy(&view, "KANADE_WATCH_CATEGORY_IDS"), json!(null));
        assert_eq!(copy(&view, "KANADE_MODEL_PERMITS"), "1");
        // Pinned at Tue 29 Sep 12:00: the boss week began Thu 24 Sep.
        assert_eq!(
            view["last_digest"],
            json!({
                "posted_at": "2026-09-24T00:15:00+08:00",
                "week_start": "2026-09-24",
                "this_week": true,
                "channel_id": "boss-schedule",
                "channel_name": "#boss-schedule",
                "url": "https://discord.com/channels/0/boss-schedule/digest-2026-09-24",
            })
        );
        assert!(s.post_digest("next", None).is_ok());
        let view = s.config_view();
        assert_eq!(view["last_digest"]["week_start"], "2026-10-01");
        assert_eq!(view["last_digest"]["this_week"], false);
        assert_eq!(
            view["last_digest"]["posted_at"],
            "2026-09-29T12:00:00+08:00"
        );
    }

    #[test]
    fn run_lengths_seed_the_week_and_follow_a_config_save() {
        let mut s = store();
        let initial = s.week(false);
        assert_eq!(
            initial
                .runs
                .iter()
                .find(|run| run.id == "r-kalos")
                .unwrap()
                .minutes,
            30
        );
        let multi = initial
            .runs
            .iter()
            .find(|run| run.bosses.len() >= 2)
            .unwrap();
        assert_eq!(multi.minutes, multi.bosses.len() as u32 * 30);
        let saved = s
            .patch_config(&json!({ "run_lengths": { "default_minutes": 20 } }))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(saved["run_lengths"]["default_minutes"], 20);
        assert_eq!(
            s.week(false)
                .runs
                .iter()
                .find(|run| run.id == "r-kalos")
                .unwrap()
                .minutes,
            20
        );
        for patch in [
            json!({ "run_lengths": { "overrides": [{ "boss": "Ghost", "difficulty": "h", "minutes": 60 }] } }),
            json!({ "run_lengths": { "overrides": [{ "boss": "BM", "difficulty": "n", "minutes": 60 }] } }),
            json!({ "run_lengths": { "overrides": [
                { "boss": "BM", "difficulty": "h", "minutes": 60 },
                { "boss": "BM", "difficulty": "h", "minutes": 75 }
            ] } }),
        ] {
            assert!(s.patch_config(&patch).is_err(), "{patch}");
        }
    }

    #[test]
    fn config_patch_needs_one_nonempty_section() {
        let mut s = store();
        for (patch, message) in [
            (json!({}), "Send one settings section."),
            (
                json!({ "run_lengths": {} }),
                "Send at least one run_lengths setting.",
            ),
            (
                json!({ "run_lengths": { "default_minutes": 20 }, "notifications": { "quiet_mode": true } }),
                "Save one section at a time.",
            ),
        ] {
            let error = s.patch_config(&patch).unwrap_err().to_string();
            assert_eq!(error, message, "{patch}");
        }
    }

    #[test]
    fn privacy_compatibility_field_stays_false_without_retired_env_rows() {
        let view = store().config_view();
        assert_eq!(view["models"]["pii_pseudonymise"], false);
        let env = view["env"].as_array().unwrap();
        for key in ["KANADE_PSEUDONYMIZE", "KANADE_ALLOW_EXTERNAL_UNMASKED"] {
            assert!(
                !env.iter().any(|row| row["key"] == key),
                "retired env row {key} remains"
            );
        }
    }

    #[test]
    fn role_profiles_replace_whole_and_reject_a_stale_digest() {
        let mut s = store();
        let before = s.config_view();
        let digest = before["persona"]["role_profiles_digest"].as_str().unwrap();
        let view = s
            .patch_config(&json!({ "persona": {
                "role_profiles": [
                    { "role_id": "300003", "profile": "sparkly" },
                    { "role_id": "300001", "profile": "kanade" }
                ],
                "role_profiles_digest": digest
            } }))
            .unwrap_or_else(|error| panic!("{error}"));
        let assignments = view["persona"]["role_profiles"].as_array().unwrap();
        assert_eq!(assignments.len(), 2);
        assert_eq!(assignments[0]["role_id"], "300003");
        assert_eq!(assignments[0]["role_name"], "bossers");
        assert_eq!(assignments[0]["profile"], "sparkly");
        assert_eq!(assignments[1]["role_id"], "300001");
        let next_digest = view["persona"]["role_profiles_digest"].as_str().unwrap();
        assert_ne!(digest, next_digest);

        let stale = s
            .patch_config(&json!({ "persona": {
                "role_profiles": [],
                "role_profiles_digest": digest
            } }))
            .unwrap_err();
        assert!(matches!(
            stale,
            crate::mock::MoveError::Coded(409, "conflict", _)
        ));
        assert_eq!(
            s.config_view()["persona"]["role_profiles"],
            view["persona"]["role_profiles"]
        );
    }

    #[test]
    fn missing_roles_can_be_reordered_or_removed_but_not_changed_or_rebound() {
        let mut s = store();
        s.config.role_profiles.push(super::RoleProfile {
            role_id: "390009".into(),
            profile: "terse".into(),
        });
        let view = s.config_view();
        let stale_digest = view["persona"]["role_profiles_digest"].as_str().unwrap();
        assert!(view["persona"]["role_profiles"][2]["role_name"].is_null());

        let reordered = s
            .patch_config(&json!({ "persona": {
                "role_profiles": [
                    { "role_id": "390009", "profile": "terse" },
                    { "role_id": "300002", "profile": "default" }
                ],
                "role_profiles_digest": stale_digest
            } }))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            reordered["persona"]["role_profiles"][0]["role_id"],
            "390009"
        );
        let current_digest = reordered["persona"]["role_profiles_digest"]
            .as_str()
            .unwrap();

        let changed_profile = s.patch_config(&json!({ "persona": {
            "role_profiles": [{ "role_id": "390009", "profile": "default" }],
            "role_profiles_digest": current_digest
        } }));
        assert!(
            changed_profile.is_err(),
            "a missing role's profile was changed"
        );

        let removed = s
            .patch_config(&json!({ "persona": {
                "role_profiles": [{ "role_id": "300002", "profile": "default" }],
                "role_profiles_digest": current_digest
            } }))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            removed["persona"]["role_profiles"]
                .as_array()
                .unwrap()
                .len(),
            1
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
    fn profile_visibility_deltas_merge_and_reload() {
        let mut s = store();
        let stale_view = s.config_view();
        assert_eq!(
            stale_view["persona"]["profiles"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["key"] == "sparkly")
                .unwrap()["public"],
            false,
            "the seeded profile fixture remains mixed"
        );

        s.patch_config(
            &json!({ "persona": { "visibility": [{ "key": "sparkly", "public": true }] } }),
        )
        .ok()
        .unwrap();
        // A second admin submits a selected-key delta from the same old read.
        // It must not republish that stale full list over the first change.
        let view = s
            .patch_config(
                &json!({ "persona": { "visibility": [{ "key": "terse", "public": false }] } }),
            )
            .ok()
            .unwrap();
        let profiles = view["persona"]["profiles"].as_array().unwrap();
        let public = |key: &str| {
            profiles.iter().find(|p| p["key"] == key).unwrap()["public"]
                .as_bool()
                .unwrap()
        };
        assert!(public("sparkly"));
        assert!(!public("terse"));
        assert!(public("default"));
        assert!(public("kanade"));

        // An absent saved row is private, not implicitly published.
        s.config.profile_visibility.retain(|v| v.key != "default");
        assert_eq!(
            s.config_view()["persona"]["profiles"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["key"] == "default")
                .unwrap()["public"],
            false
        );
    }

    #[test]
    fn profile_visibility_rejects_unknown_or_duplicate_delta_keys() {
        let mut s = store();
        for patch in [
            json!({ "persona": { "visibility": [{ "key": "missing", "public": true }] } }),
            json!({ "persona": { "visibility": [
                { "key": "sparkly", "public": true },
                { "key": "sparkly", "public": false }
            ] } }),
        ] {
            assert!(s.patch_config(&patch).is_err(), "accepted {patch}");
        }
    }

    #[test]
    fn config_and_limits_name_the_same_groups_and_load() {
        let mut s = store();
        for declared in [false, true] {
            if declared {
                s.config.declared_groups = Some(vec![
                    super::Group {
                        model: "kanata/extract".into(),
                        group: "local".into(),
                        permits: 1,
                    },
                    super::Group {
                        model: "kanata/chat".into(),
                        group: "chat".into(),
                        permits: 4,
                    },
                ]);
            }
            let config = s.config_view();
            let limits = s.limits();
            let rows = config["models"]["groups"].as_array().unwrap();
            let groups = limits["groups"].as_array().unwrap();
            for g in groups {
                for row in rows.iter().filter(|r| r["group"] == g["name"]) {
                    assert_eq!(row["in_use"], g["permits"]["in_use"], "{row}");
                    assert_eq!(row["permits"], g["permits"]["total"], "{row}");
                }
            }
            let mut named: Vec<&serde_json::Value> = rows.iter().map(|r| &r["group"]).collect();
            named.dedup();
            assert_eq!(named, groups.iter().map(|g| &g["name"]).collect::<Vec<_>>());
            assert!(
                rows.iter().any(|r| r["in_use"].as_u64() > Some(0)),
                "something is in flight"
            );
        }
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
        // Switching a role to an alias no declared group lists is refused.
        let refused = s
            .patch_config(
                &json!({ "models": { "roles": { "rewrite": { "alias": "kanata/legacy" } } } }),
            )
            .unwrap_err();
        assert!(matches!(
            refused,
            crate::mock::MoveError::Coded(422, "ungrouped", _)
        ));
        // A first extraction model waits for a restart, and says so.
        s.config.declared_groups = None;
        s.config.extraction.alias.clear();
        s.config.started.retain(|role| *role != "extraction");
        let saved = s
            .patch_config(
                &json!({ "models": { "roles": { "extraction": { "alias": "kanata/extract" } } } }),
            )
            .ok()
            .expect("saved");
        assert_eq!(
            saved["notices"],
            json!([
                "The extraction model had none when the bot started: restart to start extraction with kanata/extract."
            ])
        );
        assert!(
            saved["models"]["roles"]["extraction"]
                .get("running")
                .is_none()
        );
        // Saved roles run at once: the view names what the next call uses.
        assert_eq!(
            view["models"]["roles"]["chat"]["running"]["alias"],
            view["models"]["roles"]["chat"]["alias"]
        );
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
    fn context_saves_whole_resolves_per_role_and_warns_for_local_routes() {
        let mut s = store();
        let view = s.config_view();
        assert_eq!(view["models"]["context"]["local_default"], 8_192);
        assert!(view["models"]["context"]["chat"]["cap"].is_null());
        assert_eq!(
            view["models"]["roles"]["chat"]["context"]["source"],
            "local_default"
        );
        assert_eq!(
            view["models"]["roles"]["extraction"]["context"]["source"],
            "catalog"
        );
        let context = |local: u32, overrides: serde_json::Value| {
            json!({ "models": { "context": {
                "cloud_default": 65_536, "local_default": local,
                "chat": { "reserve": 1_024, "cap": null },
                "extraction": { "reserve": 2_500, "cap": null },
                "rewrite": { "reserve": 96, "cap": null },
                "overrides": overrides,
            } } })
        };
        // A local role past 16k saves, with the server's notice.
        let saved = s.patch_config(&context(24_576, json!({}))).ok().unwrap();
        assert_eq!(
            saved["models"]["roles"]["chat"]["context"]["window"],
            24_576
        );
        assert_eq!(
            saved["models"]["roles"]["chat"]["context"]["local_warning"],
            true
        );
        assert_eq!(saved["notices"], json!([model_context_warning()]));
        // Above the alias's published window is refused, naming it.
        let err = s
            .patch_config(&context(8_192, json!({ "kanata/extract": 20_000 })))
            .unwrap_err()
            .to_string();
        assert!(err.contains("published context window (16384)"), "{err}");
        // A partial object is refused whole.
        let err = s
            .patch_config(&json!({ "models": { "context": { "cloud_default": 1 } } }))
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "models.context must be a complete context settings object."
        );
        // Cloud routes never warn.
        let saved = s
            .patch_config(&context(8_192, json!({ "kanata/chat-cloud": 100_000 })))
            .ok()
            .unwrap();
        assert_eq!(saved["notices"], json!([]));
        assert_eq!(
            s.config_view()["models"]["context"]["overrides"]["kanata/chat-cloud"],
            100_000
        );
    }

    fn model_context_warning() -> &'static str {
        super::model_context::LOCAL_WARNING
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
