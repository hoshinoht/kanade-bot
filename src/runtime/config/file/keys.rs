//! Every `kanade.toml` key and the environment variable it sets, and every
//! removed key or variable with its new home. The documented table in
//! `docs/v5/runtime-bootstrap.md` mirrors [`KEYS`] (checked by a test).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Text,
    /// Non-negative integer.
    Int,
    /// `true`/`false` → `1`/`0`.
    Flag,
    /// Snowflake: string or positive integer.
    Id,
    Ids,
    /// List of strings without commas.
    Texts,
    /// `[[models.groups]]`, encoded as JSON.
    Groups,
    /// `[models.capabilities]` (alias → capability table), encoded as JSON;
    /// values are checked there.
    Capabilities,
}

use Kind::*;

pub(in crate::runtime::config) const KEYS: &[(&str, &str, Kind)] = &[
    ("runtime.timezone", "KANADE_TIMEZONE", Text),
    ("runtime.instance_id", "KANADE_INSTANCE_ID", Text),
    ("runtime.tick_seconds", "KANADE_TICK_SECONDS", Int),
    (
        "runtime.shutdown_timeout_seconds",
        "KANADE_SHUTDOWN_TIMEOUT_SECONDS",
        Int,
    ),
    (
        "runtime.healthcheck_timeout_seconds",
        "KANADE_HEALTHCHECK_TIMEOUT_SECONDS",
        Int,
    ),
    (
        "runtime.boss_week_reset_weekday",
        "KANADE_BOSS_WEEK_RESET_WEEKDAY",
        Text,
    ),
    (
        "runtime.boss_week_reset_time",
        "KANADE_BOSS_WEEK_RESET_TIME",
        Text,
    ),
    ("discord.gateway", "KANADE_DISCORD_GATEWAY", Flag),
    ("discord.guild_id", "KANADE_GUILD_ID", Id),
    ("discord.bossing_role_id", "KANADE_BOSSING_ROLE_ID", Id),
    ("discord.admin_role_id", "KANADE_ADMIN_ROLE_ID", Id),
    (
        "discord.chat_pilot_role_id",
        "KANADE_CHAT_PILOT_ROLE_ID",
        Id,
    ),
    ("discord.debug_user_ids", "KANADE_DEBUG_USER_IDS", Ids),
    ("discord.test_channel", "KANADE_TEST_CHANNEL_ID", Id),
    ("admin.host", "KANADE_ADMIN_HOST", Text),
    (
        "admin.discord_client_id",
        "KANADE_ADMIN_DISCORD_CLIENT_ID",
        Id,
    ),
    (
        "admin.tailscale_logins",
        "KANADE_ADMIN_TAILSCALE_LOGINS",
        Texts,
    ),
    (
        "admin.session_idle_minutes",
        "KANADE_ADMIN_SESSION_IDLE_MINUTES",
        Int,
    ),
    (
        "admin.session_absolute_hours",
        "KANADE_ADMIN_SESSION_ABSOLUTE_HOURS",
        Int,
    ),
    ("public.host", "KANADE_PUBLIC_HOST", Text),
    (
        "public.discord_client_id",
        "KANADE_PUBLIC_DISCORD_CLIENT_ID",
        Id,
    ),
    (
        "public.session_idle_minutes",
        "KANADE_PUBLIC_SESSION_IDLE_MINUTES",
        Int,
    ),
    (
        "public.session_absolute_hours",
        "KANADE_PUBLIC_SESSION_ABSOLUTE_HOURS",
        Int,
    ),
    (
        "public.fresh_write_minutes",
        "KANADE_PUBLIC_FRESH_WRITE_MINUTES",
        Int,
    ),
    ("models.base_url", "KANADE_MODEL_BASE_URL", Text),
    ("models.permits", "KANADE_MODEL_PERMITS", Int),
    ("models.groups", "KANADE_MODEL_GROUPS", Groups),
    (
        "models.capabilities",
        "KANADE_MODEL_CAPABILITIES",
        Capabilities,
    ),
    ("modules.roster", "KANADE_MODULE_ROSTER", Flag),
    (
        "modules.character_data",
        "KANADE_MODULE_CHARACTER_DATA",
        Flag,
    ),
    ("modules.guides", "KANADE_MODULE_GUIDES", Flag),
    ("character_data.scouter_url", "KANADE_SCOUTER_URL", Text),
];

const IMAGE: &str = "the image sets it";
const COMPOSE: &str = "Compose sets it";

/// Removed `kanade.toml` keys (dotted) and environment variables
/// (`KANADE_*`), each with where its value lives now. Both refuse startup by
/// name, never echoing the value.
pub(in crate::runtime::config) const RETIRED: &[(&str, &str)] = &[
    // Paths inside the image (Dockerfile `ENV`).
    ("admin.web_dir", IMAGE),
    ("admin.boss_dir", IMAGE),
    ("admin.identity_dir", IMAGE),
    ("store.db_path", IMAGE),
    ("store.owner_lock_dir", IMAGE),
    ("files.catalog_file", IMAGE),
    ("files.knowledge_dir", IMAGE),
    ("files.persona_dir", IMAGE),
    ("character_data.db_path", IMAGE),
    // Network addresses (deploy/compose.yaml and its override).
    ("admin.bind", COMPOSE),
    ("runtime.allow_private_bind", COMPOSE),
    (
        "admin.trusted_proxy",
        "set KANADE_TRUSTED_PROXY in compose.override.yaml",
    ),
    (
        "public.bind",
        "Compose sets it, and `public.host` turns the public listener on",
    ),
    ("public.cloudflared_peer", COMPOSE),
    // Computed.
    (
        "runtime.healthcheck_url",
        "the healthcheck targets the admin listener (or `kanade healthcheck --url`)",
    ),
    (
        "KANADE_HEALTHCHECK_URL",
        "the healthcheck targets KANADE_ADMIN_BIND (or `kanade healthcheck --url`)",
    ),
    (
        "admin.discord_redirect_uri",
        "it is https://<admin.host>/api/admin/auth/discord/callback",
    ),
    (
        "KANADE_ADMIN_DISCORD_REDIRECT_URI",
        "it is https://<admin.host>/api/admin/auth/discord/callback",
    ),
    (
        "public.discord_redirect_uri",
        "it is https://<public.host>/api/public/auth/discord/callback",
    ),
    (
        "KANADE_PUBLIC_DISCORD_REDIRECT_URI",
        "it is https://<public.host>/api/public/auth/discord/callback",
    ),
    // Files in the secrets directory.
    (
        "discord.token_file",
        "put the token in the secrets directory as `discord_token`",
    ),
    (
        "KANADE_DISCORD_TOKEN_FILE",
        "put the token in the secrets directory as `discord_token`",
    ),
    (
        "admin.token_file",
        "put the token in the secrets directory as `admin_token`",
    ),
    (
        "KANADE_ADMIN_TOKEN_FILE",
        "put the token in the secrets directory as `admin_token`",
    ),
    (
        "models.key_file",
        "put the key in the secrets directory as `model_api_key`",
    ),
    (
        "KANADE_MODEL_KEY_FILE",
        "put the key in the secrets directory as `model_api_key`",
    ),
    (
        "models.ca_file",
        "put the CA bundle in the secrets directory as `model_ca.pem`",
    ),
    (
        "KANADE_MODEL_CA_FILE",
        "put the CA bundle in the secrets directory as `model_ca.pem`",
    ),
    (
        "admin.discord_client_secret_file",
        "put the secret in the secrets directory as `discord_client_secret`",
    ),
    (
        "KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE",
        "put the secret in the secrets directory as `discord_client_secret`",
    ),
    (
        "public.discord_client_secret_file",
        "put the secret in the secrets directory as `public_discord_client_secret`",
    ),
    (
        "KANADE_PUBLIC_DISCORD_CLIENT_SECRET_FILE",
        "put the secret in the secrets directory as `public_discord_client_secret`",
    ),
    (
        "admin.edge_secret_file",
        "put the secret in the secrets directory as `edge_secret`",
    ),
    (
        "KANADE_EDGE_SECRET_FILE",
        "put the secret in the secrets directory as `edge_secret`",
    ),
    (
        "character_data.nexon_key_file",
        "put the key in the secrets directory as `nexon_api_key`",
    ),
    (
        "KANADE_NEXON_API_KEY_FILE",
        "put the key in the secrets directory as `nexon_api_key`",
    ),
    (
        "backup.recipients_file",
        "put the age public keys in the backups directory as `recipients`",
    ),
    (
        "KANADE_BACKUP_RECIPIENTS_FILE",
        "put the age public keys in the backups directory as `recipients`",
    ),
    // Live settings: Config owns them (saved rows are kept).
    (
        "settings.post_channel_id",
        "set it in Config › Weekly digest",
    ),
    ("settings.watch_channel_ids", "set it in Config › Channels"),
    ("settings.watch_category_ids", "set it in Config › Channels"),
    ("settings.chat_category_ids", "set it in Config › Channels"),
    (
        "settings.extraction_enabled",
        "set it in Config › Chat watching",
    ),
    ("settings.chat_enabled", "set it in Config › Chatbot"),
    ("settings.day_of_ping_time", "set it in Config › Pings"),
    ("settings.countdown_minutes", "set it in Config › Pings"),
    ("settings.run_lengths", "set it in Config › Run lengths"),
    ("models.extraction.model", "set it in Config › Models"),
    ("models.extraction.reasoning", "set it in Config › Models"),
    ("models.chat.model", "set it in Config › Models"),
    ("models.chat.reasoning", "set it in Config › Models"),
    ("models.rewrite.model", "set it in Config › Models"),
    ("models.rewrite.reasoning", "set it in Config › Models"),
    ("models.context", "set it in Config › Models"),
    // Moved.
    (
        "settings.boss_week_reset_weekday",
        "it moved to `runtime.boss_week_reset_weekday`",
    ),
    (
        "settings.boss_week_reset_time",
        "it moved to `runtime.boss_week_reset_time`",
    ),
    // Model calls are never masked.
    (
        "models.pseudonymize",
        "remove it; model calls are never masked",
    ),
    (
        "models.allow_external_unmasked",
        "remove it; model calls are never masked",
    ),
];

pub(super) fn lookup(path: &str) -> Option<(&'static str, &'static str, Kind)> {
    KEYS.iter().find(|(key, _, _)| *key == path).copied()
}

/// The key that sets `env`, if any.
pub(super) fn key_of(env: &str) -> Option<&'static str> {
    KEYS.iter()
        .find(|(_, name, _)| *name == env)
        .map(|(key, _, _)| *key)
}

/// The refusal for a removed key (dotted) or variable (`KANADE_*`).
pub(super) fn retired(name: &str) -> Option<String> {
    let (_, home) = RETIRED.iter().find(|(old, _)| *old == name)?;
    Some(if name.starts_with("KANADE_") {
        format!("{name} was removed: {home}")
    } else {
        format!("kanade.toml `{name}` was removed: {home}")
    })
}

/// A table on the way to some key or removed key, e.g. `models` or
/// `settings`.
pub(super) fn is_section(path: &str) -> bool {
    let below = |key: &str| {
        key.strip_prefix(path)
            .is_some_and(|rest| rest.starts_with('.'))
    };
    KEYS.iter()
        .any(|(key, _, kind)| *kind != Groups && below(key))
        || RETIRED.iter().any(|(old, _)| below(old))
}

/// Secrets live in the secrets directory, never in this file.
pub(super) fn looks_secret(segment: &str) -> bool {
    let lower = segment.to_ascii_lowercase();
    ["token", "secret", "key", "password", "passwd", "credential"]
        .iter()
        .any(|word| lower.contains(word))
}
