//! Extraction tuning (v4 `EXTRACT_*` settings) and the startup check for a
//! reasoning effort the extraction model does not publish.

use std::fmt;
use std::time::Duration;

use chrono::{NaiveTime, TimeDelta, Weekday};
use chrono_tz::Tz;

use crate::infrastructure::llm::{Effort, ModelCapabilities};

/// v4 `EXTRACT_DEBOUNCE_SECONDS`.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_secs(90);
/// v4 `EXTRACT_CONTEXT_MESSAGES`.
pub const DEFAULT_CONTEXT_MESSAGES: usize = 25;
/// v4 `EXTRACT_MIN_CONFIDENCE`.
pub const DEFAULT_MIN_CONFIDENCE: f64 = 0.6;
/// v4 `MODEL_CONTEXT_TOKENS`.
pub const DEFAULT_CONTEXT_TOKENS: usize = 8192;
/// v4 `KANATA_TIMEOUT`: one extraction session, answer retry included.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a burst may queue for an extraction permit behind chat.
pub const DEFAULT_PERMIT_WAIT: Duration = Duration::from_secs(120);
/// Backlog: one burst per interval, so a replay never storms the model.
pub const DEFAULT_DRAIN_INTERVAL: Duration = Duration::from_secs(15);
/// Backlog bound in message ids; the oldest are dropped (and reported) past it.
pub const DEFAULT_BACKLOG_CAPACITY: usize = 1_000;
/// How far back a burst's context may reach (v4 `CONTEXT_WINDOW_HOURS`).
pub const CONTEXT_WINDOW: TimeDelta = TimeDelta::hours(48);
/// "Was this channel just talking about scheduling" (v4 `_recent_scheduling`).
pub const RECENT_SCHEDULING: TimeDelta = TimeDelta::hours(6);

/// Everything the pipeline is tuned by; the guild's zone and reset come from
/// the scheduler policy.
#[derive(Clone, Debug, PartialEq)]
pub struct PipelineConfig {
    pub zone: Tz,
    pub reset_weekday: Weekday,
    pub reset_time: NaiveTime,
    pub debounce: Duration,
    pub context_messages: usize,
    pub min_confidence: f64,
    pub context_tokens: usize,
    /// The extraction role's configured effort (`None` sends none).
    pub reasoning: Option<Effort>,
    pub permit_wait: Duration,
    pub call_timeout: Duration,
    pub drain_interval: Duration,
    pub backlog_capacity: usize,
}

impl PipelineConfig {
    /// v4 defaults for a guild in `zone` resetting at `reset_weekday` `reset_time`.
    pub fn new(zone: Tz, reset_weekday: Weekday, reset_time: NaiveTime) -> Self {
        Self {
            zone,
            reset_weekday,
            reset_time,
            debounce: DEFAULT_DEBOUNCE,
            context_messages: DEFAULT_CONTEXT_MESSAGES,
            min_confidence: DEFAULT_MIN_CONFIDENCE,
            context_tokens: DEFAULT_CONTEXT_TOKENS,
            reasoning: None,
            permit_wait: DEFAULT_PERMIT_WAIT,
            call_timeout: DEFAULT_CALL_TIMEOUT,
            drain_interval: DEFAULT_DRAIN_INTERVAL,
            backlog_capacity: DEFAULT_BACKLOG_CAPACITY,
        }
    }
}

/// The configured extraction effort is one the model does not publish, so
/// every call would be refused before sending (`D-SHAPING`: refused, never
/// silently dropped).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnpublishedEffort {
    pub alias: String,
    pub effort: Effort,
}

impl fmt::Display for UnpublishedEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "extraction reasoning {} is not published by {}",
            self.effort.as_str(),
            self.alias
        )
    }
}

impl std::error::Error for UnpublishedEffort {}

/// Startup/config-change check mirroring the runner's shaping rule: an effort
/// other than `off` on a model with reasoning control and a published list
/// must be in that list.
// TODO(config slice): run at startup and on every extraction alias/effort
// change once runtime config resolves capabilities from `/v1/models`.
pub fn check_reasoning_effort(
    alias: &str,
    effort: Option<Effort>,
    capabilities: &ModelCapabilities,
) -> Result<(), UnpublishedEffort> {
    let Some(effort) = effort.filter(|effort| *effort != Effort::Off) else {
        return Ok(());
    };
    let refused = capabilities.reasoning_control
        && capabilities
            .reasoning_efforts
            .as_ref()
            .is_some_and(|published| !published.contains(&effort));
    if refused {
        return Err(UnpublishedEffort {
            alias: alias.to_owned(),
            effort,
        });
    }
    Ok(())
}
