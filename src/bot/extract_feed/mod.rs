//! Gateway messages into the extraction pipeline. The handler hands
//! created/edited/deleted messages to [`MessageFeed`] without awaiting,
//! stamped with when it received them; one [`Feed`] task converts them in
//! gateway order and forwards them to the pipeline's bounded channel. Only
//! regular messages and replies are read ([`extractable`]). A message (or
//! edit) received more than [`STALE_AFTER`] after it happened is `Replay`:
//! it is cached for rescans
//! but never offered to the pipeline, so stale history makes no card (parent
//! decision). A message chat handled (and its later edits) is cached but
//! never read (`handled_by_chat`). [`DiscordHistory`] is the rescan backfill.

mod convert;
mod history;

use std::collections::{HashSet, VecDeque};
use std::future::Future;

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;
use twilight_model::id::{Id, marker::UserMarker};

use crate::bot::events::{DeletedMessages, GuildMessage};
use crate::bot::ids::id_text;
use crate::extract::pipeline::{IncomingMessage, MessageEvent, MessageOrigin};

pub use convert::{STALE_AFTER, author_kind, extractable, incoming, origin, utc};
pub use history::{DiscordHistory, snowflake_before};

/// One gateway message event, with the bot's id as of `READY` and when the
/// handler received it (a busy feed must not turn live chat into history).
#[derive(Debug)]
pub enum FeedItem {
    Posted {
        message: Box<GuildMessage>,
        self_id: Option<Id<UserMarker>>,
        /// Chat took it (answered, queued, shed or rate-limited; v4
        /// `Handling(True)`): cached as processed, never extracted.
        handled_by_chat: bool,
        received_at: DateTime<Utc>,
    },
    Edited {
        message: Box<GuildMessage>,
        self_id: Option<Id<UserMarker>>,
        received_at: DateTime<Utc>,
    },
    Deleted(DeletedMessages),
}

/// How the handler saw a message.
struct Seen {
    self_id: Option<Id<UserMarker>>,
    handled_by_chat: bool,
    received_at: DateTime<Utc>,
}

/// Chat-handled ids remembered so their later edits stay chat's.
const HANDLED_MEMORY: usize = 1_024;

/// The handler's end: never blocks the gateway.
#[derive(Clone, Debug)]
pub struct MessageFeed(mpsc::UnboundedSender<FeedItem>);

impl MessageFeed {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<FeedItem>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self(sender), receiver)
    }

    pub fn send(&self, item: FeedItem) {
        let _ = self.0.send(item);
    }
}

/// Caches a stale message for later rescans without offering it.
pub trait StaleCache: Send + Sync {
    fn cache(&self, message: IncomingMessage) -> impl Future<Output = ()> + Send;
}

pub struct Feed<C> {
    pub events: mpsc::Sender<MessageEvent>,
    pub stale: C,
}

/// Bounded, oldest forgotten first.
#[derive(Default)]
struct Handled {
    order: VecDeque<String>,
    ids: HashSet<String>,
}

impl Handled {
    fn insert(&mut self, id: String) {
        if self.ids.insert(id.clone()) {
            self.order.push_back(id);
        }
        while self.order.len() > HANDLED_MEMORY {
            if let Some(old) = self.order.pop_front() {
                self.ids.remove(&old);
            }
        }
    }
}

impl<C: StaleCache> Feed<C> {
    /// Until every [`MessageFeed`] is dropped (the gateway handler is gone)
    /// or the pipeline stops; dropping `events` then ends the pipeline.
    pub async fn run(self, mut items: mpsc::UnboundedReceiver<FeedItem>) {
        let mut handled = Handled::default();
        while let Some(item) = items.recv().await {
            let events = match item {
                FeedItem::Posted {
                    message,
                    self_id,
                    handled_by_chat,
                    received_at,
                } => {
                    if handled_by_chat {
                        handled.insert(id_text(message.message.id));
                    }
                    let seen = Seen {
                        self_id,
                        handled_by_chat,
                        received_at,
                    };
                    self.message(&message, seen, MessageEvent::Posted).await
                }
                FeedItem::Edited {
                    message,
                    self_id,
                    received_at,
                } => {
                    let seen = Seen {
                        self_id,
                        handled_by_chat: handled.ids.contains(&id_text(message.message.id)),
                        received_at,
                    };
                    self.message(&message, seen, MessageEvent::Edited).await
                }
                FeedItem::Deleted(deleted) => deleted
                    .message_ids
                    .into_iter()
                    .map(|id| MessageEvent::Deleted { id: id_text(id) })
                    .collect(),
            };
            for event in events {
                if self.events.send(event).await.is_err() {
                    return;
                }
            }
        }
    }

    async fn message(
        &self,
        message: &GuildMessage,
        seen: Seen,
        event: fn(IncomingMessage) -> MessageEvent,
    ) -> Vec<MessageEvent> {
        if !extractable(&message.message) {
            return Vec::new();
        }
        let origin = origin(&message.message, seen.received_at);
        let mut incoming = incoming(
            &message.message,
            message.origin_channel_id,
            seen.self_id,
            origin,
        );
        incoming.handled_by_chat = seen.handled_by_chat;
        if origin == MessageOrigin::Replay {
            self.stale.cache(incoming).await;
            return Vec::new();
        }
        vec![event(incoming)]
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use twilight_model::channel::Message;

    use super::*;

    #[derive(Clone, Default)]
    struct Cached(Arc<Mutex<Vec<String>>>);

    impl StaleCache for Cached {
        async fn cache(&self, message: IncomingMessage) {
            self.0.lock().unwrap().push(message.id);
        }
    }

    fn message(id: u64, at: &str, kind: u8) -> Box<GuildMessage> {
        let message: Message = serde_json::from_value(json!({
            "id": id.to_string(), "channel_id": "301", "guild_id": "900",
            "author": {"id": "1001", "username": "alice", "discriminator": "0",
                "avatar": null, "bot": false},
            "content": "nkalos amend to 10pm", "timestamp": at, "edited_timestamp": null,
            "tts": false, "mention_everyone": false, "mentions": [], "mention_roles": [],
            "attachments": [], "embeds": [], "pinned": false, "type": kind,
        }))
        .unwrap();
        Box::new(GuildMessage {
            message,
            origin_channel_id: Id::new(301),
            thread_id: None,
        })
    }

    /// Live or stale is decided by when the handler received it, however
    /// late the feed gets to it; system messages are never read.
    #[tokio::test]
    async fn freshness_is_judged_at_receipt_and_system_messages_are_dropped() {
        let (events, mut out) = mpsc::channel(8);
        let cached = Cached::default();
        let (feed, items) = MessageFeed::channel();
        let received = |secs| {
            DateTime::parse_from_rfc3339(&format!("2026-09-25T12:00:{secs:02}+00:00"))
                .unwrap()
                .with_timezone(&Utc)
        };
        let posted = |id, kind, at| FeedItem::Posted {
            message: message(id, "2026-09-25T12:00:00.000000+00:00", kind),
            self_id: None,
            handled_by_chat: false,
            received_at: at,
        };
        feed.send(posted(1, 0, received(5)));
        feed.send(posted(2, 18, received(5)));
        feed.send(FeedItem::Edited {
            message: message(3, "2025-09-25T12:00:00.000000+00:00", 19),
            self_id: None,
            received_at: received(5),
        });
        drop(feed);
        Feed {
            events,
            stale: cached.clone(),
        }
        .run(items)
        .await;
        let MessageEvent::Posted(live) = out.recv().await.unwrap() else {
            panic!("a live post");
        };
        assert_eq!((live.id.as_str(), live.origin), ("1", MessageOrigin::Live));
        assert!(out.recv().await.is_none(), "the thread notice is dropped");
        assert_eq!(*cached.0.lock().unwrap(), ["3"], "an old reply is history");
    }
}
