//! The serve [`EventHandler`]: fans routed events out to the roster task,
//! the reaction worker, spawned interaction and registration tasks, the chat
//! feed and message counters. It never awaits Discord I/O inline.

mod reactions;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use twilight_model::application::interaction::{Interaction, InteractionData};
use twilight_model::id::{
    Id,
    marker::{ApplicationMarker, GuildMarker, UserMarker},
};

use crate::bot::chat_feed::ChatFeed;
use crate::bot::commands::{Dispatcher, Disposition, spawn_interaction};
use crate::bot::events::{BotEvent, EventHandler, RsvpReaction, rsvp_reaction};
use crate::bot::extract_feed::{FeedItem, MessageFeed};
use crate::bot::roster::RosterJob;
use crate::bot::transport::{DiscordTransport, Outcome};
use crate::domain::members::Directory;
use crate::runtime::logging;

pub use reactions::{Reacted, Reactions};

/// Guild messages seen (created, updated, deleted).
#[derive(Clone, Debug, Default)]
pub struct MessageCounts(Arc<[AtomicU64; 3]>);

impl MessageCounts {
    /// `(created, updated, deleted)`.
    pub fn get(&self) -> (u64, u64, u64) {
        let [created, updated, deleted] = &*self.0;
        (
            created.load(Ordering::Relaxed),
            updated.load(Ordering::Relaxed),
            deleted.load(Ordering::Relaxed),
        )
    }

    fn bump(&self, index: usize, by: u64) {
        self.0[index].fetch_add(by, Ordering::Relaxed);
    }
}

pub type OwnerFn = Arc<dyn Fn() -> Option<Id<UserMarker>> + Send + Sync>;

/// Builds the command dispatcher from the bot's own name (from `READY`).
pub type CommandsFn = Box<dyn FnMut(Option<String>) -> Arc<Dispatcher> + Send>;

/// Everything the fan-out hands work to.
pub struct Fanout<T> {
    pub guild: Id<GuildMarker>,
    pub transport: Arc<T>,
    commands: CommandsFn,
    /// Built on the first `READY`; kept across reconnects (its redelivery
    /// dedupe too).
    dispatcher: Option<Arc<Dispatcher>>,
    /// The guild owner as last recorded (staff gate for commands).
    pub owner: OwnerFn,
    pub directory: Arc<dyn Directory + Send + Sync>,
    pub roster: mpsc::UnboundedSender<RosterJob>,
    pub reactions: mpsc::UnboundedSender<RsvpReaction>,
    /// Called on every `READY` with the application id (before any
    /// interaction or registration uses the transport).
    pub on_ready: Box<dyn FnMut(Id<ApplicationMarker>) + Send>,
    /// Set once the guild first became available (the tick waits for it).
    pub guild_ready: watch::Sender<bool>,
    pub messages: MessageCounts,
    /// The chat pilot's input; `None` leaves chat off.
    pub chat: Option<ChatFeed>,
    /// Extraction's message feed; `None` counts messages only.
    feed: Option<MessageFeed>,
    self_id: Option<Id<UserMarker>>,
    /// A `READY` arrived and its guild has not been synced yet.
    sync_pending: bool,
    tasks: Vec<JoinHandle<()>>,
}

impl<T: DiscordTransport + 'static> Fanout<T> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        guild: Id<GuildMarker>,
        transport: Arc<T>,
        commands: CommandsFn,
        owner: OwnerFn,
        directory: Arc<dyn Directory + Send + Sync>,
        roster: mpsc::UnboundedSender<RosterJob>,
        reactions: mpsc::UnboundedSender<RsvpReaction>,
        on_ready: Box<dyn FnMut(Id<ApplicationMarker>) + Send>,
        guild_ready: watch::Sender<bool>,
    ) -> Self {
        Self {
            guild,
            transport,
            commands,
            dispatcher: None,
            owner,
            directory,
            roster,
            reactions,
            on_ready,
            guild_ready,
            messages: MessageCounts::default(),
            chat: None,
            feed: None,
            self_id: None,
            sync_pending: false,
            tasks: Vec::new(),
        }
    }

    /// Forward guild messages to extraction.
    #[must_use]
    pub fn with_feed(mut self, feed: Option<MessageFeed>) -> Self {
        self.feed = feed;
        self
    }

    fn forward(&self, item: FeedItem) {
        if let Some(feed) = &self.feed {
            feed.send(item);
        }
    }

    /// Wait for spawned interaction and registration tasks (after the
    /// gateway stopped), so nothing still holds the store.
    pub async fn finish(&mut self) {
        for task in self.tasks.drain(..) {
            let _ = task.await;
        }
    }

    fn spawn(&mut self, task: impl Future<Output = ()> + Send + 'static) {
        self.tasks.retain(|task| !task.is_finished());
        self.tasks.push(tokio::spawn(task));
    }

    /// Guild commands only, as one bulk overwrite; never global.
    fn register_commands(&mut self) {
        let Some(dispatcher) = &self.dispatcher else {
            return;
        };
        let commands = dispatcher.definitions();
        let transport = Arc::clone(&self.transport);
        let guild = self.guild;
        self.spawn(async move {
            let outcome = transport.register_guild_commands(guild, &commands).await;
            let names: Vec<String> = commands.into_iter().map(|command| command.name).collect();
            if outcome.is_delivered() {
                logging::event("INFO", "commands_registered", json!({"commands": names}));
            } else {
                logging::event(
                    "WARN",
                    "commands_register_failed",
                    json!({"outcome": format!("{outcome:?}")}),
                );
            }
        });
    }

    /// A command or autocomplete for a command registered to this guild;
    /// global commands, other guilds and other interaction kinds are ignored.
    fn is_ours(&self, interaction: &Interaction) -> bool {
        interaction.guild_id == Some(self.guild)
            && matches!(
                &interaction.data,
                Some(InteractionData::ApplicationCommand(data)) if data.guild_id == Some(self.guild)
            )
    }

    fn interaction(&mut self, interaction: Box<Interaction>) {
        let dispatcher = match &self.dispatcher {
            Some(dispatcher) if self.is_ours(&interaction) => Arc::clone(dispatcher),
            _ => {
                logging::event("INFO", "interaction_ignored", json!({}));
                return;
            }
        };
        let handled = spawn_interaction(
            dispatcher,
            Arc::clone(&self.transport),
            self.guild,
            interaction,
            (self.owner)(),
        );
        self.spawn(async move {
            if let Ok(Some((disposition, outcome))) = handled.await {
                // A failure's detail can quote store errors; the kind is enough.
                let level = match &disposition {
                    Disposition::Failed(_) => "WARN",
                    _ => "INFO",
                };
                logging::event(
                    level,
                    "interaction_handled",
                    handled_fields(&disposition, &outcome),
                );
            }
        });
    }
}

/// `interaction_handled` fields: the disposition's name, whether the answer
/// arrived and, when not, the content-free failure kind (`http_400`,
/// `rate_limited`, `not_sent`, …). A failure's detail can quote store
/// errors, so only its kind is logged; no content, no ids.
pub fn handled_fields(disposition: &Disposition, outcome: &Outcome<()>) -> serde_json::Value {
    let mut fields = json!({
        "disposition": format!("{disposition:?}").split('(').next(),
        "delivered": outcome.is_delivered(),
    });
    if let Some(label) = outcome.failure_label() {
        fields["rejected"] = json!(label);
    }
    fields
}

impl<T: DiscordTransport + 'static> EventHandler for Fanout<T> {
    async fn handle(&mut self, event: BotEvent) {
        match event {
            BotEvent::Ready {
                self_id,
                application_id,
                name,
            } => {
                self.self_id = Some(self_id);
                (self.on_ready)(application_id);
                if self.dispatcher.is_none() {
                    self.dispatcher = Some((self.commands)(Some(name)));
                }
                self.sync_pending = true;
                logging::event("INFO", "gateway_ready", json!({}));
            }
            BotEvent::GuildAvailable {
                owner_id,
                admin_roles,
            } => {
                let _ = self.roster.send(RosterJob::GuildAvailable {
                    owner_id,
                    admin_roles,
                });
                if std::mem::take(&mut self.sync_pending) {
                    self.register_commands();
                    let _ = self.roster.send(RosterJob::Reconcile);
                    self.guild_ready.send_replace(true);
                    logging::event("INFO", "guild_available", json!({}));
                }
            }
            BotEvent::Roster(update) => {
                let _ = self.roster.send(RosterJob::Update(update));
            }
            BotEvent::Reaction { reaction, added } => {
                if let Some(rsvp) =
                    rsvp_reaction(&reaction, added, self.self_id, self.directory.as_ref())
                {
                    let _ = self.reactions.send(rsvp);
                }
            }
            BotEvent::Interaction(interaction) => self.interaction(interaction),
            BotEvent::MessageCreated(message) => {
                // Chat first: v4 never extracts from a message the chatbot took.
                let handled_by_chat = self
                    .chat
                    .as_ref()
                    .is_some_and(|chat| chat.message(&message, self.self_id));
                self.messages.bump(0, 1);
                self.forward(FeedItem::Posted {
                    message,
                    self_id: self.self_id,
                    handled_by_chat,
                });
            }
            BotEvent::MessageUpdated(message) => {
                self.messages.bump(1, 1);
                self.forward(FeedItem::Edited(message, self.self_id));
            }
            BotEvent::MessagesDeleted(deleted) => {
                if let Some(chat) = &self.chat {
                    chat.deleted(&deleted);
                }
                self.messages.bump(
                    2,
                    u64::try_from(deleted.message_ids.len()).unwrap_or(u64::MAX),
                );
                self.forward(FeedItem::Deleted(deleted));
            }
        }
    }
}
