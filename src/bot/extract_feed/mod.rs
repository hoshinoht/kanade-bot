//! Gateway messages into the extraction pipeline. The handler hands
//! created/edited/deleted messages to [`MessageFeed`] without awaiting; one
//! [`Feed`] task converts them in gateway order and forwards them to the
//! pipeline's bounded channel. A message (or edit) first seen more than
//! [`STALE_AFTER`] after it happened is `Replay`: it is cached for rescans
//! but never offered to the pipeline, so stale history makes no card (parent
//! decision). A message chat handled (and its later edits) is cached but
//! never read (`handled_by_chat`). [`DiscordHistory`] is the rescan backfill.

mod convert;
mod history;

use std::collections::{HashSet, VecDeque};
use std::future::Future;
use std::sync::Arc;

use tokio::sync::mpsc;
use twilight_model::id::{Id, marker::UserMarker};

use crate::bot::events::{DeletedMessages, GuildMessage};
use crate::bot::ids::id_text;
use crate::domain::scheduler::Clock;
use crate::extract::pipeline::{IncomingMessage, MessageEvent, MessageOrigin};

pub use convert::{STALE_AFTER, author_kind, incoming, origin, utc};
pub use history::{DiscordHistory, snowflake_before};

/// One gateway message event, with the bot's id as of `READY`.
#[derive(Debug)]
pub enum FeedItem {
    Posted {
        message: Box<GuildMessage>,
        self_id: Option<Id<UserMarker>>,
        /// Chat took it (answered, queued, shed or rate-limited; v4
        /// `Handling(True)`): cached, never extracted.
        handled_by_chat: bool,
    },
    Edited(Box<GuildMessage>, Option<Id<UserMarker>>),
    Deleted(DeletedMessages),
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
    pub clock: Arc<dyn Clock + Send + Sync>,
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
                } => {
                    if handled_by_chat {
                        handled.insert(id_text(message.message.id));
                    }
                    self.message(&message, self_id, handled_by_chat, MessageEvent::Posted)
                        .await
                }
                FeedItem::Edited(message, self_id) => {
                    let chat = handled.ids.contains(&id_text(message.message.id));
                    self.message(&message, self_id, chat, MessageEvent::Edited)
                        .await
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
        self_id: Option<Id<UserMarker>>,
        handled_by_chat: bool,
        event: fn(IncomingMessage) -> MessageEvent,
    ) -> Vec<MessageEvent> {
        let origin = origin(&message.message, self.clock.now());
        let mut incoming = incoming(&message.message, message.origin_channel_id, self_id, origin);
        incoming.handled_by_chat = handled_by_chat;
        if origin == MessageOrigin::Replay {
            self.stale.cache(incoming).await;
            return Vec::new();
        }
        vec![event(incoming)]
    }
}
