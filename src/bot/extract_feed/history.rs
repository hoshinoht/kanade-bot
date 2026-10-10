//! `rescan::History` over Discord (v4 `record_channel`): a channel's history
//! since a time, then each cached thread under it, paged 100 at a time with
//! `After` from the window's snowflake. Thread messages group under the parent.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use twilight_model::id::{Id, marker::ChannelMarker};

use serde_json::json;

use super::convert::{extractable, incoming};
use crate::bot::guild_cache::GuildCache;
use crate::bot::ids::{id_text, parse_id};
use crate::bot::transport::{DiscordTransport, HistoryPage, MAX_MESSAGES_PAGE, MessageId, Outcome};
use crate::extract::pipeline::{IncomingMessage, MessageOrigin};
use crate::extract::rescan::{Backfilled, History};
use crate::runtime::logging;

/// Discord's snowflake epoch (2015-01-01), in Unix milliseconds.
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;
/// A runaway guard: 20 000 messages per channel or thread.
const MAX_PAGES: usize = 200;

/// The smallest message id created at or after `at`, less one, so `After`
/// includes messages from `at` on.
pub fn snowflake_before(at: DateTime<Utc>) -> MessageId {
    let millis = at
        .timestamp_millis()
        .saturating_sub(DISCORD_EPOCH_MS)
        .max(0);
    let raw = u64::try_from(millis).unwrap_or(0) << 22;
    Id::new_checked(raw.saturating_sub(1)).unwrap_or(Id::new(1))
}

pub struct DiscordHistory<T> {
    pub transport: Arc<T>,
    pub cache: Arc<GuildCache>,
}

impl<T: DiscordTransport> DiscordHistory<T> {
    /// Every message in `source` since `since`, filed under `channel`;
    /// `Err` is the outcome kind of the failed page.
    async fn read(
        &self,
        source: Id<ChannelMarker>,
        channel: Id<ChannelMarker>,
        since: DateTime<Utc>,
    ) -> Result<Vec<IncomingMessage>, String> {
        let mut cursor = snowflake_before(since);
        let mut read = Vec::new();
        for _ in 0..MAX_PAGES {
            let page = match self
                .transport
                .channel_messages(source, HistoryPage::After(cursor), MAX_MESSAGES_PAGE)
                .await
            {
                Outcome::Delivered(page) => page,
                Outcome::DefinitelyRejected(kind) => return Err(format!("{kind:?}")),
                Outcome::Ambiguous(kind) => return Err(format!("{kind:?}")),
            };
            let full = page.len() >= usize::from(MAX_MESSAGES_PAGE);
            // Newest first; the next page starts after the newest seen.
            let Some(newest) = page.iter().map(|message| message.id).max() else {
                break;
            };
            read.extend(
                page.iter()
                    .filter(|message| extractable(message))
                    .map(|message| incoming(message, channel, None, MessageOrigin::Replay))
                    .filter(|message| message.created_at >= since),
            );
            if !full || newest <= cursor {
                break;
            }
            cursor = newest;
        }
        Ok(read)
    }
}

impl<T: DiscordTransport> History for DiscordHistory<T> {
    async fn backfill(&self, channel_id: &str, since: DateTime<Utc>) -> Result<Backfilled, String> {
        let Some(channel) = parse_id::<ChannelMarker>(channel_id) else {
            return Err("not a channel id".to_owned());
        };
        let mut messages = self
            .read(channel, channel, since)
            .await
            .map_err(|kind| format!("the channel history could not be read ({kind})"))?;
        let mut skipped = Vec::new();
        let threads: Vec<Id<ChannelMarker>> = self
            .cache
            .channels()
            .into_iter()
            .filter(|cached| cached.is_thread() && cached.parent_id == Some(channel))
            .map(|thread| thread.id)
            .collect();
        for thread in threads {
            // A thread the bot cannot read is skipped, as v4 did, but said.
            match self.read(thread, channel, since).await {
                Ok(more) => messages.extend(more),
                Err(kind) => {
                    logging::event(
                        "WARN",
                        "thread_history_skipped",
                        json!({"thread_id": id_text(thread), "kind": kind}),
                    );
                    skipped.push(format!(
                        "thread {} skipped: its history could not be read ({kind})",
                        id_text(thread)
                    ));
                }
            }
        }
        messages.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(Backfilled { messages, skipped })
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn the_window_snowflake_sits_just_before_its_first_millisecond() {
        let at = Utc.with_ymd_and_hms(2026, 9, 25, 12, 0, 0).unwrap();
        let first = (u64::try_from(at.timestamp_millis() - DISCORD_EPOCH_MS).unwrap()) << 22;
        assert_eq!(snowflake_before(at).get(), first - 1);
        let before_discord = Utc.with_ymd_and_hms(2010, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(snowflake_before(before_discord).get(), 1);
    }

    #[tokio::test]
    async fn an_unreadable_thread_is_skipped_and_reported() {
        use crate::bot::transport::{FakeDiscord, Op, RejectionKind, Step};
        let channel: twilight_model::channel::Channel = serde_json::from_value(json!({
            "id": "301", "type": 0, "name": "party", "guild_id": "900",
            "parent_id": null, "position": 0, "permission_overwrites": [],
        }))
        .unwrap();
        let thread: twilight_model::channel::Channel = serde_json::from_value(json!({
            "id": "302", "type": 11, "name": "tonight", "guild_id": "900",
            "parent_id": "301", "owner_id": "1001",
            "thread_metadata": {
                "archived": false, "auto_archive_duration": 1440,
                "archive_timestamp": "2026-09-25T12:00:00.000000+00:00", "locked": false,
            },
        }))
        .unwrap();
        let cache = Arc::new(GuildCache::new(Id::new(900)));
        cache.put_channel(&channel);
        cache.put_channel(&thread);
        let fake = Arc::new(FakeDiscord::new());
        fake.script(Op::ChannelMessages, Step::Succeed);
        fake.script(
            Op::ChannelMessages,
            Step::Reject(RejectionKind::MissingAccess),
        );
        let history = DiscordHistory {
            transport: fake,
            cache,
        };
        let since = Utc.with_ymd_and_hms(2026, 9, 25, 0, 0, 0).unwrap();
        let backfilled = history.backfill("301", since).await.unwrap();
        assert!(backfilled.messages.is_empty());
        assert_eq!(
            backfilled.skipped,
            ["thread 302 skipped: its history could not be read (MissingAccess)"]
        );
    }
}
