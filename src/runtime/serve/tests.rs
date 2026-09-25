use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
};

use chrono::{NaiveTime, Weekday};

use super::*;
use crate::domain::settings::{Section, SettingsStore, keys, save_section};

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
    save_section(&*store, &Section::Watching(edited.watching.clone()))
        .await
        .unwrap();
    save_section(&*store, &Section::Chatbot(edited.chatbot.clone()))
        .await
        .unwrap();
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
