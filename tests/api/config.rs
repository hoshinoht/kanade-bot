//! A9 runtime settings over the seeded SQLite store of `reads.rs`, a fake
//! model catalog and temp persona files. Responses are validated against
//! `config.json`; refusals against `error.json`.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use kanade::{
    api::admin::config::{
        CatalogRead, ConfigDesk, ConfigFacts, ConfigFuture, ConfigInputs, ModelCatalog,
        PersonaFiles,
    },
    chat::persona::{
        PersonaId, PersonaRoot, PersonaSnapshot, PersonaStore, ProfileId, ProfileQuery,
        ProfileSource,
    },
    domain::settings::{Models, Reasoning, RoleModel, RuntimeSettings, load_settings},
    infrastructure::llm::{
        AdmissionLimits, Effort, TrustZone,
        governor::Role,
        setup::{CapacityGroup, CatalogModel, CatalogSnapshot, RoleSwap, RunningRole},
    },
    infrastructure::store::SqliteStore,
};
use serde_json::{Value, json};

use crate::{
    reads::{EDGE_HEADERS, Reads},
    schemas::assert_valid,
    support::{ADMIN_HOST, Reply, request, send},
};

const VIEW: &str = "config.json#/$defs/ConfigView";
const ERROR: &str = "error.json#/$defs/ApiError";
const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
const PATH: &str = "/api/admin/config";
const RELOAD: &str = "/api/admin/config/profiles/reload";

fn model(
    alias: &str,
    zone: Option<TrustZone>,
    tools: bool,
    efforts: Option<&[Effort]>,
    admission: Option<(u32, Option<u32>)>,
) -> CatalogModel {
    CatalogModel {
        alias: alias.into(),
        published: true,
        trust_zone: zone,
        leaves_homelab: !matches!(zone, Some(TrustZone::Local)) || alias.ends_with("-cloud"),
        reasoning_control: efforts.is_none_or(|efforts| !efforts.is_empty()),
        reasoning_efforts: efforts.map(<[Effort]>::to_vec),
        structured_output: true,
        sampling_controls: true,
        function_tools: tools,
        context_tokens: None,
        admission: admission.map(|(max, adapter)| AdmissionLimits {
            max_in_flight: max,
            max_queue: None,
            queue_ms: None,
            adapter_max_in_flight: adapter,
        }),
    }
}

fn catalog() -> CatalogSnapshot {
    use Effort::{High, Low, Medium};
    let local = Some(TrustZone::Local);
    CatalogSnapshot {
        listed: true,
        models: vec![
            model(
                "kanata/extract",
                local,
                false,
                Some(&[Low, Medium, High]),
                Some((2, Some(4))),
            ),
            model(
                "kanata/chat",
                local,
                true,
                Some(&[Low, Medium]),
                Some((4, Some(8))),
            ),
            model(
                "kanata/rewrite-small",
                local,
                false,
                Some(&[]),
                Some((2, None)),
            ),
            model("kanata/legacy", None, true, None, Some((8, None))),
            model(
                "kanata/chat-cloud",
                local,
                true,
                Some(&[Low]),
                Some((4, None)),
            ),
            model("kanata/tiny", local, false, Some(&[Low]), Some((1, None))),
            // A reasoning variant of kanata/chat, and a `:level` whose base is unlisted.
            model(
                "kanata/chat:medium",
                local,
                true,
                Some(&[Low, Medium]),
                Some((4, Some(8))),
            ),
            model(
                "kanata/solo:low",
                local,
                true,
                Some(&[Low]),
                Some((4, None)),
            ),
        ],
    }
}

/// Records applied roles; `running` is whatever a test sets.
pub struct FakeCatalog(
    Mutex<CatalogRead>,
    Mutex<Vec<Models>>,
    Mutex<BTreeMap<Role, RunningRole>>,
    Mutex<Vec<Role>>,
);

impl FakeCatalog {
    fn applied(&self) -> Vec<Models> {
        self.1.lock().unwrap().clone()
    }

    fn set_reachable(&self, reachable: bool) {
        self.0.lock().unwrap().reachable = reachable;
    }

    fn remove(&self, alias: &str) {
        self.0
            .lock()
            .unwrap()
            .snapshot
            .models
            .retain(|model| model.alias != alias);
    }
}

impl ModelCatalog for FakeCatalog {
    fn read(&self) -> ConfigFuture<'_, CatalogRead> {
        let read = self.0.lock().unwrap().clone();
        Box::pin(async move { read })
    }

    fn apply(&self, models: &Models) -> Result<Vec<RoleSwap>, String> {
        self.1.lock().unwrap().push(models.clone());
        Ok(Vec::new())
    }

    fn running(&self) -> BTreeMap<Role, RunningRole> {
        self.2.lock().unwrap().clone()
    }

    fn awaiting_restart(&self) -> Vec<Role> {
        self.3.lock().unwrap().clone()
    }
}

fn role(alias: &str, reasoning: Reasoning) -> RoleModel {
    RoleModel {
        alias: Some(alias.into()),
        reasoning,
    }
}

fn settings() -> RuntimeSettings {
    let mut settings = RuntimeSettings::default();
    settings.models.extraction = role("kanata/extract", Reasoning::Medium);
    settings.models.chat = role("kanata/chat", Reasoning::Inherit);
    settings.models.rewrite = role("kanata/rewrite-small", Reasoning::Off);
    settings
}

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// `kanade` and `calm` bundles, one `calm` reply profile and a broken one.
struct PersonaDir(PathBuf);

impl PersonaDir {
    fn new() -> Self {
        let base = fs::canonicalize(std::env::temp_dir()).unwrap();
        let root = base.join(format!("kanade-config-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("bundles")).unwrap();
        fs::create_dir_all(root.join("profiles")).unwrap();
        let bundle = fs::read_to_string(repo("config/personas/bundles/kanade.yaml")).unwrap();
        fs::write(root.join("bundles/kanade.yaml"), &bundle).unwrap();
        fs::write(
            root.join("bundles/calm.yaml"),
            bundle.replacen("\nid: kanade\n", "\nid: calm\n", 1),
        )
        .unwrap();
        fs::write(
            root.join("catalog.yaml"),
            "schema_version: 1\ndefault: kanade\npersonas:\n  - id: kanade\n    label: Kanade\n    aliases: []\n  - id: calm\n    label: Calm\n    aliases: []\n",
        )
        .unwrap();
        let dir = Self(root);
        dir.profile("calm");
        fs::write(dir.0.join("profiles/broken.yaml"), "id: [\n").unwrap();
        dir
    }

    fn profile(&self, id: &str) {
        let example = fs::read_to_string(repo("config/personas/profiles/example.yaml")).unwrap();
        fs::write(
            self.0.join(format!("profiles/{id}.yaml")),
            example.replace("id: example", &format!("id: {id}")),
        )
        .unwrap();
    }
}

impl Drop for PersonaDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Config {
    reads: Reads,
    desk: Arc<ConfigDesk>,
    catalog: Arc<FakeCatalog>,
    personas: Arc<PersonaStore>,
    dir: PersonaDir,
}

impl Config {
    async fn new() -> Self {
        Self::build(true).await
    }

    async fn build(gateway: bool) -> Self {
        Self::with_groups(gateway, Vec::new()).await
    }

    async fn with_groups(gateway: bool, groups: Vec<CapacityGroup>) -> Self {
        Self::with_settings(gateway, groups, settings()).await
    }

    async fn with_settings(
        gateway: bool,
        groups: Vec<CapacityGroup>,
        settings: RuntimeSettings,
    ) -> Self {
        Self::with_settings_and_logins(gateway, groups, settings, false).await
    }

    async fn with_two_admins() -> Self {
        Self::with_settings_and_logins(true, Vec::new(), settings(), true).await
    }

    async fn with_settings_and_logins(
        gateway: bool,
        groups: Vec<CapacityGroup>,
        settings: RuntimeSettings,
        two_admins: bool,
    ) -> Self {
        let dir = PersonaDir::new();
        let root = PersonaRoot::open(&dir.0).unwrap();
        let personas = Arc::new(PersonaStore::new(PersonaSnapshot::startup(&root, None)));
        let catalog = Arc::new(FakeCatalog(
            Mutex::new(CatalogRead {
                reachable: true,
                snapshot: catalog(),
            }),
            Mutex::default(),
            Mutex::default(),
            Mutex::default(),
        ));
        let slot = Arc::new(OnceLock::new());
        let reads = {
            let (slot, catalog, personas, dir) = (
                slot.clone(),
                catalog.clone(),
                personas.clone(),
                dir.0.clone(),
            );
            let make = move |store: Arc<SqliteStore>| {
                let desk = Arc::new(ConfigDesk::new(ConfigInputs {
                    settings,
                    store,
                    models: gateway.then_some(catalog as Arc<dyn ModelCatalog>),
                    facts: ConfigFacts {
                        timezone: "Asia/Kuala_Lumpur".into(),
                        model_gateway: gateway.then(|| "https://kanata.test/v1".into()),
                        model_permits: 2,
                        model_groups: groups.clone(),
                        allow_external_unmasked: true,
                        pseudonymize: false,
                        chat_pilot_role_id: Some("30".into()),
                    },
                    personas: Some(PersonaFiles {
                        dir,
                        store: personas,
                    }),
                }));
                slot.set(desk.clone()).ok().unwrap();
                desk
            };
            if two_admins {
                Reads::with_config_and_logins(make).await
            } else {
                Reads::with_config(make).await
            }
        };
        Self {
            reads,
            desk: slot.get().unwrap().clone(),
            catalog,
            personas,
            dir,
        }
    }

    async fn get(&self) -> Value {
        let reply = request(
            self.reads.admin,
            "GET",
            ADMIN_HOST,
            PATH,
            &[("Cookie", &self.reads.cookie)],
        )
        .await;
        view(&reply, "GET")
    }

    async fn get_as(&self, cookie: &str, edge_login: &str) -> Value {
        let mut headers = vec![("Cookie", cookie)];
        let mut edge_headers = EDGE_HEADERS;
        edge_headers[2] = ("Tailscale-User-Login", edge_login);
        headers.extend_from_slice(&edge_headers);
        let reply = request(self.reads.admin, "GET", ADMIN_HOST, PATH, &headers).await;
        view(&reply, "GET as admin")
    }

    async fn send(&self, method: &str, path: &str, key: Option<&str>, body: &Value) -> Reply {
        self.send_as(
            (&self.reads.cookie, &self.reads.csrf, None),
            method,
            path,
            key,
            body,
        )
        .await
    }

    async fn send_as(
        &self,
        session: (&str, &str, Option<&str>),
        method: &str,
        path: &str,
        key: Option<&str>,
        body: &Value,
    ) -> Reply {
        let (cookie, csrf, edge_login) = session;
        let mut headers = vec![ORIGIN, ("Cookie", cookie), ("X-Kanade-CSRF", csrf)];
        if let Some(login) = edge_login {
            let mut edge_headers = EDGE_HEADERS;
            edge_headers[2] = ("Tailscale-User-Login", login);
            headers.extend_from_slice(&edge_headers);
        }
        if let Some(key) = key {
            headers.push(("Idempotency-Key", key));
        }
        let body = body.to_string();
        send(
            self.reads.admin,
            method,
            ADMIN_HOST,
            path,
            &headers,
            Some(&body),
        )
        .await
    }

    async fn patch(&self, body: Value) -> Value {
        view(
            &self.send("PATCH", PATH, None, &body).await,
            &body.to_string(),
        )
    }

    async fn refused(&self, body: Value, status: u16, code: &str) -> String {
        let reply = self.send("PATCH", PATH, None, &body).await;
        refused(&reply, status, code, &body.to_string())
    }
}

fn view(reply: &Reply, what: &str) -> Value {
    assert_eq!(reply.status, 200, "{what}: {}", reply.text());
    let value = reply.json();
    assert_valid(VIEW, what, &value);
    value
}

fn refused(reply: &Reply, status: u16, code: &str, what: &str) -> String {
    assert_eq!(reply.status, status, "{what}: {}", reply.text());
    assert_valid(ERROR, what, &reply.json());
    assert_eq!(reply.api_error(), code, "{what}");
    reply.json()["message"].as_str().unwrap().to_owned()
}

fn roles(view: &Value) -> Value {
    view["models"]["roles"].clone()
}

#[tokio::test]
async fn get_shows_settings_models_personas_and_env_facts() {
    let config = Config::new().await;
    let view = config.get().await;
    assert_eq!(view["notices"], json!([]));
    assert_eq!(
        view["pings"],
        json!({"day_of_ping_time": "01:00", "countdown_minutes": [60]})
    );
    assert_eq!(
        view["self_service"],
        json!({"mode": "cards_and_link", "effective_mode": "cards_only", "public_portal": false})
    );
    // The pilot role is set; chat categories are not.
    assert_eq!(view["chatbot"]["configured"], false);
    assert_eq!(
        view["chatbot"]["missing_env"],
        json!(["KANADE_CHAT_CATEGORY_IDS"])
    );
    assert_eq!(view["manage_messages"], json!({"missing": []}));

    let models = &view["models"];
    assert_eq!(models["reachable"], true);
    assert_eq!(models["pii_pseudonymise"], false);
    assert_eq!(
        roles(&view),
        json!({
            "extraction": {"alias": "kanata/extract", "reasoning": "medium"},
            "chat": {"alias": "kanata/chat", "reasoning": ""},
            "rewrite": {"alias": "kanata/rewrite-small", "reasoning": "off"},
        })
    );
    let info = |id: &str| {
        models["catalog"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(info("kanata/extract")["trust_zone"], "homelab");
    assert_eq!(info("kanata/extract")["leaves_homelab"], false);
    assert_eq!(info("kanata/chat-cloud")["leaves_homelab"], true);
    assert_eq!(info("kanata/legacy")["trust_zone"], "unknown");
    assert_eq!(info("kanata/legacy")["reasoning_efforts"], Value::Null);
    assert_eq!(
        info("kanata/extract")["admission"],
        json!({"max_in_flight": 2, "adapter_max_in_flight": 4})
    );
    assert_eq!(
        info("kanata/rewrite-small")["admission"],
        json!({"max_in_flight": 2})
    );
    assert_eq!(
        models["groups"],
        json!([
            {"model": "kanata/chat", "group": "gateway", "permits": 2},
            {"model": "kanata/extract", "group": "gateway", "permits": 2},
            {"model": "kanata/rewrite-small", "group": "gateway", "permits": 2},
        ])
    );
    assert_eq!(models["groups_source"], "default");
    assert_eq!(
        models["key_limits"],
        json!({"max_in_flight": null, "shared": true})
    );
    assert!(
        models["alias_limits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|limit| limit["source"] == "published")
    );
    assert_eq!(
        models["capacity_check"],
        json!([{"level": "ok", "message": "Group gateway: 2 permits, matching Kanata's limit."}])
    );

    let env = |key: &str| {
        view["env"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == key)
            .unwrap_or_else(|| panic!("{key}"))["value"]
            .clone()
    };
    assert_eq!(env("KANADE_ALLOW_EXTERNAL_UNMASKED"), "on");
    assert_eq!(env("KANADE_PSEUDONYMIZE"), "off");
    assert_eq!(view["models"]["pii_pseudonymise"], false);
    assert_eq!(env("KANADE_TIMEZONE"), "Asia/Kuala_Lumpur");
    assert_eq!(env("KANADE_MODEL_PERMITS"), "2");
    assert_eq!(env("KANADE_BOSS_WEEK_RESET_WEEKDAY"), "Thu 00:00");

    let persona = &view["persona"];
    let keys: Vec<&str> = persona["personas"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["kanade", "calm"]);
    assert_eq!(persona["personas"][1]["bundle"], "bundles/calm.yaml");
    assert_eq!(persona["profiles"].as_array().unwrap().len(), 1);
    assert_eq!(persona["profiles"][0]["key"], "calm");
    assert_eq!(
        persona["profiles"][0]["public"], false,
        "missing is private"
    );
    assert_eq!(persona["profiles"][0]["prompt_summary"], "# Reply profile");
    assert_eq!(persona["role_profiles"], json!([]));
}

#[tokio::test]
async fn each_section_saves_normalised_and_reads_back_after_a_restart() {
    let config = Config::new().await;
    let pings = config
        .patch(json!({"pings": {"day_of_ping_time": "09:30", "countdown_minutes": [15, 60, 15]}}))
        .await;
    assert_eq!(
        pings["pings"],
        json!({"day_of_ping_time": "09:30", "countdown_minutes": [60, 15]})
    );
    config.patch(json!({"watching": {"paused": true}})).await;
    let chat = config
        .patch(json!({"chatbot": {"member_rate": {"count": 6}, "guild_rate": {"count": 20, "window_s": 600}}}))
        .await;
    assert_eq!(
        chat["chatbot"]["member_rate"],
        json!({"count": 6, "window_s": 300})
    );
    assert_eq!(
        chat["chatbot"]["guild_rate"],
        json!({"count": 20, "window_s": 600})
    );
    config
        .patch(json!({"notifications": {"quiet_mode": true}}))
        .await;
    let service = config
        .patch(json!({"self_service": {"mode": "link_first"}}))
        .await;
    assert_eq!(service["self_service"]["effective_mode"], "cards_only");
    let open = config
        .patch(json!({"self_service": {"public_portal": true}}))
        .await;
    assert_eq!(open["self_service"]["effective_mode"], "link_first");
    config
        .patch(json!({"models": {"roles": {"rewrite": {"alias": "kanata/legacy", "reasoning": "xhigh"}}}}))
        .await;

    // A restart reads the stored rows over a default seed.
    let restarted = load_settings(&*config.reads.store, &RuntimeSettings::default())
        .await
        .unwrap();
    assert_eq!(restarted.pings.countdown_minutes, [60, 15]);
    assert_eq!(restarted.pings.day_of_ping_time.to_string(), "09:30:00");
    assert!(restarted.watching.paused);
    assert!(restarted.watching.extract_enabled);
    assert_eq!(restarted.chatbot.member_rate.count, 6);
    assert_eq!(restarted.chatbot.guild_rate.window_s, 600);
    assert!(restarted.notifications.quiet_mode);
    assert!(restarted.self_service.public_portal);
    assert_eq!(restarted.self_service.mode.as_str(), "link_first");
    assert_eq!(
        restarted.models.rewrite,
        role("kanata/legacy", Reasoning::Xhigh)
    );
    assert_eq!(
        restarted.models.chat,
        role("kanata/chat", Reasoning::Inherit)
    );
    assert_eq!(restarted, config.desk.settings().await);
}

#[tokio::test]
async fn unknown_read_only_and_bad_values_are_422_and_nothing_is_saved() {
    let config = Config::new().await;
    let before = config.get().await;
    for (body, code) in [
        (json!({}), "invalid"),
        (
            json!({"pings": {"day_of_ping_time": "09:00"}, "watching": {"paused": true}}),
            "invalid",
        ),
        (json!({"pings": {}}), "invalid"),
        (json!({"nope": {"x": 1}}), "unknown_field"),
        (json!({"pings": {"timezone": "UTC"}}), "unknown_field"),
        (
            json!({"chatbot": {"member_rate": {"burst": 2}}}),
            "unknown_field",
        ),
        (
            json!({"models": {"roles": {"judge": {"alias": "kanata/chat"}}}}),
            "unknown_field",
        ),
        (
            json!({"models": {"roles": {"chat": {"model": "kanata/chat"}}}}),
            "unknown_field",
        ),
        (json!({"env": []}), "read_only"),
        (json!({"manage_messages": {"missing": []}}), "read_only"),
        (
            json!({"self_service": {"effective_mode": "cards_only"}}),
            "read_only",
        ),
        (json!({"chatbot": {"configured": true}}), "read_only"),
        (json!({"models": {"catalog": []}}), "read_only"),
        (json!({"models": {"pii_pseudonymise": true}}), "read_only"),
        (
            json!({"models": {"groups": [{"model": "kanata/chat", "group": "chat", "permits": 1}]}}),
            "read_only",
        ),
        (json!({"persona": {"role_profiles": []}}), "read_only"),
        (json!({"persona": {"profiles": []}}), "read_only"),
        (json!({"pings": {"day_of_ping_time": "9:00"}}), "invalid"),
        (json!({"pings": {"countdown_minutes": [4]}}), "invalid"),
        (
            json!({"pings": {"countdown_minutes": [5, 10, 15, 20, 25]}}),
            "invalid",
        ),
        (json!({"watching": {"paused": "yes"}}), "invalid"),
        (json!({"chatbot": {"guild_rate": {"count": 0}}}), "invalid"),
        (
            json!({"chatbot": {"member_rate": {"window_s": 5}}}),
            "invalid",
        ),
        (json!({"chatbot": {"enabled": true}}), "invalid"),
        (json!({"self_service": {"mode": "links"}}), "invalid"),
        (json!({"persona": {"active": ""}}), "invalid"),
        (json!({"persona": {"active": "ghost"}}), "invalid"),
        (
            json!({"models": {"roles": {"chat": {"alias": "kanata/gone"}}}}),
            "invalid",
        ),
        (
            json!({"models": {"roles": {"chat": {"alias": "kanata/extract"}}}}),
            "invalid",
        ),
        (
            json!({"models": {"roles": {"extraction": {"reasoning": ""}}}}),
            "invalid",
        ),
        (
            json!({"models": {"roles": {"extraction": {"reasoning": "loud"}}}}),
            "invalid",
        ),
    ] {
        config.refused(body, 422, code).await;
    }
    let malformed = config.send("PATCH", PATH, None, &json!("x")).await;
    refused(&malformed, 422, "invalid", "string body");
    let reply = send(
        config.reads.admin,
        "PATCH",
        ADMIN_HOST,
        PATH,
        &[
            ORIGIN,
            ("Cookie", &config.reads.cookie),
            ("X-Kanade-CSRF", &config.reads.csrf),
        ],
        Some("{not json"),
    )
    .await;
    refused(&reply, 400, "invalid_body", "not JSON");
    assert_eq!(config.get().await, before);
    assert_eq!(config.desk.settings().await, settings());
}

#[tokio::test]
async fn visibility_delta_is_live_for_admin_members_and_chat_resolution() {
    let config = Config::new().await;
    let initial = config.get().await;
    assert_eq!(initial["persona"]["profiles"][0]["public"], false);
    assert!(config.desk.profile_choices().options.is_empty());
    assert_eq!(
        config
            .reads
            .read("/api/admin/personas", "members.json#/$defs/Personas")
            .await,
        json!([])
    );

    config.dir.profile("bold");
    let reload = config.send("POST", RELOAD, None, &json!({})).await;
    assert_eq!(reload.status, 200, "{}", reload.text());

    let published = config
        .patch(json!({"persona": {"active": "calm", "visibility": [
            {"key": "calm", "public": true},
            {"key": "bold", "public": true}
        ]}}))
        .await;
    assert_eq!(published["persona"]["active"], "calm");
    assert_eq!(
        published["notices"],
        json!(["Reply profile visibility updated."])
    );
    assert_eq!(published["persona"]["profiles"][0]["public"], true);
    assert_eq!(
        config
            .desk
            .profile_choices()
            .options
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        ["calm", "bold"]
    );
    let options = config
        .reads
        .read("/api/admin/personas", "members.json#/$defs/Personas")
        .await;
    assert_eq!(
        options
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["key"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["calm", "bold"]
    );

    let member = config
        .reads
        .ok(
            "PATCH",
            "/api/admin/members/1001",
            json!({"persona": "calm"}),
            "members.json#/$defs/MemberRow",
        )
        .await;
    assert_eq!(member["persona"], "calm");
    assert_eq!(member["persona_available"], true);
    let calm = ProfileId::parse("calm").unwrap();
    let resolution = |choices: &kanade::api::admin::config::LiveProfileChoices| {
        let query = ProfileQuery {
            member_roles: &[],
            role_assignments: &[],
            saved_selection: Some(&calm),
            selectable: &choices.selectable,
        };
        choices
            .snapshot
            .as_ref()
            .unwrap()
            .resolve(&query)
            .unwrap()
            .profile_source
    };
    assert_eq!(
        resolution(&config.desk.profile_choices()),
        ProfileSource::MemberSelection
    );

    let private = config
        .patch(json!({"persona": {"visibility": [{"key": "calm", "public": false}]}}))
        .await;
    let calm_profile = private["persona"]["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|profile| profile["key"] == "calm")
        .unwrap();
    assert_eq!(calm_profile["public"], false);
    assert_eq!(
        resolution(&config.desk.profile_choices()),
        ProfileSource::BundleDefault {
            saved_selection_unavailable: true
        }
    );
    let rows = config
        .reads
        .read("/api/admin/members", "members.json#/$defs/MemberRows")
        .await;
    let alice = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "1001")
        .unwrap();
    assert_eq!(alice["persona"], "calm", "private selection is retained");
    assert_eq!(alice["persona_available"], false);
    assert_eq!(
        config
            .reads
            .refused(
                "PATCH",
                "/api/admin/members/1001",
                json!({"persona": "calm"}),
            )
            .await,
        (422, "invalid".into())
    );

    config
        .patch(json!({"persona": {"visibility": [{"key": "calm", "public": true}]}}))
        .await;
    fs::remove_file(config.dir.0.join("profiles/calm.yaml")).unwrap();
    assert_eq!(
        config.send("POST", RELOAD, None, &json!({})).await.status,
        200
    );
    assert!(
        !config
            .desk
            .profile_choices()
            .options
            .iter()
            .any(|item| item.key == "calm")
    );
    config.dir.profile("calm");
    assert_eq!(
        config.send("POST", RELOAD, None, &json!({})).await.status,
        200
    );
    assert!(
        config
            .desk
            .profile_choices()
            .options
            .iter()
            .any(|item| item.key == "calm")
    );
}

#[tokio::test]
async fn visibility_delta_rejects_unknown_ids_bad_flags_and_duplicate_keys() {
    let config = Config::new().await;
    for (body, code) in [
        (
            json!({"persona": {"visibility": [{"key": "../private", "public": true}]}}),
            "invalid",
        ),
        (
            json!({"persona": {"visibility": [{"key": "missing", "public": true}]}}),
            "invalid",
        ),
        (
            json!({"persona": {"visibility": [{"key": "calm", "public": "yes"}]}}),
            "invalid",
        ),
        (
            json!({"persona": {"visibility": [
                {"key": "calm", "public": true}, {"key": "calm", "public": false}
            ]}}),
            "invalid",
        ),
        (
            json!({"persona": {"visibility": [{"key": "calm", "public": true, "extra": 1}]}}),
            "unknown_field",
        ),
        (
            json!({"persona": {"visibility": {"key": "calm", "public": true}}}),
            "invalid",
        ),
    ] {
        config.refused(body, 422, code).await;
    }
    assert!(
        config
            .desk
            .settings()
            .await
            .persona
            .profile_visibility
            .is_empty()
    );
}

#[tokio::test]
async fn stale_admin_visibility_deltas_do_not_republish_another_admins_private_profile() {
    let config = Config::with_two_admins().await;
    config.dir.profile("bold");
    assert_eq!(
        config.send("POST", RELOAD, None, &json!({})).await.status,
        200
    );
    let (cookie_a, csrf_a) = config.reads.tailscale_session_as("ops@example.com").await;
    let (cookie_b, csrf_b) = config
        .reads
        .tailscale_session_as("second-ops@example.com")
        .await;

    let publish_b = json!({"persona": {"visibility": [{"key": "bold", "public": true}]}});
    let a = config
        .send_as(
            (&cookie_a, &csrf_a, Some("ops@example.com")),
            "PATCH",
            PATH,
            None,
            &publish_b,
        )
        .await;
    view(&a, "admin A publishes bold");
    let stale = config.get_as(&cookie_b, "second-ops@example.com").await;
    assert_eq!(stale["persona"]["profiles"][0]["key"], "bold");
    assert_eq!(stale["persona"]["profiles"][0]["public"], true);

    let make_b_private = json!({"persona": {"visibility": [{"key": "bold", "public": false}]}});
    let a = config
        .send_as(
            (&cookie_a, &csrf_a, Some("ops@example.com")),
            "PATCH",
            PATH,
            None,
            &make_b_private,
        )
        .await;
    view(&a, "admin A makes bold private");
    let publish_c = json!({"persona": {"visibility": [{"key": "calm", "public": true}]}});
    let b = config
        .send_as(
            (&cookie_b, &csrf_b, Some("second-ops@example.com")),
            "PATCH",
            PATH,
            None,
            &publish_c,
        )
        .await;
    let final_view = view(&b, "admin B publishes calm from stale view");
    let profiles = final_view["persona"]["profiles"].as_array().unwrap();
    let is_public = |key: &str| {
        profiles
            .iter()
            .find(|profile| profile["key"] == key)
            .unwrap()["public"]
            .as_bool()
            .unwrap()
    };
    assert!(!is_public("bold"));
    assert!(is_public("calm"));
    assert_eq!(
        config.desk.settings().await.persona.profile_visibility,
        ["calm"]
    );
}

#[tokio::test]
async fn concurrent_same_key_visibility_patches_have_one_winner() {
    let config = Config::new().await;
    let publish = json!({"persona": {"visibility": [{"key": "calm", "public": true}]}});
    let private = json!({"persona": {"visibility": [{"key": "calm", "public": false}]}});
    let (left, right) = tokio::join!(
        config.send("PATCH", PATH, Some("visibility-race"), &publish),
        config.send("PATCH", PATH, Some("visibility-race"), &private),
    );
    let (winner, loser) = if left.status == 200 {
        (&left, &right)
    } else {
        (&right, &left)
    };
    assert_eq!(winner.status, 200, "{}", winner.text());
    refused(loser, 422, "idempotency_mismatch", "concurrent key reuse");
    let winning_public =
        view(winner, "winning visibility request")["persona"]["profiles"][0]["public"]
            .as_bool()
            .unwrap();
    assert_eq!(
        config
            .desk
            .settings()
            .await
            .persona
            .profile_visibility
            .contains(&"calm".to_owned()),
        winning_public
    );
}

#[tokio::test]
async fn reasoning_follows_published_efforts_and_strands_reset_with_notices() {
    let config = Config::new().await;
    // `null` efforts: Kanata restricts nothing, so every level is accepted.
    let legacy = config
        .patch(json!({"models": {"roles": {"rewrite": {"alias": "kanata/legacy", "reasoning": "minimal"}}}}))
        .await;
    assert_eq!(legacy["models"]["roles"]["rewrite"]["reasoning"], "minimal");
    assert_eq!(legacy["notices"], json!([]));
    // An explicit level the alias does not publish is refused, naming what it takes.
    let message = config
        .refused(
            json!({"models": {"roles": {"chat": {"reasoning": "high"}}}}),
            422,
            "invalid",
        )
        .await;
    assert_eq!(
        message,
        "kanata/chat accepts reasoning low, medium, not high."
    );
    // The app sends all three roles: extraction to high while chat still
    // inherits is judged on the final extraction level.
    let message = config
        .refused(
            json!({"models": {"roles": {
                "extraction": {"alias": "kanata/extract", "reasoning": "high"},
                "chat": {"alias": "kanata/chat", "reasoning": ""},
                "rewrite": {"alias": "kanata/legacy", "reasoning": "minimal"},
            }}}),
            422,
            "invalid",
        )
        .await;
    assert!(
        message.starts_with("chat inherits high from extraction"),
        "{message}"
    );
    // Not part of the request: the stranded inheritor is reset, with a notice.
    let stranded = config
        .patch(json!({"models": {"roles": {"extraction": {"reasoning": "high"}}}}))
        .await;
    assert_eq!(
        stranded["models"]["roles"]["extraction"]["reasoning"],
        "high"
    );
    // kanata/chat requires reasoning, so the reset lands on its lowest level.
    assert_eq!(stranded["models"]["roles"]["chat"]["reasoning"], "low");
    assert_eq!(
        stranded["notices"],
        json!(["chat reasoning reset to low: kanata/chat does not publish high."])
    );
    // An alias change strands its own untouched level too.
    let moved = config
        .patch(json!({"models": {"roles": {"rewrite": {"alias": "kanata/rewrite-small"}}}}))
        .await;
    assert_eq!(moved["models"]["roles"]["rewrite"]["reasoning"], "off");
    assert_eq!(
        moved["notices"],
        json!(["rewrite reasoning reset to off: kanata/rewrite-small does not publish minimal."])
    );
    // Inherit is legal again once extraction's level fits the alias.
    let fits = config
        .patch(json!({"models": {"roles": {"extraction": {"reasoning": "low"}, "chat": {"reasoning": ""}}}}))
        .await;
    assert_eq!(fits["models"]["roles"]["chat"]["reasoning"], "");
    assert_eq!(fits["notices"], json!([]));
    // A GET carries no notices.
    assert_eq!(config.get().await["notices"], json!([]));

    // A saved alias Kanata no longer lists keeps its level and never blocks
    // saving another role.
    config.catalog.remove("kanata/chat");
    let other = config
        .patch(json!({"models": {"roles": {"extraction": {"reasoning": "medium"}}}}))
        .await;
    assert_eq!(
        other["models"]["roles"]["chat"],
        json!({"alias": "kanata/chat", "reasoning": ""})
    );
    assert_eq!(other["notices"], json!([]));
}

#[tokio::test]
async fn capacity_refuses_a_change_that_would_stop_the_bot() {
    let config = Config::new().await;
    // 2 permits but kanata/tiny admits 1.
    let message = config
        .refused(
            json!({"models": {"roles": {"rewrite": {"alias": "kanata/tiny"}}}}),
            422,
            "capacity",
        )
        .await;
    assert!(
        message.contains("admits at most 1 (capped by kanata/tiny)"),
        "{message}"
    );
    assert_eq!(config.desk.settings().await, settings());
    // Room to spare only warns.
    let roomy = config
        .patch(json!({"models": {"roles": {"extraction": {"alias": "kanata/legacy"}, "rewrite": {"alias": "kanata/legacy"}}}}))
        .await;
    assert!(
        roomy["models"]["capacity_check"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["level"] == "warning"
                && check["message"] == "Group gateway uses 2 of the 4 permits Kanata admits."),
        "{}",
        roomy["models"]["capacity_check"]
    );
    // An error already present (a vanished alias) never blocks another save.
    config.catalog.remove("kanata/chat");
    let view = config
        .patch(json!({"models": {"roles": {"extraction": {"reasoning": "low"}}}}))
        .await;
    assert!(
        view["models"]["capacity_check"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["level"] == "error"
                && check["message"] == "Kanata does not list kanata/chat.")
    );
}

#[tokio::test]
async fn model_saves_need_a_reachable_gateway_other_sections_do_not() {
    let config = Config::new().await;
    config.catalog.set_reachable(false);
    let view = config.get().await;
    assert_eq!(view["models"]["reachable"], false);
    assert_eq!(view["models"]["capacity_check"][0]["level"], "warning");
    config
        .refused(
            json!({"models": {"roles": {"chat": {"reasoning": "off"}}}}),
            503,
            "models_unreachable",
        )
        .await;
    config
        .patch(json!({"notifications": {"quiet_mode": true}}))
        .await;

    let bare = Config::build(false).await;
    let view = bare.get().await;
    assert_eq!(view["models"]["reachable"], false);
    assert_eq!(view["models"]["catalog"], json!([]));
    assert!(
        view["chatbot"]["missing_env"]
            .as_array()
            .unwrap()
            .contains(&json!("KANADE_MODEL_BASE_URL"))
    );
    bare.refused(
        json!({"models": {"roles": {"chat": {"reasoning": "off"}}}}),
        503,
        "models_unreachable",
    )
    .await;
}

#[tokio::test]
async fn a_keyed_patch_replays_and_a_reused_key_is_refused() {
    let config = Config::new().await;
    let mut changes = config.desk.subscribe();
    let body = json!({"models": {"roles": {"extraction": {"reasoning": "high"}}}});
    let first = view(
        &config.send("PATCH", PATH, Some("cfg-1"), &body).await,
        "first",
    );
    assert_eq!(first["notices"].as_array().unwrap().len(), 1);
    // The client's retry after a lost answer: same result, applied once.
    let retry = view(
        &config.send("PATCH", PATH, Some("cfg-1"), &body).await,
        "retry",
    );
    assert_eq!(retry, first);
    assert_eq!(changes.borrow_and_update().revision, 1);
    let other = config
        .send(
            "PATCH",
            PATH,
            Some("cfg-1"),
            &json!({"watching": {"paused": true}}),
        )
        .await;
    refused(&other, 422, "idempotency_mismatch", "reused key");
    assert!(!config.desk.settings().await.watching.paused);
    let bad = config
        .send(
            "PATCH",
            PATH,
            Some("bad key!"),
            &json!({"watching": {"paused": true}}),
        )
        .await;
    refused(&bad, 400, "invalid_idempotency_key", "bad key");
    // Unkeyed, a repeat is naturally a no-op.
    let unkeyed = config.patch(body.clone()).await;
    assert_eq!(unkeyed["notices"], json!([]));
    assert_eq!(roles(&unkeyed), roles(&first));
    assert!(!changes.has_changed().unwrap());
}

#[tokio::test]
async fn saved_changes_are_published_to_subscribers() {
    let config = Config::new().await;
    let mut changes = config.desk.subscribe();
    assert_eq!(changes.borrow().revision, 0);
    config.patch(json!({"watching": {"paused": true}})).await;
    changes.changed().await.unwrap();
    {
        let change = changes.borrow_and_update();
        assert_eq!(change.revision, 1);
        assert_eq!(change.section, Some("watching"));
        assert_eq!(change.actor.as_deref(), Some("admin:token"));
        assert!(change.settings.watching.paused);
    }
    // Saving the same value again changes nothing and publishes nothing.
    config.patch(json!({"watching": {"paused": true}})).await;
    assert!(!changes.has_changed().unwrap());
}

#[tokio::test]
async fn profiles_reload_and_persona_switch_swap_the_live_snapshot() {
    let config = Config::new().await;
    let effective = |store: &PersonaStore| {
        store
            .pin()
            .provenance()
            .effective
            .as_ref()
            .map(PersonaId::to_string)
    };
    assert_eq!(effective(&config.personas).as_deref(), Some("kanade"));

    config.dir.profile("terse");
    for _ in 0..2 {
        let reply = config.send("POST", RELOAD, None, &json!({})).await;
        assert_eq!(reply.status, 200, "{}", reply.text());
        let body = reply.json();
        assert_valid("common.json#/$defs/ReloadResult", "reload", &body);
        assert_eq!(body["reloaded"], 2);
        assert_eq!(
            body["message"],
            "Reloaded 2 reply profiles from config/personas/profiles/. 1 unreadable file(s) were skipped."
        );
    }
    let keys: Vec<String> = config.get().await["persona"]["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|profile| profile["key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(keys, ["calm", "terse"]);

    // Nothing saved yet: the persona in use is reported, never "".
    let unset = config.get().await;
    let active = unset["persona"]["active"].as_str().unwrap().to_owned();
    assert!(!active.is_empty());
    assert!(
        unset["persona"]["personas"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["key"] == active.as_str())
    );

    let switched = config.patch(json!({"persona": {"active": "calm"}})).await;
    assert_eq!(switched["persona"]["active"], "calm");
    assert_eq!(effective(&config.personas).as_deref(), Some("calm"));
    let restarted = load_settings(&*config.reads.store, &RuntimeSettings::default())
        .await
        .unwrap();
    assert_eq!(restarted.persona.active, "calm");

    // A persona whose bundle is broken is refused; nothing changes.
    fs::write(config.dir.0.join("bundles/kanade.yaml"), "id: [\n").unwrap();
    config
        .refused(json!({"persona": {"active": "kanade"}}), 422, "invalid")
        .await;
    assert_eq!(effective(&config.personas).as_deref(), Some("calm"));
    assert_eq!(config.desk.settings().await.persona.active, "calm");
}

#[tokio::test]
async fn config_routes_need_a_session_and_writes_need_csrf() {
    let config = Config::new().await;
    let admin = config.reads.admin;
    for (method, path) in [("GET", PATH), ("PATCH", PATH), ("POST", RELOAD)] {
        let reply = send(admin, method, ADMIN_HOST, path, &[ORIGIN], Some("{}")).await;
        refused(&reply, 401, "unauthenticated", path);
    }
    let body = r#"{"persona":{"visibility":[{"key":"calm","public":true}]}}"#;
    for (method, path) in [("PATCH", PATH), ("POST", RELOAD)] {
        let reply = send(
            admin,
            method,
            ADMIN_HOST,
            path,
            &[ORIGIN, ("Cookie", &config.reads.cookie)],
            Some(body),
        )
        .await;
        refused(&reply, 403, "csrf", path);
    }
    assert!(
        config
            .desk
            .settings()
            .await
            .persona
            .profile_visibility
            .is_empty()
    );
    // Still unmounted: it needs the Discord wiring.
    let reply = config
        .send("POST", "/api/admin/digest", None, &json!({}))
        .await;
    assert_eq!(reply.status, 404);
}

const ACCESS: &str = "/api/admin/access";
const RECHECK: &str = "/api/admin/access/recheck";

#[tokio::test]
async fn access_reports_watched_and_digest_channels() {
    let mut digest = settings();
    // Not watched: listed as the digest channel only.
    digest.posting.channel_id = Some("star".into());
    let config = Config::with_settings(true, Vec::new(), digest).await;
    let admin = config.reads.admin;
    for (method, path) in [("GET", ACCESS), ("POST", RECHECK)] {
        let reply = send(admin, method, ADMIN_HOST, path, &[ORIGIN], Some("{}")).await;
        refused(&reply, 401, "unauthenticated", path);
    }
    let reply = send(
        admin,
        "POST",
        ADMIN_HOST,
        RECHECK,
        &[ORIGIN, ("Cookie", &config.reads.cookie)],
        Some("{}"),
    )
    .await;
    refused(&reply, 403, "csrf", RECHECK);

    let get = request(
        admin,
        "GET",
        ADMIN_HOST,
        ACCESS,
        &[("Cookie", &config.reads.cookie)],
    )
    .await;
    let recheck = config.send("POST", RECHECK, None, &json!({})).await;
    for reply in [get, recheck] {
        assert_eq!(reply.status, 200, "{}", reply.text());
        let report = reply.json();
        assert_valid("config.json#/$defs/AccessReport", "access", &report);
        assert_eq!(report["connected"], true);
        assert!(
            report["checked_at"].as_str().unwrap().len() == 16,
            "{report}"
        );
        let rows = report["rows"].as_array().unwrap();
        let flags = |row: &Value| {
            [
                "view",
                "send",
                "history",
                "embed",
                "react",
                "manage_messages",
            ]
            .map(|key| row[key].as_bool().unwrap())
        };
        let summary: Vec<_> = rows
            .iter()
            .map(|row| {
                (
                    row["id"].as_str().unwrap(),
                    row["watched"].as_bool().unwrap(),
                    row["digest"].as_bool().unwrap(),
                    flags(row),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                // Unknown permissions count as allowed (v4).
                ("kalos-four", true, false, [true; 6]),
                ("star", false, true, [true, true, true, true, true, true]),
                (
                    "limbo-trio",
                    true,
                    false,
                    [true, false, true, true, true, false]
                ),
            ]
        );
    }
}

#[tokio::test]
async fn off_is_refused_where_the_alias_requires_reasoning() {
    let config = Config::new().await;
    // kanata/chat publishes low and medium but not none.
    let view = config.get().await;
    let off_allowed = |id: &str| {
        view["models"]["catalog"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["id"] == id)
            .unwrap()["off_allowed"]
            .clone()
    };
    assert_eq!(off_allowed("kanata/chat"), json!(false));
    assert_eq!(off_allowed("kanata/legacy"), json!(true), "null list");
    assert_eq!(
        off_allowed("kanata/rewrite-small"),
        json!(true),
        "no reasoning"
    );

    let message = config
        .refused(
            json!({"models": {"roles": {"chat": {"reasoning": "off"}}}}),
            422,
            "invalid",
        )
        .await;
    assert_eq!(
        message,
        "kanata/chat requires reasoning: pick low or medium."
    );
    let message = config
        .refused(
            json!({"models": {"roles": {
                "extraction": {"alias": "kanata/legacy", "reasoning": "off"},
                "chat": {"reasoning": ""},
            }}}),
            422,
            "invalid",
        )
        .await;
    assert_eq!(
        message,
        "chat inherits off from extraction, but kanata/chat requires reasoning: pick low or medium."
    );
    // Not part of the request: an inheritor stranded on off gets the lowest level.
    let stranded = config
        .patch(json!({"models": {"roles": {"extraction": {"alias": "kanata/legacy", "reasoning": "off"}}}}))
        .await;
    assert_eq!(stranded["models"]["roles"]["chat"]["reasoning"], "low");
    assert_eq!(
        stranded["notices"],
        json!(["chat reasoning set to low: kanata/chat requires reasoning."])
    );
}

#[tokio::test]
async fn reasoning_variants_are_marked_and_their_fixed_level_wins() {
    let config = Config::new().await;
    let view = config.get().await;
    let info = |id: &str| {
        view["models"]["catalog"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["id"] == id)
            .unwrap()
            .clone()
    };
    let variant = info("kanata/chat:medium");
    assert_eq!(variant["variant_of"], "kanata/chat");
    assert_eq!(variant["fixed_effort"], "medium");
    assert!(
        info("kanata/solo:low").get("variant_of").is_none(),
        "base unlisted"
    );
    assert!(info("kanata/chat").get("fixed_effort").is_none());
    assert!(roles(&view)["chat"].get("variant_of").is_none());

    // Saving a variant is accepted; an explicit differing level is ignored.
    let saved = config
        .patch(json!({"models": {"roles": {"chat": {"alias": "kanata/chat:medium", "reasoning": "low"}}}}))
        .await;
    assert_eq!(
        saved["models"]["roles"]["chat"],
        json!({
            "alias": "kanata/chat:medium",
            "reasoning": "medium",
            "variant_of": "kanata/chat",
            "fixed_effort": "medium",
        })
    );
    assert_eq!(
        saved["notices"],
        json!([
            "chat reasoning is fixed at medium by kanata/chat:medium; the requested low is ignored."
        ])
    );
    // The same level, or another role's save, raises nothing.
    let again = config
        .patch(json!({"models": {"roles": {"chat": {"reasoning": "medium"}}}}))
        .await;
    assert_eq!(again["notices"], json!([]));
    let other = config
        .patch(json!({"models": {"roles": {"extraction": {"reasoning": "low"}}}}))
        .await;
    assert_eq!(other["notices"], json!([]));
    assert_eq!(other["models"]["roles"]["chat"]["reasoning"], "medium");
    assert_eq!(
        config.get().await["models"]["roles"]["chat"]["fixed_effort"],
        "medium"
    );
}

fn group(name: &str, permits: u32, aliases: &[&str]) -> CapacityGroup {
    CapacityGroup {
        name: name.into(),
        permits,
        aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
    }
}

#[tokio::test]
async fn declared_groups_are_listed_checked_per_group_and_summarised() {
    // kanata/rewrite-small is left out on purpose; kanata/legacy is unused.
    let config = Config::with_groups(
        true,
        vec![
            group("local", 2, &["kanata/extract", "kanata/chat"]),
            group("spare", 3, &["kanata/legacy"]),
        ],
    )
    .await;
    let view = config.get().await;
    let models = &view["models"];
    assert_eq!(models["groups_source"], "config");
    assert_eq!(
        models["groups"],
        json!([
            {"model": "kanata/extract", "group": "local", "permits": 2},
            {"model": "kanata/chat", "group": "local", "permits": 2},
            {"model": "kanata/legacy", "group": "spare", "permits": 3},
        ])
    );
    assert_eq!(
        models["key_limits"],
        json!({"max_in_flight": null, "shared": true})
    );
    assert_eq!(
        models["capacity_check"],
        json!([
            {"level": "warning", "message": "The rewrite model kanata/rewrite-small is in no capacity group; its calls are refused."},
            {"level": "ok", "message": "Group local: 2 permits, matching Kanata's limit."},
            {"level": "warning", "message": "Group spare uses 3 of the 8 permits Kanata admits."},
        ])
    );
    let row = view["env"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "KANADE_MODEL_PERMITS / kanade.toml [[models.groups]]")
        .unwrap()
        .clone();
    assert_eq!(row["value"], "2 groups: local 2, spare 3");
    assert_eq!(
        row["reason"],
        "Set in kanade.toml ([[models.groups]]); restart to apply."
    );
    let message = config
        .refused(json!({"models": {"groups": []}}), 422, "read_only")
        .await;
    assert_eq!(
        message,
        "models.groups cannot be changed here: it is set in kanade.toml ([[models.groups]]); restart to apply."
    );
}

#[tokio::test]
async fn a_declared_group_over_kanata_limit_refuses_the_save() {
    let config = Config::with_groups(
        true,
        vec![group(
            "local",
            2,
            &[
                "kanata/extract",
                "kanata/chat",
                "kanata/rewrite-small",
                "kanata/tiny",
            ],
        )],
    )
    .await;
    let view = config.get().await;
    assert_eq!(
        view["models"]["capacity_check"],
        json!([{"level": "error", "message": "Group local declares 2 permits but Kanata admits at most 1 (capped by kanata/tiny); the bot refuses to start."}])
    );
    // A pre-existing error never blocks an unrelated save.
    config
        .patch(json!({"models": {"roles": {"extraction": {"reasoning": "low"}}}}))
        .await;
}

#[tokio::test]
async fn a_models_save_switches_the_running_roles_and_the_view_shows_them() {
    let config = Config::new().await;
    config.catalog.2.lock().unwrap().insert(
        Role::Chat,
        RunningRole {
            alias: "kanata/chat".into(),
            effort: Some(Effort::Medium),
        },
    );
    let view = config.get().await;
    assert_eq!(
        roles(&view)["chat"]["running"],
        json!({"alias": "kanata/chat", "reasoning": "medium"})
    );
    assert!(roles(&view)["rewrite"].get("running").is_none(), "unrouted");
    assert!(
        config.catalog.applied().is_empty(),
        "a read applies nothing"
    );

    let saved = config
        .patch(
            json!({"models": {"roles": {"chat": {"alias": "kanata/legacy", "reasoning": "high"}}}}),
        )
        .await;
    assert_eq!(saved["notices"], json!([]));
    let applied = config.catalog.applied();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].chat, role("kanata/legacy", Reasoning::High));
    assert_eq!(applied[0], config.desk.settings().await.models);
    // Other sections never touch the running models.
    config
        .patch(json!({"notifications": {"quiet_mode": true}}))
        .await;
    assert_eq!(config.catalog.applied().len(), 1);
}

#[tokio::test]
async fn a_role_switched_to_an_ungrouped_alias_is_refused_and_nothing_moves() {
    let config = Config::with_groups(
        true,
        vec![group(
            "local",
            8,
            &["kanata/extract", "kanata/chat", "kanata/rewrite-small"],
        )],
    )
    .await;
    let message = config
        .refused(
            json!({"models": {"roles": {"chat": {"alias": "kanata/legacy"}}}}),
            422,
            "ungrouped",
        )
        .await;
    assert_eq!(
        message,
        "The chat model kanata/legacy is in no capacity group; add it to [[models.groups]] in kanade.toml and restart, or pick a grouped model."
    );
    assert!(config.catalog.applied().is_empty());
    assert_eq!(
        config.desk.settings().await.models.chat,
        role("kanata/chat", Reasoning::Inherit)
    );
    // A grouped alias switches.
    config
        .patch(json!({"models": {"roles": {"chat": {"alias": "kanata/chat", "reasoning": "low"}}}}))
        .await;
    assert_eq!(config.catalog.applied().len(), 1);
}

#[tokio::test]
async fn a_first_extraction_or_rewrite_model_says_it_starts_after_a_restart() {
    let mut start = settings();
    start.models.extraction.alias = None;
    start.models.rewrite.alias = None;
    let config = Config::with_settings(true, Vec::new(), start).await;
    // What the stack reports once those roles have a model it did not start with.
    *config.catalog.3.lock().unwrap() = vec![Role::Extraction, Role::Rewrite];
    let saved = config
        .patch(json!({"models": {"roles": {
            "extraction": {"alias": "kanata/extract", "reasoning": "low"},
            "rewrite": {"alias": "kanata/rewrite-small", "reasoning": "off"},
        }}}))
        .await;
    assert_eq!(
        saved["notices"],
        json!([
            "The extraction model had none when the bot started: restart to start extraction with kanata/extract.",
            "The rewrite model had none when the bot started: restart to start heading rewrites with kanata/rewrite-small.",
        ])
    );
    // A later save that leaves those aliases alone says nothing more.
    let again = config
        .patch(json!({"models": {"roles": {"extraction": {"reasoning": "medium"}}}}))
        .await;
    assert_eq!(again["notices"], json!([]));
}
