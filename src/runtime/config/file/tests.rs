use std::collections::BTreeMap;

use super::{keys::KEYS, merge, resolve};
use crate::runtime::config::{ModelSettings, RuntimeConfig, ServeConfig};

const EXAMPLE: &str = include_str!("../../../../kanade.example.toml");

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn error(text: &str) -> String {
    merge(text, BTreeMap::new()).unwrap_err().to_string()
}

#[test]
fn the_example_file_is_a_complete_live_config() {
    let resolved = merge(EXAMPLE, BTreeMap::new()).unwrap();
    let config = ServeConfig::from_mapping(&resolved.values).unwrap();
    assert_eq!(config.runtime.timezone, chrono_tz::Asia::Kuala_Lumpur);
    assert_eq!(config.guild.guild_id, 123456789012345678);
    assert_eq!(config.instance_id, "kanade-example");
    assert!(!config.discord.v4_stopped && config.discord.gateway);
    assert!(config.runtime.admin_auth.discord.is_some());
    assert_eq!(config.models.chat_model.as_deref(), Some("kanata/chat"));
    assert_eq!(config.models.groups.len(), 1);
    assert_eq!(
        config.models.groups[0].aliases,
        ["kanata/extract", "kanata/chat"]
    );
    assert_eq!(config.seeds.watch_channel_ids, Vec::<u64>::new());
}

/// Every key is documented: uncommenting the example's keys covers the table.
#[test]
fn the_example_file_documents_every_key() {
    let uncommented: String = EXAMPLE
        .lines()
        .map(|line| {
            let rest = line.strip_prefix("# ").unwrap_or(line);
            let is_key = rest.split_once(" = ").is_some_and(|(name, _)| {
                name.bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            });
            if is_key { rest } else { line }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let resolved = merge(&uncommented, BTreeMap::new()).unwrap();
    for (key, env, _) in KEYS {
        assert!(
            resolved.values.contains_key(*env),
            "{key} is not documented"
        );
    }
}

#[test]
fn a_non_empty_environment_variable_overrides_one_key() {
    let resolved = merge(
        EXAMPLE,
        env(&[
            ("KANADE_GUILD_ID", "999999999999999999"),
            // Compose passes blank variables as empty: the file still applies.
            ("KANADE_BOSSING_ROLE_ID", ""),
            ("KANADE_UNRELATED", "kept"),
        ]),
    )
    .unwrap();
    let config = ServeConfig::from_mapping(&resolved.values).unwrap();
    assert_eq!(config.guild.guild_id, 999999999999999999);
    assert_eq!(config.guild.bossing_role_id, 223456789012345678);
    assert_eq!(resolved.values["KANADE_UNRELATED"], "kept");
    assert_eq!(config.runtime.timezone, chrono_tz::Asia::Kuala_Lumpur);
}

#[test]
fn retired_privacy_toml_keys_are_rejected_by_presence_before_environment_merge() {
    for (path, value) in [
        ("models.pseudonymize", "true"),
        ("models.pseudonymize", "false"),
        ("models.allow_external_unmasked", "true"),
        ("models.allow_external_unmasked", "false"),
    ] {
        let key = path.strip_prefix("models.").unwrap();
        let message = error(&format!("[models]\n{key} = {value}\n"));
        assert_eq!(
            message,
            format!("kanade.toml key `{path}` is retired; remove it")
        );
        assert!(!message.contains(value));
    }

    let together = merge(
        "[models]\npseudonymize = true\nallow_external_unmasked = false\n",
        BTreeMap::new(),
    )
    .unwrap_err()
    .to_string();
    assert!(together.contains("kanade.toml key `models."));
    assert!(together.contains("is retired; remove it"));
    assert!(!together.contains("true") && !together.contains("false"));

    let mixed_sources = merge(
        "[models]\npseudonymize = false\n",
        env(&[("KANADE_ALLOW_EXTERNAL_UNMASKED", "1")]),
    )
    .unwrap_err()
    .to_string();
    assert!(mixed_sources.contains("models.pseudonymize"));
    assert!(!mixed_sources.contains("false") && !mixed_sources.contains("1"));
}

#[test]
fn unknown_keys_and_bad_types_name_the_key_only() {
    assert_eq!(
        error("[discord]\nguild = \"sentinel-value\"\n"),
        "kanade.toml has an unknown key `discord.guild`"
    );
    assert_eq!(
        error("[nope]\nx = 1\n"),
        "kanade.toml has an unknown key `nope`"
    );
    assert_eq!(
        error("timezone = \"UTC\"\n"),
        "kanade.toml has an unknown key `timezone`"
    );
    assert_eq!(
        error("[runtime]\ntick_seconds = \"sentinel-value\"\n"),
        "kanade.toml `runtime.tick_seconds` must be a non-negative integer"
    );
    assert_eq!(
        error("[discord]\ngateway = 1\n"),
        "kanade.toml `discord.gateway` must be true or false"
    );
    assert_eq!(
        error("[settings]\nchat_category_ids = [1, true]\n"),
        "kanade.toml `settings.chat_category_ids` must be a list of snowflake strings or integers"
    );
    assert_eq!(
        error("models = 3\n"),
        "kanade.toml `models` must be a table"
    );
    let syntax = error("[runtime]\ntimezone = sentinel-value\n");
    assert_eq!(syntax, "kanade.toml is not valid TOML (line 2)");

    // Values that parse but fail the typed rules name the variable and the key.
    let resolved = merge(
        "[runtime]\ntimezone = \"Not/AZone-sentinel\"\n",
        BTreeMap::new(),
    )
    .unwrap();
    let error = resolved.annotate(RuntimeConfig::from_mapping(&resolved.values).unwrap_err());
    assert_eq!(
        error.to_string(),
        "KANADE_TIMEZONE must be a valid IANA timezone (kanade.toml `runtime.timezone`)"
    );
    assert!(!error.to_string().contains("sentinel"));
}

#[test]
fn secret_looking_keys_are_refused_and_file_keys_accepted() {
    for (text, key) in [
        ("[discord]\ntoken = \"sentinel-value\"\n", "discord.token"),
        ("[models]\nkey = \"sentinel-value\"\n", "models.key"),
        ("[models]\napi_key = \"sentinel-value\"\n", "models.api_key"),
        (
            "[admin]\ndiscord_client_secret = \"sentinel-value\"\n",
            "admin.discord_client_secret",
        ),
        ("[store]\npassword = \"sentinel-value\"\n", "store.password"),
        (
            "[[models.groups]]\nname = \"a\"\npermits = 1\naliases = [\"a\"]\ntoken = \"sentinel-value\"\n",
            "models.groups[0].token",
        ),
    ] {
        let message = error(text);
        assert_eq!(
            message,
            format!(
                "kanade.toml `{key}` looks like a secret; put it in a file and name that file with a `*_file` key"
            )
        );
        assert!(!message.contains("sentinel"));
    }
    let resolved = merge(
        "[discord]\ntoken_file = \"/run/secrets/t\"\n[models]\nkey_file = \"/run/secrets/k\"\n\
         [admin]\ndiscord_client_secret_file = \"/run/secrets/c\"\nedge_secret_file = \"/run/secrets/e\"\n",
        BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(
        resolved.values["KANADE_DISCORD_TOKEN_FILE"],
        "/run/secrets/t"
    );
    assert_eq!(resolved.values["KANADE_MODEL_KEY_FILE"], "/run/secrets/k");
    assert_eq!(
        resolved.values["KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE"],
        "/run/secrets/c"
    );
    assert_eq!(resolved.values["KANADE_EDGE_SECRET_FILE"], "/run/secrets/e");
}

#[test]
fn environment_only_configuration_is_unchanged() {
    let environment = env(&[
        ("KANADE_TIMEZONE", "Asia/Kuala_Lumpur"),
        ("KANADE_CONFIG", ""),
        ("KANADE_MODEL_PERMITS", "3"),
    ]);
    let resolved = resolve(environment.clone()).unwrap();
    assert_eq!(resolved.values, environment);
    let error = crate::runtime::error::Error::Configuration("KANADE_TIMEZONE is required".into());
    assert_eq!(
        resolved.annotate(error).to_string(),
        "KANADE_TIMEZONE is required"
    );
}

#[test]
fn the_config_file_is_read_from_kanade_config() {
    let path = std::env::temp_dir().join(format!("kanade-toml-{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, "[runtime]\ntimezone = \"UTC\"\n").unwrap();
    let resolved = resolve(env(&[("KANADE_CONFIG", path.to_str().unwrap())])).unwrap();
    assert_eq!(resolved.values["KANADE_TIMEZONE"], "UTC");
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        resolve(env(&[("KANADE_CONFIG", path.to_str().unwrap())]))
            .unwrap_err()
            .to_string(),
        "KANADE_CONFIG could not be read"
    );
}

#[test]
fn groups_feed_the_model_settings() {
    let text = "[models]\nbase_url = \"https://gw.example/v1\"\n\
                [[models.groups]]\nname = \"local\"\npermits = 3\naliases = [\"a\", \"b\"]\n\
                [[models.groups]]\nname = \"cloud\"\npermits = 1\naliases = [\"c\"]\n";
    let resolved = merge(text, BTreeMap::new()).unwrap();
    let models = ModelSettings::from_mapping(&resolved.values).unwrap();
    let shape: Vec<_> = models
        .groups
        .iter()
        .map(|group| (group.name.as_str(), group.permits, group.aliases.len()))
        .collect();
    assert_eq!(shape, [("local", 3, 2), ("cloud", 1, 1)]);

    assert_eq!(
        error("[[models.groups]]\nname = \"a\"\npermits = 1\n"),
        "kanade.toml `models.groups[0]` needs `aliases`"
    );
    assert_eq!(
        error("[[models.groups]]\nname = \"a\"\npermits = 1\naliases = [\"a\"]\nburst = 2\n"),
        "kanade.toml has an unknown key `models.groups[0].burst`"
    );
    let resolved = merge(
        "[models]\nbase_url = \"https://gw.example/v1\"\npermits = 2\n\
         [[models.groups]]\nname = \"a\"\npermits = 1\naliases = [\"a\"]\n",
        BTreeMap::new(),
    )
    .unwrap();
    let error = resolved.annotate(ModelSettings::from_mapping(&resolved.values).unwrap_err());
    assert_eq!(
        error.to_string(),
        "KANADE_MODEL_PERMITS and KANADE_MODEL_GROUPS are exclusive; set permits per group \
         (kanade.toml `models.permits`)"
    );
}

#[test]
fn the_debug_test_channel_is_an_optional_snowflake() {
    let channel = |line: &str| {
        let text = EXAMPLE.replace(
            "debug_user_ids = []",
            &format!("debug_user_ids = []\n{line}"),
        );
        let resolved = merge(&text, BTreeMap::new())?;
        ServeConfig::from_mapping(&resolved.values).map(|config| config.guild.test_channel_id)
    };
    assert_eq!(channel("").unwrap(), None);
    assert_eq!(
        channel("test_channel = \"523456789012345678\"").unwrap(),
        Some(523456789012345678)
    );
    assert_eq!(
        channel("test_channel = 523456789012345678").unwrap(),
        Some(523456789012345678)
    );
    assert_eq!(
        channel("test_channel = \"#bot-tests\"")
            .unwrap_err()
            .to_string(),
        "KANADE_TEST_CHANNEL_ID must be a Discord snowflake"
    );
    assert_eq!(
        channel("test_channel = true").unwrap_err().to_string(),
        "kanade.toml `discord.test_channel` must be a snowflake string or integer"
    );
    // The environment overrides the file, like every key.
    let resolved = merge(
        &EXAMPLE.replace(
            "debug_user_ids = []",
            "debug_user_ids = []\ntest_channel = \"5\"",
        ),
        env(&[("KANADE_TEST_CHANNEL_ID", "6")]),
    )
    .unwrap();
    assert_eq!(
        ServeConfig::from_mapping(&resolved.values)
            .unwrap()
            .guild
            .test_channel_id,
        Some(6)
    );
}
