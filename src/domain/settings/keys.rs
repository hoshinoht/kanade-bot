//! `config` table keys. v4 names and encodings where v4 kept a row, so an
//! imported or hand-copied v4 row reads the same; `v5.`-prefixed keys are
//! v5-only (v4 read these from the environment or had no such setting).

pub const DAY_OF_PING_TIME: &str = "day_of_ping_time";
pub const COUNTDOWN_MINUTES: &str = "countdown_minutes";
pub const PAUSED: &str = "paused";
pub const EXTRACT_ENABLED: &str = "extract_enabled";
pub const QUIET_MODE: &str = "quiet_mode";
pub const CHAT_MODE: &str = "chat_mode";
pub const PERSONA: &str = "persona";
pub const CHAT_RATE_COUNT: &str = "chat_pilot_rate_count";
pub const CHAT_RATE_WINDOW: &str = "chat_pilot_rate_window_s";
pub const CHAT_GLOBAL_RATE_COUNT: &str = "chat_pilot_global_rate_count";
pub const CHAT_GLOBAL_RATE_WINDOW: &str = "chat_pilot_global_rate_window_s";
pub const EXTRACT_MODEL: &str = "extract_model";
pub const EXTRACT_REASONING: &str = "extract_reasoning";
pub const CHAT_MODEL: &str = "chat_pilot_model";
pub const CHAT_REASONING: &str = "chat_pilot_think";

pub const REWRITE_MODEL: &str = "v5.rewrite_model";
pub const REWRITE_REASONING: &str = "v5.rewrite_reasoning";
pub const POST_CHANNEL: &str = "v5.post_channel_id";
pub const WATCHED_CHANNELS: &str = "v5.watched_channel_ids";
pub const WATCHED_CATEGORIES: &str = "v5.watched_category_ids";
pub const CHAT_CATEGORIES: &str = "v5.chat_category_ids";
pub const RESET_WEEKDAY: &str = "v5.reset_weekday";
pub const RESET_TIME: &str = "v5.reset_time";
pub const ATTENDANCE_MODE: &str = "v5.attendance_mode";
pub const SELF_SERVICE_MODE: &str = "v5.self_service_mode";
pub const PUBLIC_PORTAL: &str = "v5.public_portal";
pub const PROFILE_VISIBILITY: &str = "v5.profile_visibility";
pub const ROLE_PROFILES: &str = "v5.role_profiles";

/// Every settings key. Other `config` rows (the digest marker, v4
/// bookkeeping) are never read or written through the settings port.
pub const ALL: [&str; 28] = [
    DAY_OF_PING_TIME,
    COUNTDOWN_MINUTES,
    PAUSED,
    EXTRACT_ENABLED,
    QUIET_MODE,
    CHAT_MODE,
    PERSONA,
    CHAT_RATE_COUNT,
    CHAT_RATE_WINDOW,
    CHAT_GLOBAL_RATE_COUNT,
    CHAT_GLOBAL_RATE_WINDOW,
    EXTRACT_MODEL,
    EXTRACT_REASONING,
    CHAT_MODEL,
    CHAT_REASONING,
    REWRITE_MODEL,
    REWRITE_REASONING,
    POST_CHANNEL,
    WATCHED_CHANNELS,
    WATCHED_CATEGORIES,
    CHAT_CATEGORIES,
    RESET_WEEKDAY,
    RESET_TIME,
    ATTENDANCE_MODE,
    SELF_SERVICE_MODE,
    PUBLIC_PORTAL,
    PROFILE_VISIBILITY,
    ROLE_PROFILES,
];

pub fn is_setting(key: &str) -> bool {
    ALL.contains(&key)
}
