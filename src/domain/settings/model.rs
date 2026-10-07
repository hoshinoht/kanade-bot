//! Typed runtime settings, their code defaults and the schedule policy they
//! build. Sections follow `ConfigView` (`docs/v5/api-schemas/config.json`);
//! `schedule` and `posting` hold settings v4 read from the environment.

use std::collections::BTreeMap;

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
    pub run_lengths: RunLengths,
    pub profanity: Profanity,
    pub schedule: Schedule,
    pub posting: Posting,
}

/// The chat profanity guardrail (`v5.profanity`, user decision 2026-10-05):
/// the code-owned deny-list minus `allowed_words` plus `extra_words`, checked
/// on member questions and on finished replies; `deflection_line` is sent
/// instead. Whether an allowed word is a built-in and whether the line passes
/// the list are checked at the API (the list lives in `chat::nudge`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profanity {
    pub extra_words: Vec<String>,
    pub allowed_words: Vec<String>,
    pub check_questions: bool,
    pub check_replies: bool,
    pub deflection_line: String,
}

pub const MAX_PROFANITY_WORDS: usize = 100;
pub const PROFANITY_WORD_CHARS: std::ops::RangeInclusive<usize> = 2..=32;
pub const MAX_DEFLECTION_CHARS: usize = 200;
/// Mild and generic, in Kanade's voice; it passes the built-in list.
pub const DEFAULT_DEFLECTION_LINE: &str =
    "Ochitsuite! Let's keep it clean in here. Ask me again nicely and I'll help.";

impl Default for Profanity {
    fn default() -> Self {
        Self {
            extra_words: Vec::new(),
            allowed_words: Vec::new(),
            check_questions: true,
            check_replies: true,
            deflection_line: DEFAULT_DEFLECTION_LINE.to_owned(),
        }
    }
}

impl Profanity {
    /// The stored-form rules: each list at most [`MAX_PROFANITY_WORDS`]
    /// distinct lowercase words of letters only, [`PROFANITY_WORD_CHARS`]
    /// long, no word on both lists; the line trimmed, one line, non-empty and
    /// at most [`MAX_DEFLECTION_CHARS`].
    pub fn check(&self) -> Result<(), &'static str> {
        for words in [&self.extra_words, &self.allowed_words] {
            if words.len() > MAX_PROFANITY_WORDS {
                return Err("a word list is too long");
            }
            for (index, word) in words.iter().enumerate() {
                if !is_profanity_word(word) {
                    return Err("words are lowercase letters only");
                }
                if words[..index].contains(word) {
                    return Err("a word is listed twice");
                }
            }
        }
        if self
            .extra_words
            .iter()
            .any(|word| self.allowed_words.contains(word))
        {
            return Err("a word cannot be both extra and allowed");
        }
        let line = &self.deflection_line;
        if line.trim() != line
            || line.is_empty()
            || line.chars().count() > MAX_DEFLECTION_CHARS
            || line.chars().any(char::is_control)
        {
            return Err("the deflection line is one non-empty line of at most 200 characters");
        }
        Ok(())
    }
}

pub fn is_profanity_word(word: &str) -> bool {
    PROFANITY_WORD_CHARS.contains(&word.chars().count())
        && word.chars().all(char::is_alphabetic)
        && word.to_lowercase() == word
}

/// The length a planner uses for a run. Overrides are keyed by the catalog's
/// stable boss short key and its lowercase difficulty letter.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunLengths {
    pub default_minutes: u32,
    #[serde(default)]
    pub overrides: Vec<RunLengthOverride>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunLengthOverride {
    pub boss: String,
    pub difficulty: String,
    pub minutes: u32,
}

pub const DEFAULT_RUN_MINUTES: u32 = 30;
pub const RUN_MINUTES: std::ops::RangeInclusive<u32> = 5..=240;
pub const OVERRIDE_RUN_MINUTES: std::ops::RangeInclusive<u32> = 5..=480;

impl Default for RunLengths {
    fn default() -> Self {
        Self {
            default_minutes: DEFAULT_RUN_MINUTES,
            overrides: vec![RunLengthOverride {
                boss: "BM".into(),
                difficulty: "h".into(),
                minutes: 60,
            }],
        }
    }
}

impl RunLengths {
    /// Historic unknown tokens still receive the default so a bad row cannot
    /// erase a planner duration.
    pub fn minutes_for(
        &self,
        catalog: &crate::domain::catalog::BossTable,
        tokens: &[String],
    ) -> u32 {
        tokens
            .iter()
            .map(|token| {
                catalog
                    .split(token)
                    .and_then(|(difficulty, boss)| {
                        self.overrides
                            .iter()
                            .find(|override_| {
                                override_.boss == boss.short()
                                    && override_.difficulty == difficulty.letter()
                            })
                            .map(|override_| override_.minutes)
                    })
                    .unwrap_or(self.default_minutes)
            })
            .sum()
    }
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
    pub message_style: MessageStyle,
}

/// How the bot's Discord posts look (`v5.message_style`). Presentation only:
/// the attendance mode still decides tallies, assumed answers and pings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MessageStyle {
    /// The v4 layouts, byte for byte.
    #[default]
    Classic,
    Redesigned,
}

impl MessageStyle {
    pub const ALL: [Self; 2] = [Self::Classic, Self::Redesigned];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Redesigned => "redesigned",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|style| style.as_str() == value)
    }
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
    pub context: ContextSettings,
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
            context: ContextSettings::default(),
        }
    }
}

/// Context-window settings saved as `v5.model_context`. Defaults preserve the
/// current role output reserves while selecting a conservative local window.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContextSettings {
    pub cloud_default: u32,
    pub local_default: u32,
    pub chat: ContextRole,
    pub extraction: ContextRole,
    pub rewrite: ContextRole,
    #[serde(default)]
    pub overrides: BTreeMap<String, u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRole {
    pub reserve: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<u32>,
}

pub const MAX_CONTEXT_TOKENS: u32 = 131_072;
pub const LOCAL_CONTEXT_WARNING_TOKENS: u32 = 16_384;
/// Shown (never blocking) when a local route's effective window passes
/// [`LOCAL_CONTEXT_WARNING_TOKENS`].
pub const LOCAL_CONTEXT_WARNING: &str =
    "Context past 16k may result in degraded performance on local models.";

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            cloud_default: 65_536,
            local_default: 8_192,
            chat: ContextRole {
                reserve: 1_024,
                cap: None,
            },
            extraction: ContextRole {
                reserve: 2_500,
                cap: None,
            },
            rewrite: ContextRole {
                reserve: 96,
                cap: None,
            },
            overrides: BTreeMap::new(),
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
