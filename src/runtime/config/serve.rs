//! Live `serve` configuration: everything beyond the HTTP listeners.

use std::{collections::BTreeMap, time::Duration};

use super::{
    DiscordSettings, Error, FileSettings, GuildSettings, ModelSettings, RuntimeConfig,
    StoreSettings, guild, non_empty, parse_bounded_u64,
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
}

/// Initial runtime settings; applied only where the store has none yet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingSeeds {
    pub post_channel_id: Option<u64>,
    pub watch_channel_ids: Vec<u64>,
    pub pilot_channel_ids: Vec<u64>,
    pub extraction_enabled: bool,
    pub chat_enabled: bool,
}

impl ServeConfig {
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
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
        })
    }
}

impl SettingSeeds {
    fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        Ok(Self {
            post_channel_id: guild::optional(values, "KANADE_POST_CHANNEL_ID")?,
            watch_channel_ids: guild::list(values, "KANADE_WATCH_CHANNEL_IDS")?,
            pilot_channel_ids: guild::list(values, "KANADE_PILOT_CHANNEL_IDS")?,
            extraction_enabled: flag(values, "KANADE_EXTRACTION_ENABLED")?,
            chat_enabled: flag(values, "KANADE_CHAT_ENABLED")?,
        })
    }
}

fn flag(values: &BTreeMap<String, String>, key: &str) -> Result<bool, Error> {
    match non_empty(values, key) {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => Err(Error::Configuration(format!("{key} must be 0 or 1"))),
    }
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

    const BASE: [(&str, &str); 7] = [
        ("KANADE_TIMEZONE", "Asia/Kuala_Lumpur"),
        (
            "KANADE_DISCORD_TOKEN_FILE",
            "/run/secrets/kanade_discord_token",
        ),
        ("KANADE_EXPECT_V4_STOPPED", "1"),
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
    }

    #[test]
    fn live_serve_needs_the_v4_guard_and_a_token_file() {
        for guard in ["", "0", "yes"] {
            assert_eq!(
                config(&[("KANADE_EXPECT_V4_STOPPED", guard)]).unwrap_err(),
                "KANADE_EXPECT_V4_STOPPED must be 1: stop the v4 container first"
            );
        }
        assert_eq!(
            config(&[("KANADE_DISCORD_TOKEN_FILE", "")]).unwrap_err(),
            "KANADE_DISCORD_TOKEN_FILE is required"
        );
        for plain in ["KANADE_DISCORD_TOKEN", "DISCORD_TOKEN"] {
            let error = config(&[(plain, "secret-token")]).unwrap_err();
            assert_eq!(
                error,
                format!("{plain} is not read; use KANADE_DISCORD_TOKEN_FILE")
            );
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
        assert!(parsed.seeds.chat_enabled && !parsed.seeds.extraction_enabled);
        assert_eq!(
            config(&[("KANADE_PILOT_CHANNEL_IDS", "1,x")]).unwrap_err(),
            "KANADE_PILOT_CHANNEL_IDS must be comma-separated Discord snowflakes"
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
            "KANADE_MODEL_KEY is not read; use KANADE_MODEL_KEY_FILE"
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
            ("KANADE_MODEL_KEY_FILE", "/run/secrets/model"),
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
    fn loaded_secrets_never_reach_debug_output() {
        let path = std::env::temp_dir().join(format!("kanade-bot-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "bot-token-value\n").unwrap();
        let config = config(&[
            ("KANADE_DISCORD_TOKEN_FILE", path.to_str().unwrap()),
            ("KANADE_MODEL_BASE_URL", "https://gw.example"),
            ("KANADE_MODEL_KEY_FILE", path.to_str().unwrap()),
        ])
        .unwrap();
        let token = config.discord.read_token().unwrap();
        let key = config.models.read_key().unwrap().unwrap();
        assert_eq!(token.expose(), "bot-token-value");
        let debug = format!("{token:?} {key:?} {config:?}");
        assert!(!debug.contains("bot-token-value"));
        std::fs::remove_file(path).unwrap();
    }
}
