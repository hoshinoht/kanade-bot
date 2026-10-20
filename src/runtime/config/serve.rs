//! Live `serve` configuration: everything beyond the HTTP listeners.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Duration,
};

use chrono::{NaiveTime, Weekday};

use crate::domain::settings::{OVERRIDE_RUN_MINUTES, RUN_MINUTES, RunLengths};

use super::{
    DiscordSettings, Error, FileSettings, GuildSettings, ModelSettings, RuntimeConfig,
    StoreSettings, character_data::CharacterDataSettings, flag, guild, non_empty,
    parse_bounded_u64,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeConfig {
    pub runtime: RuntimeConfig,
    pub discord: DiscordSettings,
    pub guild: GuildSettings,
    pub store: StoreSettings,
    pub files: FileSettings,
    pub models: ModelSettings,
    pub tick: Duration,
    pub instance_id: String,
    pub seeds: SettingSeeds,
    /// `KANADE_BACKUP_DIR`, listed read only by History checkpoints.
    pub backup_dir: Option<PathBuf>,
    /// `<KANADE_BACKUP_DIR>/recipients`, checked at startup when present so
    /// a broken key file fails the deploy, not the next backup.
    pub backup_recipients_file: Option<PathBuf>,
    pub modules: Modules,
    /// Module `character_data`'s file and key; `None` while it is off.
    pub character_data: Option<CharacterDataSettings>,
}

/// Optional modules, read once at startup: a change applies on restart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modules {
    /// `KANADE_MODULE_ROSTER` (`modules.roster`, default off): the manual
    /// character roster's admin routes.
    pub roster: bool,
    /// `KANADE_MODULE_CHARACTER_DATA` (`modules.character_data`, default
    /// off, needs `roster`): NEXON Open API data for linked characters.
    pub character_data: bool,
    /// `KANADE_MODULE_GUIDES` (`modules.guides`, default off): the guide
    /// publisher's admin routes and its publish worker.
    pub guides: bool,
}

/// Initial runtime settings from the `KANADE_*` seed variables (tests and
/// harnesses; `kanade.toml` holds only the week reset); applied only where
/// the store has none yet. `None` and empty lists keep the code default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingSeeds {
    pub post_channel_id: Option<u64>,
    pub watch_channel_ids: Vec<u64>,
    pub watch_category_ids: Vec<u64>,
    /// Kanade chats in every channel of these categories.
    pub chat_category_ids: Vec<u64>,
    pub extraction_enabled: Option<bool>,
    pub chat_enabled: Option<bool>,
    pub reset_weekday: Option<Weekday>,
    pub reset_time: Option<NaiveTime>,
    pub day_of_ping_time: Option<NaiveTime>,
    /// Largest first, no duplicates.
    pub countdown_minutes: Option<Vec<u32>>,
    /// `KANADE_RUN_LENGTHS`, used only while no saved row exists.
    pub run_lengths: Option<RunLengths>,
}

impl ServeConfig {
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        let modules = Modules {
            roster: flag(values, "KANADE_MODULE_ROSTER")?.unwrap_or(false),
            character_data: flag(values, super::character_data::FLAG_KEY)?.unwrap_or(false),
            guides: flag(values, "KANADE_MODULE_GUIDES")?.unwrap_or(false),
        };
        Ok(Self {
            runtime: RuntimeConfig::from_mapping(values)?,
            discord: DiscordSettings::from_mapping(values)?,
            guild: GuildSettings::from_mapping(values)?,
            store: StoreSettings::from_mapping(values)?,
            files: FileSettings::from_mapping(values),
            models: ModelSettings::from_mapping(values)?,
            tick: Duration::from_secs(parse_bounded_u64(
                values,
                "KANADE_TICK_SECONDS",
                30,
                5,
                300,
            )?),
            instance_id: instance_id(values)?,
            seeds: SettingSeeds::from_mapping(values)?,
            backup_dir: super::backup::optional_dir(values)?,
            backup_recipients_file: super::backup::recipients_file(values),
            character_data: CharacterDataSettings::from_mapping(
                values,
                modules.character_data,
                modules.roster,
            )?,
            modules,
        })
    }
}

impl SettingSeeds {
    fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        // Removed before release: chat follows categories, never a channel list.
        if non_empty(values, "KANADE_PILOT_CHANNEL_IDS").is_some() {
            return Err(Error::Configuration(
                "KANADE_PILOT_CHANNEL_IDS was removed; use KANADE_CHAT_CATEGORY_IDS".into(),
            ));
        }
        Ok(Self {
            post_channel_id: guild::optional(values, "KANADE_POST_CHANNEL_ID")?,
            watch_channel_ids: guild::list(values, "KANADE_WATCH_CHANNEL_IDS")?,
            watch_category_ids: guild::list(values, "KANADE_WATCH_CATEGORY_IDS")?,
            chat_category_ids: guild::list(values, "KANADE_CHAT_CATEGORY_IDS")?,
            extraction_enabled: flag(values, "KANADE_EXTRACTION_ENABLED")?,
            chat_enabled: flag(values, "KANADE_CHAT_ENABLED")?,
            reset_weekday: non_empty(values, "KANADE_BOSS_WEEK_RESET_WEEKDAY")
                .map(|value| {
                    weekday(value).ok_or_else(|| {
                        Error::Configuration(
                            "KANADE_BOSS_WEEK_RESET_WEEKDAY must be one of mon..sun".into(),
                        )
                    })
                })
                .transpose()?,
            reset_time: clock(values, "KANADE_BOSS_WEEK_RESET_TIME")?,
            day_of_ping_time: clock(values, "KANADE_DAY_OF_PING_TIME")?,
            countdown_minutes: countdowns(values, "KANADE_COUNTDOWN_MINUTES")?,
            run_lengths: run_lengths(values)?,
        })
    }
}

fn run_lengths(values: &BTreeMap<String, String>) -> Result<Option<RunLengths>, Error> {
    let Some(value) = non_empty(values, "KANADE_RUN_LENGTHS") else {
        return Ok(None);
    };
    let lengths: RunLengths = serde_json::from_str(value)
        .map_err(|_| Error::Configuration("KANADE_RUN_LENGTHS must be run lengths JSON".into()))?;
    if !RUN_MINUTES.contains(&lengths.default_minutes) {
        return Err(Error::Configuration(
            "KANADE_RUN_LENGTHS.default_minutes must be 5-240 whole minutes".into(),
        ));
    }
    let mut pairs = BTreeSet::new();
    for (index, override_) in lengths.overrides.iter().enumerate() {
        let field = format!("KANADE_RUN_LENGTHS.overrides[{index}]");
        if !OVERRIDE_RUN_MINUTES.contains(&override_.minutes) {
            return Err(Error::Configuration(format!(
                "{field}.minutes must be 5-480 whole minutes"
            )));
        }
        if override_.boss.is_empty() {
            return Err(Error::Configuration(format!(
                "{field}.boss must not be empty"
            )));
        }
        if override_.difficulty.is_empty() {
            return Err(Error::Configuration(format!(
                "{field}.difficulty must not be empty"
            )));
        }
        if !pairs.insert((&override_.boss, &override_.difficulty)) {
            return Err(Error::Configuration(format!(
                "{field} duplicates a previous boss and difficulty pair"
            )));
        }
    }
    Ok(Some(lengths))
}

/// A three-letter day name (`mon` … `sun`), any case.
fn weekday(value: &str) -> Option<Weekday> {
    (value.len() == 3).then(|| value.parse().ok()).flatten()
}

/// Exactly `HH:MM`, 24-hour.
fn clock(values: &BTreeMap<String, String>, key: &str) -> Result<Option<NaiveTime>, Error> {
    let Some(value) = non_empty(values, key) else {
        return Ok(None);
    };
    let bytes = value.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        bytes[range.clone()]
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| value[range].parse::<u32>().ok())
            .flatten()
    };
    let time = (bytes.len() == 5 && bytes[2] == b':')
        .then(|| NaiveTime::from_hms_opt(digits(0..2)?, digits(3..5)?, 0))
        .flatten();
    time.map(Some)
        .ok_or_else(|| Error::Configuration(format!("{key} must be HH:MM (24-hour)")))
}

fn countdowns(values: &BTreeMap<String, String>, key: &str) -> Result<Option<Vec<u32>>, Error> {
    let Some(text) = non_empty(values, key) else {
        return Ok(None);
    };
    let mut minutes = text
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            let ok = item.bytes().all(|byte| byte.is_ascii_digit());
            ok.then(|| item.parse::<u32>().ok())
                .flatten()
                .filter(|minute| *minute > 0)
                .ok_or_else(|| {
                    Error::Configuration(format!(
                        "{key} must be comma-separated positive whole minutes"
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    minutes.sort_unstable_by(|a, b| b.cmp(a));
    minutes.dedup();
    Ok(Some(minutes))
}

fn instance_id(values: &BTreeMap<String, String>) -> Result<String, Error> {
    let Some(id) = non_empty(values, "KANADE_INSTANCE_ID") else {
        let random = uuid::Uuid::new_v4().simple().to_string();
        return Ok(format!("kanade-{}", &random[..12]));
    };
    let ok = id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    ok.then(|| id.to_owned()).ok_or_else(|| {
        Error::Configuration(
            "KANADE_INSTANCE_ID must be at most 64 letters, digits, `-`, `_` or `.`".into(),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const BASE: [(&str, &str); 6] = [
        ("KANADE_TIMEZONE", "Asia/Kuala_Lumpur"),
        ("KANADE_SECRETS_DIR", "/run/secrets"),
        ("KANADE_GUILD_ID", "123456789012345678"),
        ("KANADE_BOSSING_ROLE_ID", "223456789012345678"),
        ("KANADE_DB_PATH", "/data/kanade.sqlite3"),
        ("KANADE_OWNER_LOCK_DIR", "/run/kanade/locks"),
    ];

    fn config(pairs: &[(&str, &str)]) -> Result<ServeConfig, String> {
        let mut values: BTreeMap<String, String> = BASE
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect();
        for (key, value) in pairs {
            values.insert((*key).into(), (*value).into());
        }
        ServeConfig::from_mapping(&values).map_err(|error| error.to_string())
    }

    #[test]
    fn minimal_live_config_takes_defaults() {
        let config = config(&[]).unwrap();
        assert_eq!(config.guild.guild_id, 123456789012345678);
        assert_eq!(config.guild.admin_role_id, None);
        assert_eq!(config.files.catalog_file, PathBuf::from("boss/bosses.yaml"));
        assert_eq!(config.files.persona_dir, PathBuf::from("config/personas"));
        assert_eq!(config.models.base_url, None);
        assert_eq!(config.models.permits, 2);
        assert_eq!(config.tick, Duration::from_secs(30));
        assert!(config.instance_id.starts_with("kanade-"));
        assert_eq!(config.seeds, SettingSeeds::default());
        assert_eq!(config.modules, Modules::default(), "modules start off");
    }

    #[test]
    fn modules_are_strict_flags() {
        for (value, on) in [("", false), ("0", false), ("1", true)] {
            let modules = config(&[("KANADE_MODULE_ROSTER", value)]).unwrap().modules;
            assert_eq!(modules.roster, on, "{value:?}");
            let modules = config(&[("KANADE_MODULE_GUIDES", value)]).unwrap().modules;
            assert_eq!(modules.guides, on, "{value:?}");
        }
        for key in ["KANADE_MODULE_ROSTER", "KANADE_MODULE_GUIDES"] {
            assert_eq!(
                config(&[(key, "on")]).unwrap_err(),
                format!("{key} must be 0 or 1")
            );
        }
    }

    #[test]
    fn character_data_needs_the_roster_and_its_own_file() {
        // Off: its keys are ignored and nothing is configured.
        let off = config(&[("KANADE_CHARACTER_DATA_DB_PATH", "relative")]).unwrap();
        assert_eq!(off.character_data, None);
        assert!(!off.modules.character_data);
        assert_eq!(
            config(&[("KANADE_MODULE_CHARACTER_DATA", "1")]).unwrap_err(),
            "KANADE_MODULE_CHARACTER_DATA needs the roster module on (KANADE_MODULE_ROSTER)"
        );
        let on = [
            ("KANADE_MODULE_ROSTER", "1"),
            ("KANADE_MODULE_CHARACTER_DATA", "1"),
        ];
        assert_eq!(
            config(&on).unwrap_err(),
            "KANADE_CHARACTER_DATA_DB_PATH is required"
        );
        let mut pairs = on.to_vec();
        pairs.push((
            "KANADE_CHARACTER_DATA_DB_PATH",
            "/chardata/db/chardata.sqlite",
        ));
        let settings = config(&pairs).unwrap().character_data.unwrap();
        assert_eq!(
            settings.db_path,
            PathBuf::from("/chardata/db/chardata.sqlite")
        );
        assert_eq!(
            settings.nexon_key.unwrap().path(),
            std::path::Path::new("/run/secrets/nexon_api_key"),
            "read at startup when present"
        );
        // Estimates: off without a scouter URL, refused for a public host.
        assert_eq!(settings.scouter_url, None, "estimates are optional");
        let mut public = pairs.clone();
        public.push(("KANADE_SCOUTER_URL", "http://scouter.example.com:8091"));
        assert!(
            config(&public)
                .unwrap_err()
                .starts_with("KANADE_SCOUTER_URL must be http://host[:port]")
        );
        pairs.push(("KANADE_SCOUTER_URL", "http://scouter:8091"));
        assert_eq!(
            config(&pairs).unwrap().character_data.unwrap().scouter_url,
            Some("http://scouter:8091".into())
        );
        // Off, the URL is ignored like the module's other keys.
        let ignored = config(&[("KANADE_SCOUTER_URL", "https://public.example")]).unwrap();
        assert_eq!(ignored.character_data, None);
    }

    #[test]
    fn live_serve_names_its_token_file_and_takes_a_strict_gateway_flag() {
        for (value, gateway) in [("", true), ("1", true), ("0", false)] {
            let discord = config(&[("KANADE_DISCORD_GATEWAY", value)])
                .unwrap()
                .discord;
            assert_eq!(discord.gateway, gateway);
        }
        assert_eq!(
            config(&[("KANADE_DISCORD_GATEWAY", "off")]).unwrap_err(),
            "KANADE_DISCORD_GATEWAY must be 0 or 1"
        );
        assert_eq!(
            config(&[]).unwrap().discord.token.path(),
            std::path::Path::new("/run/secrets/discord_token")
        );
        for plain in ["KANADE_DISCORD_TOKEN", "DISCORD_TOKEN"] {
            let error = config(&[(plain, "secret-token")]).unwrap_err();
            assert_eq!(
                error,
                format!("{plain} is not read; put it in the secrets directory as `discord_token`")
            );
        }
    }

    #[test]
    fn schedule_seeds_are_strict() {
        let seeds = config(&[
            ("KANADE_BOSS_WEEK_RESET_WEEKDAY", "Wed"),
            ("KANADE_BOSS_WEEK_RESET_TIME", "08:30"),
            ("KANADE_DAY_OF_PING_TIME", "23:59"),
            ("KANADE_COUNTDOWN_MINUTES", " 15,60,, 15 "),
            ("KANADE_WATCH_CATEGORY_IDS", "21,22"),
            ("KANADE_CHAT_CATEGORY_IDS", "31"),
            ("KANADE_EXTRACTION_ENABLED", "0"),
        ])
        .unwrap()
        .seeds;
        assert_eq!(seeds.reset_weekday, Some(Weekday::Wed));
        assert_eq!(seeds.reset_time, NaiveTime::from_hms_opt(8, 30, 0));
        assert_eq!(seeds.day_of_ping_time, NaiveTime::from_hms_opt(23, 59, 0));
        assert_eq!(seeds.countdown_minutes, Some(vec![60, 15]));
        assert_eq!(seeds.watch_category_ids, [21, 22]);
        assert_eq!(seeds.chat_category_ids, [31]);
        assert_eq!(seeds.extraction_enabled, Some(false));
        assert_eq!(seeds.chat_enabled, None);
        for bad in ["thursday", "th", "7"] {
            assert_eq!(
                config(&[("KANADE_BOSS_WEEK_RESET_WEEKDAY", bad)]).unwrap_err(),
                "KANADE_BOSS_WEEK_RESET_WEEKDAY must be one of mon..sun",
                "{bad}"
            );
        }
        for bad in [
            "8:30",
            "24:00",
            "08:60",
            "0830",
            "08:30pm",
            "+8:30",
            "٠٨:٣٠",
        ] {
            assert_eq!(
                config(&[("KANADE_DAY_OF_PING_TIME", bad)]).unwrap_err(),
                "KANADE_DAY_OF_PING_TIME must be HH:MM (24-hour)",
                "{bad}"
            );
        }
        for bad in ["0", "15,x", "-5", "+5"] {
            assert_eq!(
                config(&[("KANADE_COUNTDOWN_MINUTES", bad)]).unwrap_err(),
                "KANADE_COUNTDOWN_MINUTES must be comma-separated positive whole minutes",
                "{bad}"
            );
        }
        assert_eq!(
            config(&[("KANADE_PILOT_CHANNEL_IDS", "1")]).unwrap_err(),
            "KANADE_PILOT_CHANNEL_IDS was removed; use KANADE_CHAT_CATEGORY_IDS"
        );
    }

    #[test]
    fn run_length_seed_bounds_and_pairs_are_strict() {
        let valid = config(&[(
            "KANADE_RUN_LENGTHS",
            r#"{"default_minutes":20,"overrides":[{"boss":"BM","difficulty":"h","minutes":90}]}"#,
        )])
        .unwrap()
        .seeds
        .run_lengths
        .unwrap();
        assert_eq!(valid.default_minutes, 20);
        assert_eq!(valid.overrides[0].minutes, 90);
        for (value, field) in [
            (
                r#"{"default_minutes":0,"overrides":[]}"#,
                "KANADE_RUN_LENGTHS.default_minutes",
            ),
            (
                r#"{"default_minutes":30,"overrides":[{"boss":"BM","difficulty":"h","minutes":481}]}"#,
                "KANADE_RUN_LENGTHS.overrides[0].minutes",
            ),
            (
                r#"{"default_minutes":30,"overrides":[{"boss":"BM","difficulty":"h","minutes":60},{"boss":"BM","difficulty":"h","minutes":90}]}"#,
                "KANADE_RUN_LENGTHS.overrides[1]",
            ),
        ] {
            let error = config(&[("KANADE_RUN_LENGTHS", value)]).unwrap_err();
            assert!(error.contains(field), "{error}");
        }
    }

    #[test]
    fn snowflakes_are_canonical() {
        for bad in [
            "0",
            "0123",
            "+1",
            "-1",
            "1e5",
            "18446744073709551616",
            "abc",
        ] {
            assert_eq!(
                config(&[("KANADE_GUILD_ID", bad)]).unwrap_err(),
                "KANADE_GUILD_ID must be a Discord snowflake",
                "{bad}"
            );
        }
        assert_eq!(
            config(&[("KANADE_BOSSING_ROLE_ID", "")]).unwrap_err(),
            "KANADE_BOSSING_ROLE_ID is required"
        );
        let parsed = config(&[
            ("KANADE_ADMIN_ROLE_ID", "18446744073709551615"),
            ("KANADE_DEBUG_USER_IDS", " 5 , 7,,5 "),
            ("KANADE_WATCH_CHANNEL_IDS", "9,10"),
            ("KANADE_POST_CHANNEL_ID", "11"),
            ("KANADE_CHAT_ENABLED", "1"),
        ])
        .unwrap();
        assert_eq!(parsed.guild.admin_role_id, Some(u64::MAX));
        assert_eq!(parsed.guild.debug_user_ids, [5, 7]);
        assert_eq!(parsed.seeds.watch_channel_ids, [9, 10]);
        assert_eq!(parsed.seeds.post_channel_id, Some(11));
        assert_eq!(parsed.seeds.chat_enabled, Some(true));
        assert_eq!(parsed.seeds.extraction_enabled, None);
        assert_eq!(
            config(&[("KANADE_CHAT_CATEGORY_IDS", "1,x")]).unwrap_err(),
            "KANADE_CHAT_CATEGORY_IDS must be comma-separated Discord snowflakes"
        );
        assert_eq!(
            config(&[("KANADE_EXTRACTION_ENABLED", "true")]).unwrap_err(),
            "KANADE_EXTRACTION_ENABLED must be 0 or 1"
        );
    }

    #[test]
    fn store_paths_are_absolute_and_plain() {
        for bad in ["data/kanade.sqlite3", "/data/../kanade.sqlite3"] {
            assert_eq!(
                config(&[("KANADE_DB_PATH", bad)]).unwrap_err(),
                "KANADE_DB_PATH must be an absolute path without `..`",
                "{bad}"
            );
        }
        assert_eq!(
            config(&[("KANADE_OWNER_LOCK_DIR", "")]).unwrap_err(),
            "KANADE_OWNER_LOCK_DIR is required"
        );
    }

    #[test]
    fn model_settings_follow_the_endpoint_rule_and_never_take_plain_keys() {
        for good in [
            "https://gw.example/api",
            "http://127.0.0.1:11434",
            "http://[::1]:8000/v1",
            "http://localhost:8000",
            "http://host.docker.internal:4000",
        ] {
            assert!(config(&[("KANADE_MODEL_BASE_URL", good)]).is_ok(), "{good}");
        }
        for bad in [
            "http://gw.example",
            "ftp://gw.example",
            "https://user:pw@gw.example",
            "https://gw.example/v1?x=1",
            "gw.example",
        ] {
            let error = config(&[("KANADE_MODEL_BASE_URL", bad)]).unwrap_err();
            assert_eq!(
                error, "KANADE_MODEL_BASE_URL must be https, or http to a loopback host",
                "{bad}"
            );
        }
        assert_eq!(
            config(&[("KANADE_MODEL_KEY", "sk-secret")]).unwrap_err(),
            "KANADE_MODEL_KEY is not read; put it in the secrets directory as `model_api_key`"
        );
        assert_eq!(
            config(&[("KANADE_CHAT_MODEL", "chat")]).unwrap_err(),
            "KANADE_CHAT_MODEL requires KANADE_MODEL_BASE_URL"
        );
        assert_eq!(
            config(&[("KANADE_MODEL_PERMITS", "17")]).unwrap_err(),
            "KANADE_MODEL_PERMITS must be between 1 and 16"
        );
        let models = config(&[
            ("KANADE_MODEL_BASE_URL", "https://gw.example"),
            ("KANADE_EXTRACT_MODEL", "extract"),
            ("KANADE_MODEL_PERMITS", "4"),
        ])
        .unwrap()
        .models;
        assert_eq!(models.extract_model.as_deref(), Some("extract"));
        assert_eq!(models.permits, 4);
        assert!(
            config(&[
                ("KANADE_MODEL_BASE_URL", "https://gw.example"),
                ("KANADE_CHAT_MODEL", "has space"),
            ])
            .is_err()
        );
    }

    #[test]
    fn tick_and_instance_are_bounded() {
        assert_eq!(
            config(&[("KANADE_TICK_SECONDS", "4")]).unwrap_err(),
            "KANADE_TICK_SECONDS must be between 5 and 300"
        );
        assert_eq!(
            config(&[("KANADE_INSTANCE_ID", "kanade-prod")])
                .unwrap()
                .instance_id,
            "kanade-prod"
        );
        assert!(config(&[("KANADE_INSTANCE_ID", "a b")]).is_err());
    }

    #[test]
    fn the_secrets_directory_is_read_by_feature_and_never_echoed() {
        let dir = std::env::temp_dir().join(format!("kanade-bot-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let at = |name: &str| dir.join(name);
        std::fs::write(at("discord_token"), "bot-token-value\n").unwrap();
        std::fs::write(at("model_api_key"), "model-key-value\n").unwrap();
        let secrets = dir.to_str().unwrap();
        let gateway = config(&[
            ("KANADE_SECRETS_DIR", secrets),
            ("KANADE_MODEL_BASE_URL", "https://gw.example"),
        ])
        .unwrap();
        let token = gateway.discord.read_token().unwrap();
        let key = gateway.models.read_key().unwrap().unwrap();
        assert_eq!(token.expose(), "bot-token-value");
        assert_eq!(key.expose(), "model-key-value");
        let debug = format!("{token:?} {key:?} {gateway:?}");
        assert!(!debug.contains("bot-token-value") && !debug.contains("model-key-value"));
        // The CA bundle is optional, used when present.
        assert_eq!(gateway.models.ca_file().unwrap(), None);
        std::fs::write(at("model_ca.pem"), "-----BEGIN CERTIFICATE-----\n").unwrap();
        assert_eq!(gateway.models.ca_file().unwrap(), Some(at("model_ca.pem")));

        // A file no feature uses is ignored: no gateway, no key read.
        let keyless = config(&[("KANADE_SECRETS_DIR", secrets)]).unwrap();
        assert_eq!(keyless.models.read_key().unwrap(), None);
        assert_eq!(keyless.models.ca_file().unwrap(), None);

        // A bad present file refuses by name; an absent required one too.
        std::fs::write(at("model_api_key"), "two\nlines-sentinel\n").unwrap();
        let error = gateway.models.read_key().unwrap_err().to_string();
        assert_eq!(
            error,
            "secrets file `model_api_key` must contain one non-empty line"
        );
        std::fs::remove_file(at("model_api_key")).unwrap();
        assert_eq!(
            gateway.models.read_key().unwrap(),
            None,
            "a keyless gateway"
        );
        std::fs::remove_file(at("discord_token")).unwrap();
        assert_eq!(
            gateway.discord.read_token().unwrap_err().to_string(),
            format!(
                "secrets file `discord_token` is missing from {secrets}: the bot signs in to Discord with it"
            )
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
