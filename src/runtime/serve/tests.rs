use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

use chrono::{NaiveTime, Weekday};

use super::*;
use crate::domain::settings::{IdList, Section, SettingsStore, keys, save_section};

pub(super) struct Temp(pub(super) PathBuf);

impl Temp {
    pub(super) fn new() -> Self {
        let base = fs::canonicalize(std::env::temp_dir()).unwrap();
        let root = base.join(format!("kanade-serve-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        fs::write(root.join("token"), "bot-token-value\n").unwrap();
        let bundles = root.join("Personas/bundles");
        fs::create_dir_all(&bundles).unwrap();
        fs::copy(
            repo("config/personas/bundles/kanade.yaml"),
            bundles.join("kanade.yaml"),
        )
        .unwrap();
        Self(root)
    }

    pub(super) fn config(&self, extra: &[(&str, &str)]) -> ServeConfig {
        let path = |relative: &str| self.0.join(relative).display().to_string();
        let mut values: BTreeMap<String, String> = [
            ("KANADE_TIMEZONE", "Asia/Kuala_Lumpur".to_owned()),
            ("KANADE_ADMIN_BIND", "127.0.0.1:0".to_owned()),
            ("KANADE_DISCORD_TOKEN_FILE", path("token")),
            ("KANADE_GUILD_ID", "900".to_owned()),
            ("KANADE_BOSSING_ROLE_ID", "10".to_owned()),
            ("KANADE_DB_PATH", path("db/kanade.sqlite3")),
            ("KANADE_OWNER_LOCK_DIR", path("locks")),
            (
                "KANADE_CATALOG_FILE",
                repo("boss/bosses.yaml").display().to_string(),
            ),
            ("KANADE_PERSONA_DIR", path("Personas")),
            ("KANADE_DISCORD_GATEWAY", "0".to_owned()),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
        for (key, value) in extra {
            values.insert((*key).to_owned(), (*value).to_owned());
        }
        ServeConfig::from_mapping(&values).unwrap()
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repo(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

async fn with_rows(config: &ServeConfig, rows: &[(&str, &str)]) {
    let store = store::open(&config.store).await.unwrap();
    store
        .put_settings_rows(
            rows.iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        )
        .await
        .unwrap();
    store::close(store, Duration::ZERO).await;
}

#[tokio::test]
async fn seeds_apply_under_stored_rows_and_category_lists_round_trip() {
    let temp = Temp::new();
    let config = temp.config(&[
        ("KANADE_POST_CHANNEL_ID", "11"),
        ("KANADE_WATCH_CHANNEL_IDS", "12"),
        ("KANADE_WATCH_CATEGORY_IDS", "21,22"),
        ("KANADE_CHAT_CATEGORY_IDS", "31"),
        ("KANADE_EXTRACTION_ENABLED", "0"),
        ("KANADE_CHAT_ENABLED", "1"),
        ("KANADE_BOSS_WEEK_RESET_WEEKDAY", "mon"),
        ("KANADE_BOSS_WEEK_RESET_TIME", "02:30"),
        ("KANADE_DAY_OF_PING_TIME", "09:15"),
        ("KANADE_COUNTDOWN_MINUTES", "15,60"),
    ]);
    // A stored row beats its seed; the other seeds still apply.
    with_rows(&config, &[(keys::RESET_WEEKDAY, "fri")]).await;

    let store = store::open(&config.store).await.unwrap();
    let loaded = settings::load(&store, &config.seeds).await.unwrap();
    assert_eq!(loaded.posting.channel_id.as_deref(), Some("11"));
    assert_eq!(loaded.watching.channel_ids, ["12"]);
    assert_eq!(loaded.watching.category_ids, ["21", "22"]);
    assert!(!loaded.watching.extract_enabled);
    assert!(loaded.chatbot.enabled);
    assert_eq!(loaded.chatbot.category_ids, ["31"]);
    assert_eq!(loaded.schedule.reset_weekday, Weekday::Fri);
    assert_eq!(
        loaded.schedule.reset_time,
        NaiveTime::from_hms_opt(2, 30, 0).unwrap()
    );
    assert_eq!(
        loaded.pings.day_of_ping_time,
        NaiveTime::from_hms_opt(9, 15, 0).unwrap()
    );
    assert_eq!(loaded.pings.countdown_minutes, [60, 15]);

    let mut edited = loaded.clone();
    edited.watching.category_ids = vec!["51".into()];
    edited.chatbot.category_ids = vec!["61".into(), "62".into()];
    for (list, ids) in [
        (IdList::WatchedCategories, &edited.watching.category_ids),
        (IdList::ChatCategories, &edited.chatbot.category_ids),
    ] {
        save_section(&*store, &Section::IdList(list, ids.clone()))
            .await
            .unwrap();
    }
    let rows = store.settings_rows().await.unwrap();
    assert_eq!(rows[keys::WATCHED_CATEGORIES], "51");
    assert_eq!(rows[keys::CHAT_CATEGORIES], "61,62");
    assert!(!rows.contains_key("v5.chat_channel_ids"));
    assert_eq!(settings::load(&store, &config.seeds).await.unwrap(), edited);
    store::close(store, Duration::ZERO).await;

    // Unset seeds keep the code defaults.
    let defaults = settings::seed(&temp.config(&[]).seeds);
    assert_eq!(
        defaults,
        crate::domain::settings::RuntimeSettings::default()
    );
}

#[tokio::test]
async fn malformed_stored_settings_fail_startup_naming_the_key() {
    for (key, value) in [
        (keys::RESET_WEEKDAY, "someday"),
        (keys::WATCHED_CATEGORIES, "12,general"),
        (keys::PERSONA, "Not A Persona"),
    ] {
        let temp = Temp::new();
        let config = temp.config(&[]);
        with_rows(&config, &[(key, value)]).await;
        let error = serve_until(config, std::future::pending())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(&format!("`{key}`")), "{error}");
        assert!(!error.contains(value), "{error}");
        // The failed start still closed the store: it opens again.
        let store = store::open(&temp.config(&[]).store).await.unwrap();
        store::close(store, Duration::ZERO).await;
    }
}

#[tokio::test]
async fn run_length_seeds_need_catalog_pairs_only_when_unsaved() {
    for (seed, field) in [
        (
            r#"{"default_minutes":30,"overrides":[{"boss":"Ghost","difficulty":"h","minutes":60}]}"#,
            "KANADE_RUN_LENGTHS.overrides[0].boss",
        ),
        (
            r#"{"default_minutes":30,"overrides":[{"boss":"BM","difficulty":"H","minutes":60}]}"#,
            "KANADE_RUN_LENGTHS.overrides[0].difficulty",
        ),
    ] {
        let temp = Temp::new();
        let config = temp.config(&[("KANADE_RUN_LENGTHS", seed)]);
        let error = serve_until(config, std::future::pending())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(field), "{error}");
    }

    let valid =
        r#"{"default_minutes":20,"overrides":[{"boss":"BM","difficulty":"h","minutes":90}]}"#;
    let temp = Temp::new();
    serve_until(
        temp.config(&[("KANADE_RUN_LENGTHS", valid)]),
        std::future::ready(()),
    )
    .await
    .unwrap();

    // A saved row wins over an invalid seed, just as Config saves do at runtime.
    let temp = Temp::new();
    let config = temp.config(&[(
        "KANADE_RUN_LENGTHS",
        r#"{"default_minutes":30,"overrides":[{"boss":"Ghost","difficulty":"h","minutes":60}]}"#,
    )]);
    with_rows(
        &config,
        &[("v5.run_lengths", r#"{"default_minutes":30,"overrides":[]}"#)],
    )
    .await;
    serve_until(config, std::future::ready(())).await.unwrap();
}

#[tokio::test]
async fn store_directories_are_created_private_and_one_owner_is_enforced() {
    let temp = Temp::new();
    let config = temp.config(&[]);
    let store = store::open(&config.store).await.unwrap();
    for dir in ["db", "locks"] {
        let mode = fs::metadata(temp.0.join(dir)).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o700,
            "{dir}"
        );
    }
    let Err(error) = store::open(&config.store).await else {
        panic!("a second owner opened the store");
    };
    assert_eq!(
        error.to_string(),
        "KANADE_DB_PATH is already owned by another kanade process"
    );
    store::close(store, Duration::ZERO).await;
}

#[tokio::test]
async fn compose_builds_the_model_stack_and_seeds_unset_role_aliases_from_env() {
    let temp = Temp::new();
    let config = temp.config(&[
        ("KANADE_MODEL_BASE_URL", "http://127.0.0.1:9/v1"),
        ("KANADE_EXTRACT_MODEL", "kanata/extract"),
        ("KANADE_CHAT_MODEL", "kanata/chat"),
    ]);
    // A stored alias beats its env seed.
    with_rows(&config, &[(keys::CHAT_MODEL, "kanata/stored")]).await;
    let store = store::open(&config.store).await.unwrap();
    let health = LiveHealth::new(store.clone());
    let composition = api::compose(
        &config,
        store.clone(),
        Arc::new(StaticChannels(Vec::new())),
        health,
    )
    .await
    .unwrap();
    let models = &composition.settings.models;
    assert_eq!(models.extraction.alias.as_deref(), Some("kanata/extract"));
    assert_eq!(models.chat.alias.as_deref(), Some("kanata/stored"));
    assert_eq!(models.rewrite.alias, None);
    let stack = composition.models.as_ref().expect("stack");
    assert_eq!(stack.roles().chat.alias.as_deref(), Some("kanata/stored"));
    let desk = composition
        .admin
        .state
        .config
        .as_ref()
        .expect("config desk");
    assert_eq!(desk.settings().await, composition.settings);
    drop(composition);
    store::close(store, Duration::ZERO).await;
}

/// Serves one `GET /v1/models` listing on loopback.
async fn model_gateway(listing: serde_json::Value) -> String {
    use axum::{Json, Router, routing::get};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new().route("/v1/models", get(move || async move { Json(listing) }));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/v1")
}

/// The model report lines only.
async fn compose_and_report(config: &ServeConfig) -> Vec<serde_json::Value> {
    let mut lines = compose_lines(config).await;
    lines.retain(|line| !line["event"].as_str().unwrap_or("").starts_with("persona_"));
    lines
}

async fn compose_lines(config: &ServeConfig) -> Vec<serde_json::Value> {
    let store = store::open(&config.store).await.unwrap();
    let health = LiveHealth::new(store.clone());
    crate::runtime::logging::capture();
    let mut composition = api::compose(
        config,
        store.clone(),
        Arc::new(StaticChannels(Vec::new())),
        health,
    )
    .await
    .unwrap();
    composition.model_tasks.report_done().await;
    let lines = crate::runtime::logging::captured();
    drop(composition);
    store::close(store, Duration::ZERO).await;
    lines
}

#[tokio::test]
async fn compose_logs_the_persona_selection() {
    let temp = Temp::new();
    let lines = compose_lines(&temp.config(&[])).await;
    let persona = lines
        .iter()
        .find(|line| line["event"] == "persona_selected" || line["event"] == "persona_unavailable")
        .expect("a persona line");
    let text = persona.to_string();
    assert!(!text.contains(temp.0.to_str().unwrap()), "no paths: {text}");
}

#[tokio::test]
async fn compose_logs_the_model_report_with_effective_efforts_and_routes() {
    let reasoning = serde_json::json!({
        "trust_zone": "local",
        "reasoning_control": true,
        "reasoning_efforts": ["low", "medium", "high"],
    });
    let url = model_gateway(serde_json::json!({"object": "list", "data": [
        {"id": "gpt-6-luna", "kanata": reasoning},
        {"id": "gpt-6-luna:high", "kanata": reasoning},
        {"id": "ext", "kanata": {"reasoning_control": false}},
    ]}))
    .await;
    let temp = Temp::new();
    let config = temp.config(&[
        ("KANADE_MODEL_BASE_URL", url.as_str()),
        ("KANADE_EXTRACT_MODEL", "gpt-6-luna:high"),
        ("KANADE_CHAT_MODEL", "gpt-6-luna"),
        ("KANADE_CHAT_REASONING", "off"),
        ("KANADE_REWRITE_MODEL", "ext"),
        ("KANADE_REWRITE_REASONING", "high"),
    ]);
    // A saved level beats its env seed.
    with_rows(&config, &[(keys::REWRITE_REASONING, "low")]).await;
    let lines = compose_and_report(&config).await;
    let text = serde_json::to_string(&lines).unwrap();
    assert!(!text.contains("127.0.0.1"), "no gateway URL: {text}");
    let events: Vec<(&str, &str)> = lines
        .iter()
        .map(|line| {
            (
                line["level"].as_str().unwrap(),
                line["event"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        events,
        [
            ("INFO", "models_listed"),
            ("INFO", "model_role"),
            ("INFO", "model_role"),
            ("INFO", "model_role"),
            ("INFO", "model_warning"),
            ("WARN", "model_warning"),
        ],
        "{text}"
    );
    assert_eq!(lines[0]["models"], 3);
    let role = |index: usize| {
        let line = &lines[index];
        (
            line["role"].as_str().unwrap().to_owned(),
            line["alias"].as_str().unwrap().to_owned(),
            line["effort"].as_str().unwrap().to_owned(),
            line["source"].as_str().unwrap().to_owned(),
            line["route"].as_str().unwrap().to_owned(),
        )
    };
    let owned = |values: [&str; 5]| values.map(str::to_owned).into();
    assert_eq!(
        role(1),
        owned(["extraction", "gpt-6-luna:high", "high", "fixed", "homelab"])
    );
    assert_eq!(
        role(2),
        owned(["chat", "gpt-6-luna", "low", "floor", "homelab"])
    );
    assert_eq!(
        role(3),
        owned(["rewrite", "ext", "low", "stored", "external_unmasked"])
    );
    assert_eq!(lines[4]["kind"], "unpublished_effort");
    assert_eq!(lines[5]["kind"], "external_unmasked");
    assert!(
        lines[5]["message"]
            .as_str()
            .unwrap()
            .starts_with("UNMASKED: rewrite model ext")
    );
}

#[tokio::test]
async fn external_models_report_raw_data_even_when_the_alias_is_unknown() {
    let url = model_gateway(serde_json::json!({"object": "list", "data": [
        {"id": "ext", "kanata": {"reasoning_control": false}},
    ]}))
    .await;
    let temp = Temp::new();
    let config = temp.config(&[
        ("KANADE_MODEL_BASE_URL", url.as_str()),
        ("KANADE_CHAT_MODEL", "ext"),
    ]);
    let lines = compose_and_report(&config).await;
    let role = lines
        .iter()
        .find(|line| line["event"] == "model_role")
        .expect("role line");
    assert_eq!(role["route"], "external_unmasked");
    assert!(!role.as_object().unwrap().contains_key("masking"));
    let kinds: Vec<&str> = lines
        .iter()
        .filter(|line| line["event"] == "model_warning")
        .map(|line| line["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["external_unmasked"]);
    let warning = lines
        .iter()
        .find(|line| line["kind"] == "external_unmasked")
        .unwrap()["message"]
        .as_str()
        .unwrap();
    for detail in [
        "member names",
        "IDs",
        "messages",
        "complete URLs",
        "Kanata ZDR",
    ] {
        assert!(warning.contains(detail), "missing {detail}: {warning}");
    }
}

#[tokio::test]
async fn compose_logs_a_degraded_listing_and_disabled_models() {
    let temp = Temp::new();
    let degraded = temp.config(&[
        ("KANADE_MODEL_BASE_URL", "http://127.0.0.1:9/v1"),
        ("KANADE_CHAT_MODEL", "kanata/chat"),
        ("KANADE_CHAT_REASONING", "medium"),
    ]);
    let lines = compose_and_report(&degraded).await;
    assert_eq!(lines[0]["event"], "models_degraded");
    assert_eq!(lines[0]["level"], "WARN");
    assert_eq!(lines[1]["event"], "model_role");
    assert_eq!(lines[1]["effort"], "medium");
    assert_eq!(lines[1]["source"], "env");
    assert_eq!(lines[1]["route"], "external_unmasked");
    assert_eq!(lines.last().unwrap()["kind"], "external_unmasked");

    let temp = Temp::new();
    let lines = compose_and_report(&temp.config(&[])).await;
    assert_eq!(
        lines,
        [serde_json::json!({"level": "INFO", "event": "models_disabled"})]
    );
}

/// The composed context settings and every line logged while composing.
async fn compose_context(
    config: &ServeConfig,
) -> (
    crate::domain::settings::ContextSettings,
    Vec<serde_json::Value>,
) {
    let store = store::open(&config.store).await.unwrap();
    let health = LiveHealth::new(store.clone());
    crate::runtime::logging::capture();
    let mut composition = api::compose(
        config,
        store.clone(),
        Arc::new(StaticChannels(Vec::new())),
        health,
    )
    .await
    .unwrap();
    composition.model_tasks.report_done().await;
    let lines = crate::runtime::logging::captured();
    let context = composition.settings.models.context.clone();
    drop(composition);
    store::close(store, Duration::ZERO).await;
    (context, lines)
}

#[tokio::test]
async fn a_context_seed_applies_only_while_unsaved_and_clamps_past_the_hard_cap() {
    let seed = serde_json::json!({
        "cloud_default": 200_000,
        "local_default": 4_096,
        "chat": {"reserve": 1_024},
        "extraction": {"reserve": 2_000},
        "rewrite": {"reserve": 96},
    })
    .to_string();
    let temp = Temp::new();
    let config = temp.config(&[
        ("KANADE_MODEL_BASE_URL", "http://127.0.0.1:9/v1"),
        ("KANADE_MODEL_CONTEXT", seed.as_str()),
    ]);
    let (context, lines) = compose_context(&config).await;
    assert_eq!(context.cloud_default, 131_072);
    assert_eq!(context.local_default, 4_096);
    let clamped = lines
        .iter()
        .find(|line| line["event"] == "model_context_clamped")
        .expect("the clamp is logged");
    assert_eq!(clamped["level"], "WARN");
    assert_eq!(
        clamped["fields"],
        serde_json::json!(["models.context.cloud_default"])
    );
    assert_eq!(clamped["max"], 131_072);

    // A saved row wins; the seed (and its clamp) is not used.
    let temp = Temp::new();
    let config = temp.config(&[
        ("KANADE_MODEL_BASE_URL", "http://127.0.0.1:9/v1"),
        ("KANADE_MODEL_CONTEXT", seed.as_str()),
    ]);
    let saved = crate::domain::settings::ContextSettings {
        local_default: 12_000,
        ..Default::default()
    };
    let row = serde_json::to_string(&saved).unwrap();
    with_rows(&config, &[(keys::MODEL_CONTEXT, row.as_str())]).await;
    let (context, lines) = compose_context(&config).await;
    assert_eq!(context, saved);
    assert!(
        !lines
            .iter()
            .any(|line| line["event"] == "model_context_clamped")
    );
}

#[test]
fn an_invalid_context_seed_refuses_startup() {
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in [
        ("KANADE_MODEL_BASE_URL", "http://127.0.0.1:9/v1"),
        (
            "KANADE_MODEL_CONTEXT",
            r#"{"cloud_default":65536,"local_default":8192,"chat":{"reserve":1024},"extraction":{"reserve":9000},"rewrite":{"reserve":96}}"#,
        ),
    ] {
        values.insert(key.into(), value.into());
    }
    let error = crate::runtime::config::ModelSettings::from_mapping(&values).unwrap_err();
    assert_eq!(
        error.to_string(),
        "models.context.extraction.reserve (9000) must be smaller than its window (8192)"
    );
}

#[tokio::test]
async fn startup_warns_once_per_local_role_past_16k_and_never_for_cloud() {
    let url = model_gateway(serde_json::json!({"object": "list", "data": [
        {"id": "big-local", "kanata": {"trust_zone": "local", "context_tokens": 32_768}},
        {"id": "small-local", "kanata": {"trust_zone": "local", "context_tokens": 16_384}},
        {"id": "big-cloud", "kanata": {"context_tokens": 131_072}},
    ]}))
    .await;
    let temp = Temp::new();
    let config = temp.config(&[
        ("KANADE_MODEL_BASE_URL", url.as_str()),
        ("KANADE_EXTRACT_MODEL", "small-local"),
        ("KANADE_CHAT_MODEL", "big-local"),
        ("KANADE_REWRITE_MODEL", "big-cloud"),
    ]);
    let (_, lines) = compose_context(&config).await;
    let warnings: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|line| line["kind"] == "local_context")
        .collect();
    assert_eq!(warnings.len(), 1, "{lines:?}");
    assert_eq!(warnings[0]["level"], "WARN");
    assert_eq!(warnings[0]["role"], "chat");
    assert_eq!(warnings[0]["alias"], "big-local");
    assert_eq!(warnings[0]["window"], 32_768);
    assert_eq!(
        warnings[0]["message"],
        "Context past 16k may result in degraded performance on local models."
    );
}

#[tokio::test]
async fn startup_warns_when_a_routed_reserve_fills_its_effective_window() {
    // The catalog publishes less room than extraction's default 2500 reserve.
    let url = model_gateway(serde_json::json!({"object": "list", "data": [
        {"id": "tiny-local", "kanata": {"trust_zone": "local", "context_tokens": 2_048}},
        {"id": "roomy-local", "kanata": {"trust_zone": "local", "context_tokens": 8_192}},
    ]}))
    .await;
    let temp = Temp::new();
    let config = temp.config(&[
        ("KANADE_MODEL_BASE_URL", url.as_str()),
        ("KANADE_EXTRACT_MODEL", "tiny-local"),
        ("KANADE_CHAT_MODEL", "roomy-local"),
    ]);
    let (_, lines) = compose_context(&config).await;
    let warnings: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|line| line["kind"] == "context_reserve")
        .collect();
    assert_eq!(warnings.len(), 1, "{lines:?}");
    let warning = warnings[0];
    assert_eq!(warning["level"], "WARN");
    assert_eq!(warning["event"], "model_warning");
    assert_eq!(warning["role"], "extraction");
    assert_eq!(warning["alias"], "tiny-local");
    assert_eq!(warning["window"], 2_048);
    assert_eq!(warning["reserve"], 2_500);
    assert_eq!(
        warning["message"],
        "extraction context reserve 2500 is not smaller than tiny-local's effective window 2048; its prompts cannot fit"
    );
}
