//! Rescan and backfill jobs (v4 `bot/agent/rescan.py` and
//! `Pipeline.rescan_window`): requests queue, one job runs at a time, its
//! channels one after another, and each channel's window is backfilled,
//! cut into conversations, read at the backlog's fixed pace and proposed
//! once. Jobs persist in `rescan_jobs`; cancellation lands between bursts.

mod jobs;
mod read;

use std::fmt;
use std::future::Future;

use chrono::{DateTime, Utc};

pub use jobs::{INTERRUPTED, JobView, KEEP_JOBS, Rescans, SWITCHED_OFF};

use crate::domain::scheduler::StoreError;
use crate::extract::pipeline::IncomingMessage;
use crate::extract::window::{WindowError, clamp_window};

/// Admin API spellings (`docs/v5/admin-api.md`), mapped onto v4's windows.
pub const API_WINDOWS: [&str; 3] = ["week", "since_reset", "two_weeks"];

/// Attempts per burst while the governor turns the rescan away.
pub const TURNED_AWAY_ATTEMPTS: usize = 3;

/// What a job reads: a v4 window and whether an empty `week` may widen to
/// the boss week before (v4 `should_widen`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedWindow {
    pub key: &'static str,
    pub may_widen: bool,
}

/// `week` is v4's (widens once when empty); `since_reset` is the same start
/// without widening; `two_weeks` is v4 `2weeks`. v4 spellings (`2weeks`,
/// `48h`, `24h`) still work; automated jobs are capped at `48h`.
///
/// # Errors
/// [`WindowError::Unknown`] for anything else.
pub fn resolve_window(window: &str, automated: bool) -> Result<ResolvedWindow, WindowError> {
    let (v4, may_widen) = match window {
        "since_reset" => ("week", false),
        "two_weeks" => ("2weeks", true),
        other => (other, true),
    };
    let key = clamp_window(v4, automated).map_err(|error| match error {
        WindowError::Unknown { .. } => WindowError::Unknown {
            window: window.to_owned(),
        },
        other => other,
    })?;
    Ok(ResolvedWindow {
        key,
        may_widen: may_widen && !automated,
    })
}

/// What a backfill read, and the sources (threads) it had to skip, each
/// counted in the job's errors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Backfilled {
    pub messages: Vec<IncomingMessage>,
    pub skipped: Vec<String>,
}

/// Pulls a channel's history from Discord (v4 `backfill_channel`); the
/// rescan caches what comes back. `Err`: the channel itself was unreadable.
pub trait History: Send + Sync {
    fn backfill(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
    ) -> impl Future<Output = Result<Backfilled, String>> + Send;
}

/// A rescan request (v4 `RescanWorker.submit`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RescanRequest {
    pub channels: Vec<String>,
    pub window: String,
    /// `manual`, `portal`, `startup`…, stored as given.
    pub source: String,
    pub automated: bool,
    pub requested_by: Option<String>,
    /// Read only messages no pass has read yet (the startup rescan, so a
    /// restart never re-proposes or reposts cards). Manual rescans re-read.
    pub unprocessed_only: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RescanError {
    NoChannels,
    Window(WindowError),
    /// The worker is shutting down.
    Closed,
    Store(StoreError),
}

impl fmt::Display for RescanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoChannels => f.write_str("no channels to rescan"),
            Self::Window(error) => error.fmt(f),
            Self::Closed => f.write_str("rescans are shutting down"),
            Self::Store(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RescanError {}

impl From<StoreError> for RescanError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_windows_map_onto_v4_windows() {
        let resolved = |window, automated| resolve_window(window, automated).expect("known");
        assert_eq!(
            resolved("week", false),
            ResolvedWindow {
                key: "week",
                may_widen: true
            }
        );
        assert_eq!(
            resolved("since_reset", false),
            ResolvedWindow {
                key: "week",
                may_widen: false
            }
        );
        assert_eq!(resolved("two_weeks", false).key, "2weeks");
        assert_eq!(resolved("24h", false).key, "24h");
        // Automated rescans never read more than 48 h and never widen.
        assert_eq!(
            resolved("two_weeks", true),
            ResolvedWindow {
                key: "48h",
                may_widen: false
            }
        );
        assert_eq!(resolved("24h", true).key, "24h");
        assert!(matches!(
            resolve_window("month", false),
            Err(WindowError::Unknown { window }) if window == "month"
        ));
    }
}
