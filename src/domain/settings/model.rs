//! Typed runtime settings, their code defaults and the schedule policy they
//! build. Sections follow `ConfigView` (`docs/v5/api-schemas/config.json`);
//! `schedule` and `posting` hold settings v4 read from the environment.

use chrono::{NaiveTime, Weekday};
use chrono_tz::Tz;

use crate::domain::attendance::{AttendanceMode, AttendancePolicy};
use crate::domain::schedule::{ReminderPolicy, SchedulePolicy};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuntimeSettings {
    pub pings: Pings,
    pub watching: Watching,
    pub chatbot: Chatbot,
    pub notifications: Notifications,
    pub self_service: SelfService,
    pub persona: Persona,
    pub models: Models,
    pub schedule: Schedule,
    pub posting: Posting,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pings {
    pub day_of_ping_time: NaiveTime,
    /// Largest first, no duplicates (v4 order).
    pub countdown_minutes: Vec<u32>,
}

impl Default for Pings {
    fn default() -> Self {
        Self {
            day_of_ping_time: NaiveTime::from_hms_opt(1, 0, 0).unwrap_or(NaiveTime::MIN),
            countdown_minutes: vec![60],
        }
    }
}

/// Extractor switches and watched channels (Discord ids).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Watching {
    pub paused: bool,
    pub extract_enabled: bool,
    pub channel_ids: Vec<String>,
    pub category_ids: Vec<String>,
}

impl Default for Watching {
    fn default() -> Self {
        Self {
            paused: false,
            extract_enabled: true,
            channel_ids: Vec::new(),
            category_ids: Vec::new(),
        }
    }
}

/// `enabled` is v4's `chat_mode` kill switch; the chatbot answers in every
/// channel of its categories (no per-channel list, user decision).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chatbot {
    pub enabled: bool,
    pub category_ids: Vec<String>,
    pub member_rate: Rate,
    pub guild_rate: Rate,
}

impl Default for Chatbot {
    fn default() -> Self {
        Self {
            enabled: false,
            category_ids: Vec::new(),
            member_rate: Rate {
                count: 4,
                window_s: 300,
            },
            guild_rate: Rate {
                count: 12,
                window_s: 900,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rate {
    pub count: u32,
    pub window_s: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Notifications {
    pub quiet_mode: bool,
}

/// Mirrors `extract::redirect::SelfServiceMode` (the domain cannot depend on
/// `extract`); the stored text is the same.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelfServiceMode {
    #[default]
    CardsAndLink,
    LinkFirst,
    CardsOnly,
}

impl SelfServiceMode {
    pub const ALL: [Self; 3] = [Self::CardsAndLink, Self::LinkFirst, Self::CardsOnly];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::CardsAndLink => "cards_and_link",
            Self::LinkFirst => "link_first",
            Self::CardsOnly => "cards_only",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == value)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelfService {
    pub mode: SelfServiceMode,
    /// Closed by default: cards only until the public launch.
    pub public_portal: bool,
}

impl SelfService {
    pub fn effective_mode(&self) -> SelfServiceMode {
        if self.public_portal {
            self.mode
        } else {
            SelfServiceMode::CardsOnly
        }
    }
}

/// The selected persona id (v4 `persona`); `""` is none selected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Persona {
    pub active: String,
    /// Ordered readable profiles members may choose; missing means private.
    pub profile_visibility: Vec<String>,
    /// Ordered role-to-profile overrides; role assignments do not grant chat access.
    pub role_profiles: Vec<RoleProfileAssignment>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoleProfileAssignment {
    pub role_id: String,
    pub profile: String,
}

pub const MAX_ROLE_PROFILE_ASSIGNMENTS: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Models {
    pub extraction: RoleModel,
    pub chat: RoleModel,
    pub rewrite: RoleModel,
}

impl Default for Models {
    fn default() -> Self {
        Self {
            extraction: RoleModel {
                alias: None,
                reasoning: Reasoning::Off,
            },
            chat: RoleModel::default(),
            rewrite: RoleModel::default(),
        }
    }
}

/// `alias` `None` is unset (no code default: required while the role is on).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoleModel {
    pub alias: Option<String>,
    pub reasoning: Reasoning,
}

/// A stored reasoning level. Whether the alias publishes it is checked by
/// the config API against the live catalog, not here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reasoning {
    /// `""`: the extraction role's effort (chat and rewrite only).
    #[default]
    Inherit,
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Reasoning {
    const LEVELS: [Self; 8] = [
        Self::Inherit,
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "",
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// v4 `normalize_reasoning`: trimmed, case-insensitive, `false`/`none`
    /// read as `off`.
    pub fn parse(value: &str) -> Option<Self> {
        let key = value.trim().to_ascii_lowercase();
        let key = match key.as_str() {
            "false" | "none" => "off",
            other => other,
        };
        Self::LEVELS.into_iter().find(|level| level.as_str() == key)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub reset_weekday: Weekday,
    pub reset_time: NaiveTime,
    pub attendance: AttendanceMode,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            reset_weekday: Weekday::Thu,
            reset_time: NaiveTime::MIN,
            attendance: AttendanceMode::V4Compat,
        }
    }
}

/// Guild-wide posts (the weekly digest) and the home-channel fallback.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Posting {
    pub channel_id: Option<String>,
}

impl RuntimeSettings {
    /// The schedule policy in the guild `zone` (environment-only).
    pub fn schedule_policy(&self, zone: Tz) -> SchedulePolicy {
        SchedulePolicy::new(
            ReminderPolicy {
                zone,
                ping_time: self.pings.day_of_ping_time,
                countdowns: self.pings.countdown_minutes.clone(),
            },
            self.schedule.reset_weekday,
            self.schedule.reset_time,
        )
        .with_attendance(AttendancePolicy {
            mode: self.schedule.attendance,
            unknown_window: AttendancePolicy::DEFAULT_UNKNOWN_WINDOW,
        })
    }
}
