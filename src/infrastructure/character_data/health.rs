//! The module's health block (informational: never part of `/healthz`),
//! shown in the admin Config environment view. Counts only.

use chrono::{DateTime, Utc};

use super::CharacterData;
use crate::domain::chardata::{CALL_WINDOW, DAILY_CALLS, LinkStatus, PostState};
use crate::infrastructure::scouter::ScouterHealth;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NexonState {
    NoKey,
    Ok,
    /// Maintenance hold until [`Health::backoff_until`].
    Backoff,
    KeyRefused,
}

impl NexonState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoKey => "no_key",
            Self::Ok => "ok",
            Self::Backoff => "backoff",
            Self::KeyRefused => "key_refused",
        }
    }
}

/// The last daily batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchReport {
    pub at: DateTime<Utc>,
    pub refreshed: u32,
    pub failed: u32,
    /// Why it stopped early (`OPENAPI00007`, `budget`, `key_refused`,
    /// `maintenance`, `no_key`, `store`).
    pub stopped: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkCounts {
    pub pending: u32,
    pub active: u32,
    pub stale: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PostCounts {
    pub due: u32,
    pub unknown: u32,
    pub stuck: u32,
    pub overdue: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Health {
    pub nexon: NexonState,
    pub backoff_until: Option<DateTime<Utc>>,
    /// `None` when the file could not be read.
    pub calls_24h: Option<u32>,
    pub budget: u32,
    pub last_batch: Option<BatchReport>,
    pub links: Option<LinkCounts>,
    pub last_sweep: Option<DateTime<Utc>>,
    pub posts: Option<PostCounts>,
    /// Posts an admin should look at: `unknown`, `stuck`, `overdue`.
    pub post_issues: Vec<super::PostIssue>,
    /// The scouter: `off` without `KANADE_SCOUTER_URL`.
    pub scouter: ScouterHealth,
}

impl CharacterData {
    pub async fn health(&self) -> Health {
        let now = self.now();
        let (nexon, backoff_until, last_batch, last_sweep) = {
            let holds = self.holds();
            let held = holds.maintenance_until.filter(|until| *until > now);
            let state = if !self.connected() {
                NexonState::NoKey
            } else if holds.key_refused {
                NexonState::KeyRefused
            } else if held.is_some() {
                NexonState::Backoff
            } else {
                NexonState::Ok
            };
            (state, held, holds.last_batch.clone(), holds.last_sweep)
        };
        let links = self.store.links().await.ok().map(|links| {
            let mut counts = LinkCounts::default();
            for link in links {
                match link.status {
                    LinkStatus::Pending => counts.pending += 1,
                    LinkStatus::Active => counts.active += 1,
                    LinkStatus::Stale => counts.stale += 1,
                }
            }
            counts
        });
        let stored = self.store.posts().await.ok();
        let posts = stored.as_ref().map(|posts| {
            let mut counts = PostCounts::default();
            for post in posts {
                counts.due += u32::from(post.is_due(now));
                counts.unknown += u32::from(post.state == PostState::Unknown);
                counts.stuck += u32::from(post.state == PostState::Stuck);
                counts.overdue += u32::from(post.is_overdue(now));
            }
            counts
        });
        let post_issues = stored
            .as_deref()
            .map(|posts| super::posts::issues(posts, now))
            .unwrap_or_default();
        Health {
            nexon,
            backoff_until,
            calls_24h: self.store.calls_since(now - CALL_WINDOW).await.ok(),
            budget: DAILY_CALLS,
            last_batch,
            links,
            last_sweep,
            posts,
            post_issues,
            scouter: match self.estimates() {
                Some(estimates) => estimates.health().await,
                None => ScouterHealth::off(),
            },
        }
    }
}
