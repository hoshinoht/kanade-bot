use std::{collections::BTreeMap, path::Path};

use super::{
    annotate,
    keys::{KEYS, RETIRED},
    merge, resolve,
};
use crate::runtime::config::{
    Components, HealthcheckConfig, ModelSettings, RuntimeConfig, ServeConfig, SettingSeeds,
};

const EXAMPLE: &str = include_str!("../../../../kanade.example.toml");
const OPENAI_COMPATIBLE: &str = include_str!("../../../../kanade.openai-compatible.example.toml");
const COMPOSE: &str = include_str!("../../../../deploy/compose.yaml");
const DOCKERFILE: &str = include_str!("../../../../deploy/Dockerfile");
const DOCS: &str = include_str!("../../../../docs/v5/runtime-bootstrap.md");

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn error(text: &str) -> String {
    merge(text, BTreeMap::new()).unwrap_err().to_string()
}

/// The runtime image's `ENV` in deploy/Dockerfile.
fn dockerfile_environment() -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let mut lines = DOCKERFILE.lines();
    while let Some(line) = lines.next() {
        let Some(mut rest) = line.strip_prefix("ENV ") else {
            continue;
        };
        loop {
            let (pairs, more) = match rest.trim_end().strip_suffix('\\') {
                Some(pairs) => (pairs, true),
                None => (rest, false),
            };
            for pair in pairs.split_whitespace() {
                let (key, value) = pair.split_once('=').expect("ENV KEY=VALUE");
                values.insert(key.to_owned(), value.to_owned());
            }
            match (more, lines.next()) {
                (true, Some(next)) => rest = next,
                _ => break,
            }
        }
    }
    values
}

/// The bot's `environment:` in deploy/compose.yaml as Compose passes it with
/// no `.env` and no override: `${VAR:-default}` is the default and
/// `${VAR:+value}` is empty.
fn compose_environment() -> BTreeMap<String, String> {
    #[derive(serde::Deserialize)]
    struct Compose {
        services: BTreeMap<String, Service>,
    }
    #[derive(serde::Deserialize)]
    struct Service {
        #[serde(default)]
        environment: BTreeMap<String, String>,
    }
    let compose: Compose = serde_saphyr::from_str(COMPOSE).unwrap();
    compose.services["bot"]
        .environment
        .iter()
        .map(|(key, value)| {
            let value = match value
                .strip_prefix("${")
                .and_then(|rest| rest.strip_suffix('}'))
            {
                Some(expression) => match expression.split_once(":-") {
                    Some((_, default)) => default.to_owned(),
                    None => {
                        assert!(expression.contains(":+"), "{key}: {value}");
                        String::new()
                    }
                },
                None => value.clone(),
            };
            assert!(!value.contains('$'), "{key}: unsupported interpolation");
            (key.clone(), value)
        })
        .collect()
}

/// What the bot's process sees under the base Compose file: the image's
/// `ENV`, then Compose's `environment:` over it, then `pairs`.
fn image_env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut values = dockerfile_environment();
    values.extend(compose_environment());
    values.extend(env(pairs));
    values
}

/// A first start: the example file under the image and the base
/// deploy/compose.yaml, with no override and no `.env`, is a complete live
/// config with every add-on off.
#[test]
fn the_example_file_starts_under_the_image_and_base_compose() {
    let resolved = merge(EXAMPLE, image_env(&[])).unwrap();
    assert!(resolved.shadowed.is_empty(), "{:?}", resolved.shadowed);
    let config = ServeConfig::from_mapping(&resolved.values).unwrap();
    assert_eq!(config.runtime.timezone, chrono_tz::Asia::Kuala_Lumpur);
    assert_eq!(config.guild.guild_id, 123456789012345678);
    assert_eq!(config.guild.bossing_role_id, 223456789012345678);
    assert_eq!(config.components, Components::default(), "bot and web");
    assert_eq!(
        config.discord.token.path(),
        Path::new("/run/secrets/discord_token")
    );
    assert!(
        config.runtime.admin_auth.discord.is_none(),
        "admin Discord sign-in is an add-on"
    );
    assert_eq!(
        config.runtime.admin_auth.token.path(),
        Path::new("/run/secrets/admin_token")
    );
    assert_eq!(config.runtime.http.admin_host, None, "localhost names only");
    assert_eq!(config.runtime.http.trusted_proxy, None);
    assert_eq!(config.runtime.http.edge_secret, None);
    assert_eq!(
        config.runtime.admin_bind,
        "172.23.0.10:8080".parse().unwrap()
    );
    assert_eq!(config.runtime.public_bind, None, "no public.host");
    assert!(config.runtime.public_auth.discord.is_none());
    assert_eq!(
        config.models.base_url.as_deref(),
        Some("https://kanata.example/v1")
    );
    assert_eq!(
        config.models.key.as_ref().unwrap().path(),
        Path::new("/run/secrets/model_api_key")
    );
    assert_eq!(config.store.db_path, Path::new("/data/db/kanade.sqlite"));
    assert_eq!(config.store.owner_lock_dir, Path::new("/data/run"));
    assert_eq!(
        config.files.catalog_file,
        Path::new("/app/boss/bosses.yaml")
    );
    assert_eq!(config.files.persona_dir, Path::new("/config/personas"));
    assert_eq!(config.backup_dir.as_deref(), Some(Path::new("/backups")));
    assert_eq!(
        config.backup_recipients_file.as_deref(),
        Some(Path::new("/backups/recipients"))
    );
    assert_eq!(config.character_data, None);
    assert_eq!(config.seeds, SettingSeeds::default());
    // The healthcheck targets the admin listener; no URL is configured.
    let health = HealthcheckConfig::from_mapping(&resolved.values, None).unwrap();
    assert_eq!(health.target, config.runtime.admin_bind);
}

/// The public portal is one key: Compose already fixes the listener's
/// address and the tunnel's.
#[test]
fn public_host_turns_the_public_listener_on_under_compose() {
    let text = EXAMPLE.replace(
        "# host = \"kanade-public.example\"",
        "host = \"kanade-public.example\"",
    );
    let resolved = merge(&text, image_env(&[])).unwrap();
    let config = ServeConfig::from_mapping(&resolved.values).unwrap();
    assert_eq!(
        config.runtime.public_bind,
        Some("172.25.0.10:8081".parse().unwrap())
    );
    assert_eq!(
        config.runtime.http.cloudflared_peer,
        Some([172, 25, 0, 3].into())
    );
}

#[test]
fn the_openai_compatible_fragment_declares_its_models() {
    let resolved = merge(OPENAI_COMPATIBLE, BTreeMap::new()).unwrap();
    let models = ModelSettings::from_mapping(&resolved.values).unwrap();
    let declared: Vec<_> = models.capabilities.keys().map(String::as_str).collect();
    assert_eq!(
        declared,
        [
            "example-chat",
            "example-extract",
            "example-reasoning",
            "example-rewrite"
        ]
    );
    assert!(models.capabilities["example-chat"].function_tools);
    assert!(models.capabilities["example-extract"].structured_output);
    assert_eq!(models.capabilities["example-extract"].output_limit, None);
    assert_eq!(
        models.capabilities["example-reasoning"].output_limit,
        Some(kanade_llm::OutputLimit::MaxCompletionTokens)
    );
}

/// The example with every `# key = value` line uncommented.
fn uncommented(text: &str) -> String {
    text.lines()
        .map(|line| {
            let rest = line.strip_prefix("# ").unwrap_or(line);
            let is_key = rest.split_once(" = ").is_some_and(|(name, _)| {
                name.bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            });
            if is_key { rest } else { line }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every key is documented: uncommenting the example's keys covers the table.
#[test]
fn the_example_file_documents_every_key() {
    let resolved = merge(&uncommented(EXAMPLE), BTreeMap::new()).unwrap();
    for (key, env, _) in KEYS {
        assert!(
            resolved.values.contains_key(*env),
            "{key} is not documented"
        );
    }
}

/// docs/v5/runtime-bootstrap.md's "Config file" table is exactly [`KEYS`].
#[test]
fn the_documented_key_table_matches_the_keys() {
    let section = DOCS
        .split_once("\n### Config file\n")
        .and_then(|(_, rest)| rest.split("\n### ").next())
        .expect("a Config file section");
    let documented: Vec<(String, String)> = section
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
            let key = cells.first()?.strip_prefix('`')?.strip_suffix('`')?;
            let env = cells.get(1)?.strip_prefix('`')?.strip_suffix('`')?;
            (key.contains('.') && env.starts_with("KANADE_"))
                .then(|| (key.to_owned(), env.to_owned()))
        })
        .collect();
    let keys: Vec<(String, String)> = KEYS
        .iter()
        .map(|(key, env, _)| ((*key).to_owned(), (*env).to_owned()))
        .collect();
    assert_eq!(documented, keys, "the docs table drifted from KEYS");
}

#[test]
fn a_non_empty_environment_variable_overrides_one_key_and_is_reported() {
    let resolved = merge(
        EXAMPLE,
        image_env(&[
            ("KANADE_GUILD_ID", "999999999999999999"),
            // Compose passes blank variables as empty: the file still applies.
            ("KANADE_BOSSING_ROLE_ID", ""),
            ("KANADE_UNRELATED", "kept"),
        ]),
    )
    .unwrap();
    assert_eq!(resolved.shadowed, ["discord.guild_id"]);
    let config = ServeConfig::from_mapping(&resolved.values).unwrap();
    assert_eq!(config.guild.guild_id, 999999999999999999);
    assert_eq!(config.guild.bossing_role_id, 223456789012345678);
    assert_eq!(resolved.values["KANADE_UNRELATED"], "kept");
    assert_eq!(config.runtime.timezone, chrono_tz::Asia::Kuala_Lumpur);
}

/// Each removed key refuses by name with its new home, whatever its value,
/// before any environment merge.
#[test]
fn removed_keys_refuse_naming_their_new_home_never_the_value() {
    for (name, home) in RETIRED
        .iter()
        .filter(|(name, _)| !name.starts_with("KANADE_"))
    {
        let (table, key) = name.rsplit_once('.').unwrap();
        for value in ["\"sentinel-value\"", "true", "7"] {
            let message = error(&format!("[{table}]\n{key} = {value}\n"));
            assert_eq!(message, format!("kanade.toml `{name}` was removed: {home}"));
            assert!(!message.contains("sentinel"));
        }
    }
    // The privacy keys refuse even when false, whatever the environment says.
    let mixed_sources = merge(
        "[models]\npseudonymize = false\n",
        env(&[("KANADE_ALLOW_EXTERNAL_UNMASKED", "1")]),
    )
    .unwrap_err()
    .to_string();
    assert_eq!(
        mixed_sources,
        "kanade.toml `models.pseudonymize` was removed: remove it; model calls are never masked"
    );
    // A whole removed table refuses on its first key.
    assert_eq!(
        error("[store]\ndb_path = \"/data/db/kanade.sqlite\"\n"),
        "kanade.toml `store.db_path` was removed: the image sets it"
    );
    assert_eq!(
        error("[settings.run_lengths]\ndefault_minutes = 30\n"),
        "kanade.toml `settings.run_lengths` was removed: set it in Config › Run lengths"
    );
    assert_eq!(
        error("[models.context]\ncloud_default = 65536\n"),
        "kanade.toml `models.context` was removed: set it in Config › Models"
    );
    assert_eq!(
        error("[models.chat]\nmodel = \"kanata/chat\"\n"),
        "kanade.toml `models.chat.model` was removed: set it in Config › Models"
    );
}

/// The `[settings]` seeds now belong to Config: each names its section.
#[test]
fn settings_keys_refuse_naming_their_config_section() {
    for (key, section) in [
        ("post_channel_id = \"5\"", "Weekly digest"),
        ("watch_channel_ids = []", "Channels"),
        ("watch_category_ids = []", "Channels"),
        ("chat_category_ids = [1]", "Channels"),
        ("extraction_enabled = true", "Chat watching"),
        ("chat_enabled = false", "Chatbot"),
        ("day_of_ping_time = \"09:00\"", "Pings"),
        ("countdown_minutes = [60, 15]", "Pings"),
    ] {
        let message = error(&format!("[settings]\n{key}\n"));
        assert!(
            message.ends_with(&format!("was removed: set it in Config › {section}")),
            "{message}"
        );
    }
    // The week reset moved next to the timezone.
    assert_eq!(
        error("[settings]\nboss_week_reset_weekday = \"thu\"\n"),
        "kanade.toml `settings.boss_week_reset_weekday` was removed: it moved to \
         `runtime.boss_week_reset_weekday`"
    );
    let resolved = merge(
        "[runtime]\nboss_week_reset_weekday = \"wed\"\nboss_week_reset_time = \"08:30\"\n",
        BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(resolved.values["KANADE_BOSS_WEEK_RESET_WEEKDAY"], "wed");
    assert_eq!(resolved.values["KANADE_BOSS_WEEK_RESET_TIME"], "08:30");
}

/// Removed variables refuse in every command, by name; blank ones (Compose's
/// unset) do not count. The seed variables stay for tests and harnesses.
#[test]
fn removed_variables_refuse_naming_their_new_home_never_the_value() {
    for (name, home) in RETIRED
        .iter()
        .filter(|(name, _)| name.starts_with("KANADE_"))
    {
        let message = resolve(env(&[(name, "/run/secrets/sentinel-value")]))
            .unwrap_err()
            .to_string();
        assert_eq!(message, format!("{name} was removed: {home}"));
        assert!(!message.contains("sentinel"));
        assert!(resolve(env(&[(name, "")])).is_ok(), "{name} blank");
    }
    let seeds = env(&[
        ("KANADE_POST_CHANNEL_ID", "11"),
        ("KANADE_RUN_LENGTHS", "{}"),
        ("KANADE_CHAT_MODEL", "kanata/chat"),
    ]);
    assert_eq!(resolve(seeds.clone()).unwrap().values, seeds);
}

/// Every removed key or variable says where its value lives now, and none
/// is also a current key or variable.
#[test]
fn every_removed_entry_names_its_new_home() {
    for (name, home) in RETIRED {
        assert!(
            !KEYS.iter().any(|(key, env, _)| key == name || env == name),
            "{name} is both current and removed"
        );
        // A moved key names a key that exists.
        if let Some(target) = home.strip_prefix("it moved to `") {
            let target = target.strip_suffix('`').unwrap();
            assert!(KEYS.iter().any(|(key, _, _)| *key == target), "{name}");
        }
        // A replaced key or variable names a current one.
        if let Some(target) = home.strip_prefix("replaced by ") {
            let target = target.split(' ').next().unwrap().trim_matches('`');
            assert!(
                KEYS.iter()
                    .any(|(key, env, _)| *key == target || *env == target),
                "{name}"
            );
        }
        let homes = [
            "secrets directory as `",
            "backups directory as `",
            "set it in Config › ",
            "it moved to `",
            "the image sets it",
            "Compose sets it",
            "compose.override.yaml",
            "the healthcheck targets",
            "it is https://",
            "remove it;",
            "replaced by ",
        ];
        assert!(
            homes.iter().any(|known| home.contains(known)),
            "{name}: {home}"
        );
    }
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
        error("[modules]\nguides = 1\n"),
        "kanade.toml `modules.guides` must be true or false"
    );
    assert_eq!(
        error("[runtime]\ncomponents = []\n"),
        "kanade.toml `runtime.components` must be a non-empty list of strings without commas"
    );
    assert_eq!(
        error("[discord]\ndebug_user_ids = [1, true]\n"),
        "kanade.toml `discord.debug_user_ids` must be a list of snowflake strings or integers"
    );
    assert_eq!(
        error("models = 3\n"),
        "kanade.toml `models` must be a table"
    );
    let syntax = error("[runtime]\ntimezone = sentinel-value\n");
    assert_eq!(syntax, "kanade.toml is not valid TOML (line 2)");

    // Values that parse but fail the typed rules name the key and the variable.
    let resolved = merge(
        "[runtime]\ntimezone = \"Not/AZone-sentinel\"\n",
        BTreeMap::new(),
    )
    .unwrap();
    let error = annotate(RuntimeConfig::from_mapping(&resolved.values).unwrap_err());
    assert_eq!(
        error.to_string(),
        "kanade.toml `runtime.timezone` [KANADE_TIMEZONE] must be a valid IANA timezone"
    );
    assert!(!error.to_string().contains("sentinel"));
}

/// Errors name every key a message mentions, wherever its value came from;
/// variables without a key stay as they are.
#[test]
fn annotate_names_each_key_with_its_variable_in_brackets() {
    use crate::runtime::error::Error;
    let named = |message: &str| annotate(Error::Configuration(message.into())).to_string();
    assert_eq!(
        named("KANADE_TIMEZONE is required"),
        "kanade.toml `runtime.timezone` [KANADE_TIMEZONE] is required"
    );
    assert_eq!(
        named("KANADE_PUBLIC_BIND is required when KANADE_PUBLIC_HOST is set"),
        "KANADE_PUBLIC_BIND is required when kanade.toml `public.host` [KANADE_PUBLIC_HOST] is set"
    );
    assert_eq!(
        named("KANADE_MODULE_CHARACTER_DATA needs the roster module on (KANADE_MODULE_ROSTER)"),
        "kanade.toml `modules.character_data` [KANADE_MODULE_CHARACTER_DATA] needs the roster \
         module on (kanade.toml `modules.roster` [KANADE_MODULE_ROSTER])"
    );
    // Image paths, addresses and seeds have no key; parts of words are not
    // names.
    for plain in [
        "KANADE_DB_PATH is required",
        "KANADE_RUN_LENGTHS.default_minutes must be 5-240 whole minutes",
        "X_KANADE_TIMEZONE is odd",
        "KANADE_TIMEZONES is not a variable",
    ] {
        assert_eq!(named(plain), plain);
    }
    // Other errors are left alone.
    assert_eq!(
        annotate(Error::Usage("KANADE_TIMEZONE".into())).to_string(),
        Error::Usage("KANADE_TIMEZONE".into()).to_string()
    );
}

#[test]
fn secret_looking_keys_are_refused() {
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
            "[models]\nnew_key_file = \"/run/secrets/sentinel\"\n",
            "models.new_key_file",
        ),
        (
            "[[models.groups]]\nname = \"a\"\npermits = 1\naliases = [\"a\"]\ntoken = \"sentinel-value\"\n",
            "models.groups[0].token",
        ),
    ] {
        let message = error(text);
        assert_eq!(
            message,
            format!(
                "kanade.toml `{key}` looks like a secret; put secrets in the secrets directory, never in this file"
            )
        );
        assert!(!message.contains("sentinel"));
    }
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
    assert!(resolved.shadowed.is_empty());
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
    let error = annotate(ModelSettings::from_mapping(&resolved.values).unwrap_err());
    assert_eq!(
        error.to_string(),
        "kanade.toml `models.permits` [KANADE_MODEL_PERMITS] and kanade.toml `models.groups` \
         [KANADE_MODEL_GROUPS] are exclusive; set permits per group"
    );
}

#[test]
fn capability_declarations_feed_the_model_settings() {
    use kanade_llm::{Effort, ModelCapabilities, OutputLimit};

    let base = "[models]\nbase_url = \"https://llm.example/v1\"\n";
    let models = |tables: &str| {
        let resolved = merge(&format!("{base}{tables}"), BTreeMap::new()).unwrap();
        ModelSettings::from_mapping(&resolved.values).map_err(|error| annotate(error).to_string())
    };
    let settings = models(
        "[models.capabilities.\"plain/chat\"]\nfunction_tools = true\nstructured_output = true\n\
         reasoning_control = true\nreasoning_efforts = [\"off\", \"medium\"]\n\
         context_tokens = 32768\nmax_output_tokens = 2048\n\
         [models.capabilities.\"plain/extract\"]\nstructured_output = true\n\
         output_limit = \"max_completion_tokens\"\n",
    )
    .unwrap();
    assert_eq!(
        settings.capabilities,
        BTreeMap::from([
            (
                "plain/chat".to_owned(),
                ModelCapabilities {
                    function_tools: true,
                    structured_output: true,
                    reasoning_control: true,
                    reasoning_efforts: Some(vec![Effort::Off, Effort::Medium]),
                    context_tokens: Some(32_768),
                    max_output_tokens: Some(2048),
                    ..ModelCapabilities::minimal()
                }
            ),
            (
                "plain/extract".to_owned(),
                ModelCapabilities {
                    structured_output: true,
                    output_limit: Some(OutputLimit::MaxCompletionTokens),
                    ..ModelCapabilities::minimal()
                }
            ),
        ])
    );
    assert!(models("").unwrap().capabilities.is_empty());

    // Unknown fields and bad values refuse, pointing at the TOML key.
    let at = "kanade.toml `models.capabilities` [KANADE_MODEL_CAPABILITIES]";
    assert_eq!(
        models("[models.capabilities.m]\nstreaming = true\n").unwrap_err(),
        format!(
            "{at} `m` has an unknown field `streaming`; declare \
             structured_output, function_tools, sampling_controls, reasoning_control, \
             reasoning_efforts, context_tokens, max_output_tokens or output_limit"
        )
    );
    assert_eq!(
        models("[models.capabilities.m]\noutput_limit = \"max_output\"\n").unwrap_err(),
        format!("{at} `m`.output_limit must be \"max_tokens\" or \"max_completion_tokens\"")
    );
    assert_eq!(
        models("[models.capabilities.m]\nfunction_tools = \"sentinel-value\"\n").unwrap_err(),
        format!("{at} `m`.function_tools must be true or false")
    );
    assert_eq!(
        models("[models.capabilities.m]\nmax_output_tokens = 0\n").unwrap_err(),
        format!("{at} `m`.max_output_tokens must be a token count from 1 to 1048576")
    );
    assert_eq!(
        models("[models.capabilities]\nm = 3\n").unwrap_err(),
        format!("{at} `m` must be a table of capability fields")
    );
    assert_eq!(
        error("[models]\ncapabilities = 3\n"),
        "kanade.toml `models.capabilities` must be a table of model alias tables"
    );
    // Declarations need a model gateway, like every other model key.
    let resolved = merge(
        "[models.capabilities.m]\nfunction_tools = true\n",
        BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(
        annotate(ModelSettings::from_mapping(&resolved.values).unwrap_err()).to_string(),
        format!("{at} requires kanade.toml `models.base_url` [KANADE_MODEL_BASE_URL]")
    );
}

#[test]
fn the_debug_test_channel_is_an_optional_snowflake() {
    // The last active `[discord]` line.
    const ROLE: &str = "bossing_role_id = \"223456789012345678\"";
    let channel = |line: &str| {
        let text = EXAMPLE.replace(ROLE, &format!("{ROLE}\n{line}"));
        let resolved = merge(&text, image_env(&[]))?;
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
        &EXAMPLE.replace(ROLE, &format!("{ROLE}\ntest_channel = \"5\"")),
        image_env(&[("KANADE_TEST_CHANNEL_ID", "6")]),
    )
    .unwrap();
    assert_eq!(resolved.shadowed, ["discord.test_channel"]);
    assert_eq!(
        ServeConfig::from_mapping(&resolved.values)
            .unwrap()
            .guild
            .test_channel_id,
        Some(6)
    );
}
