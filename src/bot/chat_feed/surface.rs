//! The chat driver's Discord side: its own reactions and replies. Replies
//! are outside the delivery journal (user decision): an ambiguous reply is
//! logged and never retried.

use std::sync::Arc;

use serde_json::json;

use crate::bot::ids::{id_text, parse_id};
use crate::bot::mentions;
use crate::bot::transport::{ChannelId, DiscordTransport, MessageId, Outcome, OutgoingMessage};
use crate::chat::driver::Surface;
use crate::chat::sanitize::reply_parts;
use crate::runtime::logging;

pub struct DiscordSurface<T>(pub Arc<T>);

fn failed<T>(event: &'static str, outcome: &Outcome<T>) {
    let kind = match outcome {
        Outcome::Delivered(_) => return,
        Outcome::DefinitelyRejected(kind) => format!("{kind:?}"),
        Outcome::Ambiguous(kind) => format!("ambiguous {kind:?}"),
    };
    logging::event("WARN", event, json!({"outcome": kind}));
}

impl<T: DiscordTransport + 'static> DiscordSurface<T> {
    async fn send(
        &self,
        channel: ChannelId,
        text: String,
        reply_to: Option<MessageId>,
    ) -> Outcome<MessageId> {
        let message = OutgoingMessage {
            content: Some(text),
            embeds: Vec::new(),
            // Names only; the asker is not pinged by the reply either.
            allowed_mentions: mentions::none(),
            reply_to,
            attachments: Vec::new(),
        };
        self.0.create_message(channel, &message).await
    }

    async fn reaction(&self, channel_id: &str, message_id: &str, emoji: &str, add: bool) {
        let (Some(channel), Some(message)) = (parse_id(channel_id), parse_id(message_id)) else {
            return;
        };
        let outcome = if add {
            self.0.add_own_reaction(channel, message, emoji).await
        } else {
            self.0.remove_own_reaction(channel, message, emoji).await
        };
        failed("chat_reaction_failed", &outcome);
    }
}

impl<T: DiscordTransport + 'static> Surface for DiscordSurface<T> {
    async fn react(&self, channel_id: &str, message_id: &str, emoji: &str) {
        self.reaction(channel_id, message_id, emoji, true).await;
    }

    async fn unreact(&self, channel_id: &str, message_id: &str, emoji: &str) {
        self.reaction(channel_id, message_id, emoji, false).await;
    }

    /// A reply over the member bound posts as follow-ups after it
    /// (`reply_parts`), in order and pinging nobody. A failed follow-up is
    /// logged and ends the reply; the answer is not retried.
    async fn reply(&self, channel_id: &str, reply_to: &str, text: &str) -> Result<String, String> {
        let channel = parse_id(channel_id).ok_or("unknown channel")?;
        let mut parts = reply_parts(text).into_iter();
        let first = parts.next().unwrap_or_default();
        let outcome = self.send(channel, first, parse_id(reply_to)).await;
        failed("chat_reply_failed", &outcome);
        let id = match outcome {
            Outcome::Delivered(id) => id_text(id),
            Outcome::DefinitelyRejected(kind) => return Err(format!("{kind:?}")),
            Outcome::Ambiguous(kind) => return Err(format!("ambiguous {kind:?}")),
        };
        for (index, part) in parts.enumerate() {
            let outcome = self.send(channel, part, None).await;
            if !matches!(outcome, Outcome::Delivered(_)) {
                failed("chat_followup_failed", &outcome);
                logging::event("WARN", "chat_followup_stopped", json!({"part": index + 2}));
                break;
            }
        }
        Ok(id)
    }
}
