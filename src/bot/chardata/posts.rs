//! The Discord side of tracked share cards: posting a card through the
//! transport (not as an interaction response, so it stays editable after
//! the token's 15 minutes), finding the bot's own card by its ref after an
//! ambiguous create, and stripping it to the stub (or deleting it when an
//! edit is refused).

use std::sync::Arc;
use std::time::Duration;

use twilight_model::channel::Message;
use twilight_model::channel::message::Component;
use twilight_model::id::{Id, marker::UserMarker};

use super::copy;
use crate::bot::ids::{id_text, parse_id};
use crate::bot::mentions;
use crate::bot::transport::{
    DiscordTransport, HistoryPage, MessageEdit, Outcome, OutgoingMessage, RejectionKind,
};
use crate::infrastructure::character_data::{DeskFuture, Located, PostDesk, Stripped};

/// The bot's own user id once `READY` arrived (serve: the guild cache).
pub type SelfId = Arc<dyn Fn() -> Option<Id<UserMarker>> + Send + Sync>;

/// How many of a channel's latest messages a ref search reads.
const SEARCH_DEPTH: u16 = 50;
/// Searches after an ambiguous create before the card is `unknown`.
const SEARCHES: usize = 2;

/// How a card send ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sent {
    Posted(String),
    /// Definitely not posted; the rejection.
    Refused(RejectionKind),
    /// Maybe posted, and not found by its ref.
    Unknown,
}

pub struct DiscordPosts<T> {
    pub transport: Arc<T>,
    pub self_id: SelfId,
    /// The wait between the two searches after an ambiguous create.
    pub search_pause: Duration,
}

/// Every text display's content, nested ones included.
fn texts(components: &[Component], out: &mut Vec<String>) {
    for component in components {
        match component {
            Component::TextDisplay(display) => out.push(display.content.clone()),
            Component::Container(container) => texts(&container.components, out),
            Component::Section(section) => texts(&section.components, out),
            _ => {}
        }
    }
}

/// The bot's own message carrying `footer`.
fn carries(message: &Message, me: Id<UserMarker>, footer: &str) -> bool {
    if message.author.id != me {
        return false;
    }
    let mut found = Vec::new();
    texts(&message.components, &mut found);
    found.iter().any(|text| text.contains(footer))
}

impl<T: DiscordTransport> DiscordPosts<T> {
    /// Post `card` in `channel_id`. An ambiguous create is looked for by
    /// its ref twice before it counts as unknown; it is never sent again.
    pub async fn send(&self, channel_id: &str, card: Vec<Component>, reference: &str) -> Sent {
        let Some(channel) = parse_id(channel_id) else {
            return Sent::Refused(RejectionKind::Invalid);
        };
        let message = OutgoingMessage::v2(card, mentions::none());
        match self.transport.create_message(channel, &message).await {
            Outcome::Delivered(id) => Sent::Posted(id_text(id)),
            Outcome::DefinitelyRejected(kind) => Sent::Refused(kind),
            Outcome::Ambiguous(_) => {
                for search in 0..SEARCHES {
                    if search > 0 {
                        tokio::time::sleep(self.search_pause).await;
                    }
                    if let Located::Found(id) = self.find(channel_id, reference).await {
                        return Sent::Posted(id);
                    }
                }
                Sent::Unknown
            }
        }
    }

    async fn find(&self, channel_id: &str, reference: &str) -> Located {
        let (Some(channel), Some(me)) = (parse_id(channel_id), (self.self_id)()) else {
            return Located::Unreadable;
        };
        let footer = copy::share_footer(reference);
        match self
            .transport
            .channel_messages(channel, HistoryPage::Latest, SEARCH_DEPTH)
            .await
        {
            Outcome::Delivered(messages) => messages
                .iter()
                .find(|message| carries(message, me, &footer))
                .map_or(Located::Missing, |message| {
                    Located::Found(id_text(message.id))
                }),
            _ => Located::Unreadable,
        }
    }

    async fn strip_message(&self, channel_id: &str, message_id: &str) -> Stripped {
        let (Some(channel), Some(message)) = (parse_id(channel_id), parse_id(message_id)) else {
            return Stripped::Stuck;
        };
        let edit = MessageEdit::v2(copy::stub(), mentions::none());
        match self.transport.edit_message(channel, message, &edit).await {
            Outcome::Delivered(()) => Stripped::Done,
            Outcome::DefinitelyRejected(
                RejectionKind::UnknownMessage | RejectionKind::UnknownChannel,
            ) => Stripped::Done,
            Outcome::Ambiguous(_)
            | Outcome::DefinitelyRejected(RejectionKind::NotSent | RejectionKind::RateLimited) => {
                Stripped::Retry
            }
            // Missing access or permission (or any other refusal): the
            // card must not stay, so it is deleted instead.
            Outcome::DefinitelyRejected(_) => {
                match self.transport.delete_message(channel, message).await {
                    Outcome::Delivered(())
                    | Outcome::DefinitelyRejected(
                        RejectionKind::UnknownMessage | RejectionKind::UnknownChannel,
                    ) => Stripped::Done,
                    Outcome::Ambiguous(_)
                    | Outcome::DefinitelyRejected(
                        RejectionKind::NotSent | RejectionKind::RateLimited,
                    ) => Stripped::Retry,
                    Outcome::DefinitelyRejected(_) => Stripped::Stuck,
                }
            }
        }
    }
}

impl<T: DiscordTransport + 'static> PostDesk for DiscordPosts<T> {
    fn strip<'a>(&'a self, channel_id: &'a str, message_id: &'a str) -> DeskFuture<'a, Stripped> {
        Box::pin(self.strip_message(channel_id, message_id))
    }

    fn locate<'a>(&'a self, channel_id: &'a str, reference: &'a str) -> DeskFuture<'a, Located> {
        Box::pin(self.find(channel_id, reference))
    }
}
