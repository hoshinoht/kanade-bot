//! Every `kanade.toml` key and the environment variable it sets. The
//! documented table in `docs/v5/runtime-bootstrap.md` mirrors this list.

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
    Ints,
    /// `[[models.groups]]`, encoded as JSON.
    Groups,
    /// `[models.context]`, encoded as JSON for the runtime settings seed.
    Context,
    /// `[settings.run_lengths]`, encoded as JSON for the runtime settings seed.
    RunLengths,
}

use Kind::*;

pub(super) const KEYS: &[(&str, &str, Kind)] = &[
    ("runtime.timezone", "KANADE_TIMEZONE", Text),
    ("runtime.instance_id", "KANADE_INSTANCE_ID", Text),
    ("runtime.tick_seconds", "KANADE_TICK_SECONDS", Int),
    (
        "runtime.shutdown_timeout_seconds",
        "KANADE_SHUTDOWN_TIMEOUT_SECONDS",
        Int,
    ),
    (
        "runtime.allow_private_bind",
        "KANADE_ALLOW_PRIVATE_BIND",
        Flag,
    ),
    ("runtime.healthcheck_url", "KANADE_HEALTHCHECK_URL", Text),
    (
        "runtime.healthcheck_timeout_seconds",
        "KANADE_HEALTHCHECK_TIMEOUT_SECONDS",
        Int,
    ),
    ("admin.bind", "KANADE_ADMIN_BIND", Text),
    ("admin.host", "KANADE_ADMIN_HOST", Text),
    ("admin.trusted_proxy", "KANADE_TRUSTED_PROXY", Text),
    ("admin.edge_secret_file", "KANADE_EDGE_SECRET_FILE", Text),
    ("admin.web_dir", "KANADE_WEB_DIR", Text),
    ("admin.boss_dir", "KANADE_BOSS_DIR", Text),
    ("admin.identity_dir", "KANADE_IDENTITY_DIR", Text),
    ("admin.token_file", "KANADE_ADMIN_TOKEN_FILE", Text),
    (
        "admin.discord_client_id",
        "KANADE_ADMIN_DISCORD_CLIENT_ID",
        Id,
    ),
    (
        "admin.discord_client_secret_file",
        "KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE",
        Text,
    ),
    (
        "admin.discord_redirect_uri",
        "KANADE_ADMIN_DISCORD_REDIRECT_URI",
        Text,
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
    ("public.bind", "KANADE_PUBLIC_BIND", Text),
    ("public.host", "KANADE_PUBLIC_HOST", Text),
    ("public.cloudflared_peer", "KANADE_CLOUDFLARED_PEER", Text),
    ("discord.token_file", "KANADE_DISCORD_TOKEN_FILE", Text),
    (
        "discord.expect_v4_stopped",
        "KANADE_EXPECT_V4_STOPPED",
        Flag,
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
    ("store.db_path", "KANADE_DB_PATH", Text),
    ("store.owner_lock_dir", "KANADE_OWNER_LOCK_DIR", Text),
    ("files.catalog_file", "KANADE_CATALOG_FILE", Text),
    ("files.knowledge_dir", "KANADE_KNOWLEDGE_DIR", Text),
    ("files.persona_dir", "KANADE_PERSONA_DIR", Text),
    ("models.base_url", "KANADE_MODEL_BASE_URL", Text),
    ("models.key_file", "KANADE_MODEL_KEY_FILE", Text),
    ("models.ca_file", "KANADE_MODEL_CA_FILE", Text),
    ("models.permits", "KANADE_MODEL_PERMITS", Int),
    ("models.groups", "KANADE_MODEL_GROUPS", Groups),
    ("models.context", "KANADE_MODEL_CONTEXT", Context),
    ("models.extraction.model", "KANADE_EXTRACT_MODEL", Text),
    (
        "models.extraction.reasoning",
        "KANADE_EXTRACT_REASONING",
        Text,
    ),
    ("models.chat.model", "KANADE_CHAT_MODEL", Text),
    ("models.chat.reasoning", "KANADE_CHAT_REASONING", Text),
    ("models.rewrite.model", "KANADE_REWRITE_MODEL", Text),
    ("models.rewrite.reasoning", "KANADE_REWRITE_REASONING", Text),
    ("settings.post_channel_id", "KANADE_POST_CHANNEL_ID", Id),
    (
        "settings.watch_channel_ids",
        "KANADE_WATCH_CHANNEL_IDS",
        Ids,
    ),
    (
        "settings.watch_category_ids",
        "KANADE_WATCH_CATEGORY_IDS",
        Ids,
    ),
    (
        "settings.chat_category_ids",
        "KANADE_CHAT_CATEGORY_IDS",
        Ids,
    ),
    (
        "settings.extraction_enabled",
        "KANADE_EXTRACTION_ENABLED",
        Flag,
    ),
    ("settings.chat_enabled", "KANADE_CHAT_ENABLED", Flag),
    (
        "settings.boss_week_reset_weekday",
        "KANADE_BOSS_WEEK_RESET_WEEKDAY",
        Text,
    ),
    (
        "settings.boss_week_reset_time",
        "KANADE_BOSS_WEEK_RESET_TIME",
        Text,
    ),
    ("settings.day_of_ping_time", "KANADE_DAY_OF_PING_TIME", Text),
    (
        "settings.countdown_minutes",
        "KANADE_COUNTDOWN_MINUTES",
        Ints,
    ),
    ("settings.run_lengths", "KANADE_RUN_LENGTHS", RunLengths),
];

pub(super) fn lookup(path: &str) -> Option<(&'static str, &'static str, Kind)> {
    KEYS.iter().find(|(key, _, _)| *key == path).copied()
}

pub(super) fn is_retired(path: &str) -> bool {
    matches!(
        path,
        "models.allow_external_unmasked" | "models.pseudonymize"
    )
}

/// A table on the way to some key, e.g. `models` or `models.chat`.
pub(super) fn is_section(path: &str) -> bool {
    KEYS.iter().any(|(key, _, kind)| {
        *kind != Groups
            && key
                .strip_prefix(path)
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

/// Secrets live in files: a key naming one must be its `*_file` path.
pub(super) fn looks_secret(segment: &str) -> bool {
    let lower = segment.to_ascii_lowercase();
    ["token", "secret", "key", "password", "passwd", "credential"]
        .iter()
        .any(|word| lower.contains(word))
        && !lower.ends_with("_file")
}
