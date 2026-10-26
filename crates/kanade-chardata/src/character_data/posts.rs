//! Persistent Discord posts showing Nexon data (`/character share` cards):
//! tracking a post before it is sent, and the strip pipeline the sweeper
//! runs. A due post (29 days old, or its character unlinked) is edited to
//! the data-free stub through the Discord side's [`PostDesk`]; a confirmed
//! edit or a post already gone forgets the row, a refused edit is deleted
//! instead or left `stuck` for a manual removal, and an unconfirmed one is
//! tried again 5 minutes later, then hourly. Logs carry ids and counts only.

use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::json;

use super::{CharacterData, log};
use kanade_domain::chardata::{Post, PostState, STRIP_RETRY_AGAIN, STRIP_RETRY_FIRST, Snapshot};
use kanade_domain::scheduler::StoreError;

pub type DeskFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A post with no message id younger than this is still being sent.
const CREATE_GRACE: TimeDelta = TimeDelta::minutes(2);

/// How one strip attempt ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stripped {
    /// Edited to the stub, deleted, or already gone.
    Done,
    /// Unconfirmed (ambiguous, unsent or rate limited): try again later.
    Retry,
    /// Neither edit nor delete was allowed.
    Stuck,
}

/// A search of a channel's latest messages for the bot's post with a ref.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Located {
    Found(String),
    Missing,
    /// The history could not be read.
    Unreadable,
}

/// The Discord side of tracked posts (serve: `bot::chardata::DiscordPosts`).
pub trait PostDesk: Send + Sync {
    /// Edit the message to the stub; if editing is refused, delete it.
    fn strip<'a>(&'a self, channel_id: &'a str, message_id: &'a str) -> DeskFuture<'a, Stripped>;
    /// Find the bot's own message carrying `reference` among the channel's
    /// latest 50.
    fn locate<'a>(&'a self, channel_id: &'a str, reference: &'a str) -> DeskFuture<'a, Located>;
}

/// A post about to be sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tracked {
    /// The row is written (state `live`, no message yet): send the card.
    Ready(Post, Box<Snapshot>),
    /// No active link with fresh data on an active roster character.
    NoData,
}

/// A post that needs an admin (health): `unknown`, `stuck` or `overdue`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostIssue {
    pub reference: String,
    /// `unknown`, `stuck` or `overdue`.
    pub state: &'static str,
    pub channel_id: String,
    pub message_id: Option<String>,
    pub posted_at: DateTime<Utc>,
}

/// One post's failed strips, kept in memory: a restart tries at once.
#[derive(Clone, Copy, Debug)]
pub(super) struct StripRetry {
    tries: u32,
    pub(super) next_at: DateTime<Utc>,
}

/// What one strip pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StripReport {
    pub stripped: u32,
    pub retrying: u32,
    pub stuck: u32,
    pub unknown: u32,
}

/// Six lowercase hex characters from the system's random source.
fn new_reference() -> Option<String> {
    let mut bytes = [0u8; 3];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

impl CharacterData {
    /// Let the sweeper strip posts through `desk` (serve's Discord side).
    /// Without one (no gateway) due posts wait and show in health.
    pub fn attach_posts(&self, desk: Arc<dyn PostDesk>) {
        let _ = self.desk.set(desk);
    }

    /// Write the `posts` row of a card about to be sent to `channel_id`
    /// for `character_id`, with the data it will show. The data is read
    /// again after the row is written, so an unlink in between never leaves
    /// a card that no strip covers.
    pub async fn track_post(
        &self,
        character_id: &str,
        channel_id: &str,
    ) -> Result<Tracked, StoreError> {
        if self.data(character_id).await?.is_none() {
            return Ok(Tracked::NoData);
        }
        let now = self.now();
        let post = loop {
            let reference =
                new_reference().ok_or_else(|| StoreError::Backend("no random source".into()))?;
            let post = Post::new(
                reference,
                channel_id.to_owned(),
                character_id.to_owned(),
                now,
            );
            if self.store.create_post(post.clone()).await? {
                break post;
            }
        };
        match self.data(character_id).await? {
            Some(snapshot) => Ok(Tracked::Ready(post, Box::new(snapshot))),
            None => {
                self.store.delete_post(post.reference).await?;
                Ok(Tracked::NoData)
            }
        }
    }

    /// The card was posted as `message_id`.
    pub async fn post_sent(&self, reference: &str, message_id: &str) -> Result<(), StoreError> {
        self.store
            .set_post_message(reference.to_owned(), message_id.to_owned())
            .await
            .map(drop)
    }

    /// The card was definitely not posted: forget it.
    pub async fn post_refused(&self, reference: &str) -> Result<(), StoreError> {
        self.store.delete_post(reference.to_owned()).await.map(drop)
    }

    /// The card may have been posted but was not found again: `unknown`,
    /// for an admin (health names the channel).
    pub async fn post_unknown(&self, post: &Post) -> Result<(), StoreError> {
        log(
            "WARN",
            "chardata_post_unknown",
            json!({ "ref": post.reference, "channel": post.channel_id }),
        );
        self.store
            .set_post_state(post.reference.clone(), PostState::Unknown)
            .await
            .map(drop)
    }

    /// The character's tracked posts (any state), oldest first.
    pub async fn character_posts(&self, character_id: &str) -> Result<Vec<Post>, StoreError> {
        Ok(self
            .store
            .posts()
            .await?
            .into_iter()
            .filter(|post| post.character_id == character_id)
            .collect())
    }

    /// The earliest time a failed strip is tried again, for the task.
    pub fn next_strip_retry(&self) -> Option<DateTime<Utc>> {
        self.holds()
            .strip_retries
            .values()
            .map(|retry| retry.next_at)
            .min()
    }

    /// Strip every due post that is not waiting for a retry, `unknown` or
    /// `stuck`. Nothing without a desk.
    pub async fn strip_posts(&self) -> StripReport {
        let mut report = StripReport::default();
        let Some(desk) = self.desk.get().cloned() else {
            return report;
        };
        let Ok(posts) = self.store.posts().await else {
            return report;
        };
        for post in posts {
            let now = self.now();
            if !post.is_due(now) || matches!(post.state, PostState::Unknown | PostState::Stuck) {
                continue;
            }
            let waiting = self
                .holds()
                .strip_retries
                .get(&post.reference)
                .is_some_and(|retry| retry.next_at > now);
            if waiting {
                continue;
            }
            match self.strip_one(desk.as_ref(), &post, now).await {
                Some(Stripped::Done) => report.stripped += 1,
                Some(Stripped::Retry) => report.retrying += 1,
                Some(Stripped::Stuck) => report.stuck += 1,
                None => report.unknown += 1,
            }
        }
        report
    }

    /// One due post; `None` when it turned out `unknown` (or is still
    /// being sent, counted the same: nothing to edit yet).
    async fn strip_one(
        &self,
        desk: &dyn PostDesk,
        post: &Post,
        now: DateTime<Utc>,
    ) -> Option<Stripped> {
        let message_id = match &post.message_id {
            Some(id) => id.clone(),
            None if post.posted_at > now - CREATE_GRACE => return Some(Stripped::Retry),
            None => match desk.locate(&post.channel_id, &post.reference).await {
                Located::Found(id) => {
                    if self.post_sent(&post.reference, &id).await.is_err() {
                        return Some(self.retry_later(&post.reference, now));
                    }
                    id
                }
                Located::Missing => {
                    let _ = self.post_unknown(post).await;
                    return None;
                }
                Located::Unreadable => return Some(self.retry_later(&post.reference, now)),
            },
        };
        if post.state == PostState::Live
            && self
                .store
                .set_post_state(post.reference.clone(), PostState::Stripping)
                .await
                .is_err()
        {
            return Some(self.retry_later(&post.reference, now));
        }
        let outcome = desk.strip(&post.channel_id, &message_id).await;
        match outcome {
            Stripped::Done => {
                self.holds().strip_retries.remove(&post.reference);
                if self
                    .store
                    .delete_post(post.reference.clone())
                    .await
                    .is_err()
                {
                    // Edited, but the row stays: the next pass edits again
                    // (a no-op on Discord) and forgets it then.
                    return Some(self.retry_later(&post.reference, now));
                }
                log(
                    "INFO",
                    "chardata_post_stripped",
                    json!({ "ref": post.reference }),
                );
            }
            Stripped::Retry => {
                self.retry_later(&post.reference, now);
            }
            Stripped::Stuck => {
                self.holds().strip_retries.remove(&post.reference);
                log(
                    "ERROR",
                    "chardata_post_stuck",
                    json!({
                        "ref": post.reference,
                        "channel": post.channel_id,
                        "message": message_id,
                    }),
                );
                let _ = self
                    .store
                    .set_post_state(post.reference.clone(), PostState::Stuck)
                    .await;
            }
        }
        Some(outcome)
    }

    /// 5 minutes after the first failure, then hourly.
    fn retry_later(&self, reference: &str, now: DateTime<Utc>) -> Stripped {
        let mut holds = self.holds();
        let retry = holds
            .strip_retries
            .entry(reference.to_owned())
            .or_insert(StripRetry {
                tries: 0,
                next_at: now,
            });
        retry.tries += 1;
        retry.next_at = now
            + if retry.tries == 1 {
                STRIP_RETRY_FIRST
            } else {
                STRIP_RETRY_AGAIN
            };
        log(
            "WARN",
            "chardata_post_strip_retry",
            json!({ "ref": reference, "tries": retry.tries }),
        );
        Stripped::Retry
    }
}

/// The posts health lists: unknown, stuck, and still unstripped at day 30.
pub(super) fn issues(posts: &[Post], now: DateTime<Utc>) -> Vec<PostIssue> {
    posts
        .iter()
        .filter_map(|post| {
            let state = match post.state {
                PostState::Unknown => "unknown",
                PostState::Stuck => "stuck",
                _ if post.is_overdue(now) => "overdue",
                _ => return None,
            };
            Some(PostIssue {
                reference: post.reference.clone(),
                state,
                channel_id: post.channel_id.clone(),
                message_id: post.message_id.clone(),
                posted_at: post.posted_at,
            })
        })
        .collect()
}
