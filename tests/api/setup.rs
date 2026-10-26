//! Admin Setup (`GET /api/admin/setup`) over the seeded store of `reads.rs`:
//! item order and facts, `complete`, presence-only answers, modules off and
//! on, no API state, and the admin-only mount. Answers are validated
//! against `setup.json`.

use std::{collections::BTreeMap, sync::Arc};

use kanade::{
    api::{
        listeners::Site,
        state::{ApplicationPort, SetupFacts},
    },
    app::config::{CatalogRead, ConfigDesk, ConfigFacts, ConfigFuture, ConfigInputs, ModelCatalog},
    discord::transport::{DiscordTransport, FAKE_APPLICATION_ID, FakeDiscord, Outcome},
    infrastructure::llm::{governor::Role, setup::RunningRole},
    runtime::config::Modules,
};
use kanade_domain::settings::RuntimeSettings;
use kanade_store::SqliteStore;
use serde_json::{Value, json};
use twilight_model::oauth::ApplicationFlags;

use crate::{
    reads::{Build, Reads},
    schemas::assert_valid,
    support::{ADMIN_HOST, Fixture, PUBLIC_HOST, find, request, spawn, strs},
};

const VIEW: &str = "setup.json#/$defs/SetupView";
const PATH: &str = "/api/admin/setup";
/// An unwatched text channel the fixture adds for the digest.
const DIGEST: &str = "120000000000000010";

const ORDER: [&str; 16] = [
    "discord_gateway",
    "discord_intents",
    "bot_access",
    "digest_channel",
    "watched_channels",
    "model_gateway",
    "extraction_model",
    "chat",
    "persona",
    "admin_sign_in",
    "public_portal",
    "roster",
    "character_data",
    "guides",
    "scouter",
    "backups",
];

/// A gateway that answers (or not, or without a model list) and runs the
/// extraction role (or not).
struct Catalog {
    reachable: bool,
    no_listing: bool,
    extraction: bool,
}

impl ModelCatalog for Catalog {
    fn read(&self) -> ConfigFuture<'_, CatalogRead> {
        let (reachable, no_listing) = (self.reachable, self.no_listing);
        Box::pin(async move {
            CatalogRead {
                reachable,
                no_listing,
                ..CatalogRead::default()
            }
        })
    }

    fn running(&self) -> BTreeMap<Role, RunningRole> {
        self.extraction
            .then(|| {
                (
                    Role::Extraction,
                    RunningRole {
                        alias: "kanata/extract".into(),
                        effort: None,
                    },
                )
            })
            .into_iter()
            .collect()
    }
}

/// The settings a set-up deployment saved: a digest channel, a watched
/// list, chat on, a persona and an extraction model.
fn configured() -> RuntimeSettings {
    let mut settings = RuntimeSettings::default();
    settings.posting.channel_id = Some(DIGEST.into());
    settings.watching.channel_ids = vec!["kalos-four".into(), "limbo-trio".into()];
    settings.chatbot.enabled = true;
    settings.persona.active = "kanade".into();
    settings.models.extraction.alias = Some("kanata/extract".into());
    settings
}

fn desk(
    settings: RuntimeSettings,
    store: Arc<SqliteStore>,
    catalog: Option<Catalog>,
) -> Arc<ConfigDesk> {
    let model_gateway = catalog.as_ref().map(|_| "https://kanata.test/v1".into());
    Arc::new(ConfigDesk::new(ConfigInputs {
        settings,
        store,
        models: catalog.map(|catalog| Arc::new(catalog) as Arc<dyn ModelCatalog>),
        facts: ConfigFacts {
            timezone: "Asia/Kuala_Lumpur".into(),
            model_gateway,
            model_permits: 2,
            model_groups: Vec::new(),
            chat_pilot_role_id: None,
        },
        personas: None,
    }))
}

/// The application read through a fake Discord, as serve wires it.
fn application(discord: Arc<FakeDiscord>) -> ApplicationPort {
    Arc::new(move || {
        let discord = Arc::clone(&discord);
        Box::pin(async move {
            match discord.application_info().await {
                Outcome::Delivered(info) => Some(info),
                _ => None,
            }
        })
    })
}

/// Settings, gateway and application wired in; `tune` adjusts the rest.
async fn reads(
    settings: RuntimeSettings,
    catalog: Option<Catalog>,
    discord: Option<Arc<FakeDiscord>>,
    tune: impl FnOnce(&mut Build),
) -> Reads {
    Reads::build(|build| {
        build.config = Some(Box::new(move |store| desk(settings, store, catalog)));
        build.application = discord.map(application);
        build.extra_channels = &[DIGEST];
        tune(build);
    })
    .await
}

fn item<'a>(view: &'a Value, id: &str) -> &'a Value {
    find(&view["items"], "id", id)
}

#[tokio::test]
async fn items_come_in_order_with_their_facts_and_complete_needs_every_required_one() {
    let discord = Arc::new(FakeDiscord::new());
    let reads = reads(
        configured(),
        Some(Catalog {
            reachable: true,
            no_listing: false,
            extraction: true,
        }),
        Some(discord.clone()),
        |_| {},
    )
    .await;
    let view = reads.read(PATH, VIEW).await;

    assert_eq!(strs(&view["items"], "id"), ORDER);
    for (index, row) in view["items"].as_array().unwrap().iter().enumerate() {
        assert_eq!(row["required"], index < 7, "{}", row["id"]);
    }
    let state = |id: &str| item(&view, id)["state"].clone();
    // No health probe here: the loaded guild stands for a ready gateway.
    assert_eq!(state("discord_gateway"), "ok");
    assert_eq!(
        item(&view, "discord_intents"),
        &json!({"id": "discord_intents", "required": true, "state": "ok", "missing_intents": []})
    );
    // `limbo-trio` may neither post nor tidy; `kalos-four` is unknown (allowed).
    let access = item(&view, "bot_access");
    assert_eq!(access["state"], "missing");
    assert_eq!(access["channels_lacking"], 1);
    assert_eq!(
        access["invite_url"],
        format!(
            "https://discord.com/oauth2/authorize?client_id={FAKE_APPLICATION_ID}&scope=bot+applications.commands&permissions=93248"
        )
    );
    assert_eq!(
        item(&view, "digest_channel"),
        &json!({"id": "digest_channel", "required": true, "state": "ok", "source": "environment", "channel_id": DIGEST})
    );
    let watched = item(&view, "watched_channels");
    assert_eq!(
        (&watched["state"], &watched["count"]),
        (&json!("ok"), &json!(2))
    );
    assert_eq!(state("model_gateway"), "ok");
    assert_eq!(state("extraction_model"), "ok");
    assert_eq!(
        view["complete"], false,
        "bot_access is the one required item not ok"
    );

    // Optional: chat lacks its pilot role (kanade.toml); nothing else is set
    // up. The chat categories are Config's own, never a deployment gap.
    let chat = item(&view, "chat");
    assert_eq!(
        (&chat["state"], &chat["reason"]),
        (&json!("warn"), &json!("env_missing"))
    );
    assert_eq!(chat["missing_env"], json!(["discord.chat_pilot_role_id"]));
    assert_eq!(state("persona"), "ok");
    assert_eq!(state("admin_sign_in"), "missing", "break-glass alone");
    for off in [
        "public_portal",
        "roster",
        "character_data",
        "guides",
        "scouter",
    ] {
        assert_eq!(state(off), "off", "{off}");
    }
    let backups = item(&view, "backups");
    assert_eq!(backups["state"], "ok");
    assert_eq!(backups["recipients"], false);
    assert_eq!(backups["latest_backup_at"], Value::Null, "none listed");

    // One application read per answer.
    assert_eq!(
        discord.count(kanade::discord::transport::Op::ApplicationInfo),
        1
    );
}

#[tokio::test]
async fn missing_intents_unreachable_models_and_saved_but_idle_roles_are_named() {
    let discord = Arc::new(FakeDiscord::new());
    discord.set_application_flags(ApplicationFlags::GATEWAY_GUILD_MEMBERS);
    let reads = reads(
        configured(),
        Some(Catalog {
            reachable: false,
            no_listing: false,
            extraction: false,
        }),
        Some(discord),
        |build| build.role_directory_connected = false,
    )
    .await;
    let view = reads.read(PATH, VIEW).await;
    let intents = item(&view, "discord_intents");
    assert_eq!(intents["state"], "missing");
    assert_eq!(intents["missing_intents"], json!(["message_content"]));
    assert_eq!(
        (
            &item(&view, "discord_gateway")["state"],
            &item(&view, "discord_gateway")["reason"]
        ),
        (&json!("missing"), &json!("gateway_off"))
    );
    let access = item(&view, "bot_access");
    assert_eq!(
        (&access["state"], &access["reason"]),
        (&json!("missing"), &json!("not_connected"))
    );
    assert_eq!(access.get("channels_lacking"), None);
    let models = item(&view, "model_gateway");
    assert_eq!(
        (&models["state"], &models["reason"]),
        (&json!("warn"), &json!("unreachable"))
    );
    let extraction = item(&view, "extraction_model");
    assert_eq!(
        (&extraction["state"], &extraction["reason"]),
        (&json!("warn"), &json!("not_running"))
    );
    assert_eq!(view["complete"], false);
}

/// A gateway without a model list is set up (declared models are used), with
/// a note saying so; it never blocks `complete`.
#[tokio::test]
async fn a_gateway_without_a_model_list_is_ok_with_a_note() {
    let reads = reads(
        configured(),
        Some(Catalog {
            reachable: true,
            no_listing: true,
            extraction: true,
        }),
        Some(Arc::new(FakeDiscord::new())),
        |_| {},
    )
    .await;
    let view = reads.read(PATH, VIEW).await;
    assert_eq!(
        item(&view, "model_gateway"),
        &json!({"id": "model_gateway", "required": true, "state": "ok", "reason": "no_listing"})
    );
}

#[tokio::test]
async fn a_refused_application_read_warns_and_drops_the_invite() {
    let discord = Arc::new(FakeDiscord::new());
    discord.set_default(
        kanade::discord::transport::Op::ApplicationInfo,
        Some(kanade::discord::transport::Step::Reject(
            kanade::discord::transport::RejectionKind::Unauthorized,
        )),
    );
    let reads = reads(configured(), None, Some(discord), |_| {}).await;
    let view = reads.read(PATH, VIEW).await;
    assert_eq!(
        item(&view, "discord_intents"),
        &json!({"id": "discord_intents", "required": true, "state": "warn", "reason": "unavailable"})
    );
    assert_eq!(item(&view, "bot_access").get("invite_url"), None);
    assert_eq!(
        item(&view, "model_gateway")["state"],
        "missing",
        "no gateway configured"
    );
    assert_eq!(item(&view, "extraction_model")["state"], "warn");
}

#[tokio::test]
async fn nothing_set_up_is_missing_or_off_and_never_a_failure() {
    // No Config desk, no Discord side, no backup directory.
    let reads = Reads::build(|build| build.backup_dir = false).await;
    let view = reads.read(PATH, VIEW).await;
    assert_eq!(strs(&view["items"], "id"), ORDER);
    assert_eq!(view["complete"], false);
    let states: Vec<_> = view["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["id"].as_str().unwrap(), row["state"].as_str().unwrap()))
        .collect();
    assert_eq!(
        states,
        [
            ("discord_gateway", "ok"),
            ("discord_intents", "warn"),
            ("bot_access", "missing"),
            ("digest_channel", "missing"),
            ("watched_channels", "missing"),
            ("model_gateway", "missing"),
            ("extraction_model", "missing"),
            ("chat", "off"),
            ("persona", "missing"),
            ("admin_sign_in", "missing"),
            ("public_portal", "off"),
            ("roster", "off"),
            ("character_data", "off"),
            ("guides", "off"),
            ("scouter", "off"),
            ("backups", "missing"),
        ]
    );

    // Without any API state (offline) the page still answers.
    let mut site = reads.site.clone();
    site.state = None;
    let offline = spawn(site).await;
    let reply = request(
        offline,
        "GET",
        ADMIN_HOST,
        PATH,
        &[("Cookie", &reads.cookie)],
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let view = reply.json();
    assert_valid(VIEW, "offline", &view);
    assert_eq!(item(&view, "discord_gateway")["reason"], "gateway_off");
    assert!(
        view["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["state"] == "missing"
                || row["state"] == "off"
                || row["state"] == "warn")
    );
}

#[tokio::test]
async fn modules_portal_and_sign_in_report_the_deployment() {
    let setup = SetupFacts {
        public_listener: true,
        member_sign_in: false,
        modules: Modules {
            roster: true,
            character_data: true,
            guides: true,
        },
        backup_recipients: true,
    };
    let reads = reads(configured(), None, None, |build| {
        build.setup = setup;
        build.logins = true;
    })
    .await;
    let view = reads.read(PATH, VIEW).await;
    assert_eq!(
        item(&view, "admin_sign_in")["state"],
        "ok",
        "Discord and Tailscale"
    );
    assert_eq!(
        item(&view, "public_portal"),
        &json!({"id": "public_portal", "required": false, "state": "warn", "source": "environment", "reason": "member_sign_in_missing", "open": false})
    );
    assert_eq!(item(&view, "roster")["state"], "ok");
    assert_eq!(item(&view, "guides")["state"], "ok");
    // On in the config, but no module service to read here.
    let data = item(&view, "character_data");
    assert_eq!(
        (&data["state"], &data["reason"]),
        (&json!("warn"), &json!("unavailable"))
    );
    assert_eq!(item(&view, "scouter")["state"], "off", "no scouter URL");
    assert_eq!(item(&view, "backups")["recipients"], true);

    let reads = reads_with_portal().await;
    let view = reads.read(PATH, VIEW).await;
    assert_eq!(item(&view, "public_portal")["state"], "ok");
}

async fn reads_with_portal() -> Reads {
    reads(configured(), None, None, |build| {
        build.setup = SetupFacts {
            public_listener: true,
            member_sign_in: true,
            ..SetupFacts::default()
        };
    })
    .await
}

#[tokio::test]
async fn the_answer_carries_presence_only_and_needs_a_session() {
    let reads = reads(
        configured(),
        Some(Catalog {
            reachable: true,
            no_listing: false,
            extraction: true,
        }),
        Some(Arc::new(FakeDiscord::new())),
        |build| build.logins = true,
    )
    .await;
    let reply = request(
        reads.admin,
        "GET",
        ADMIN_HOST,
        PATH,
        &[("Cookie", &reads.cookie)],
    )
    .await;
    assert_eq!(reply.status, 200);
    let text = reply.text();
    let backup_dir = reads.backup_dir.as_ref().unwrap().display().to_string();
    for secret in [
        "break-glass-token-with-at-least-32-bytes!",
        "edge-secret-shared-with-the-caddy-edge!!",
        "discord-client-secret",
        "kanata.test",
        "ops@example.com",
        backup_dir.as_str(),
    ] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    assert_eq!(
        reads.status(PATH, false).await,
        (401, "unauthenticated".into())
    );
}

#[tokio::test]
async fn the_public_origin_never_mounts_setup() {
    let reads = reads_with_portal().await;
    let fixture = Fixture::new();
    let mut site = Site::public(&fixture.http()).unwrap();
    site.state = reads.site.state.clone();
    let public = spawn(site).await;
    let reply = request(
        public,
        "GET",
        PUBLIC_HOST,
        PATH,
        &[("Cookie", &reads.cookie)],
    )
    .await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.api_error(), "not_found");
}
