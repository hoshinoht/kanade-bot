//! The chat driver's Discord side: its own reactions and the message effects
//! that deliver an answer. Each effect is one transport call with its outcome
//! classified for the driver, which owns the retry rules; nothing here
//! retries. Every message mentions nobody; a silent one is sent with
//! `SUPPRESS_NOTIFICATIONS`.

use std::sync::Arc;

use serde_json::json;

use crate::bot::ids::{id_text, parse_id};
use crate::bot::mentions;
use crate::bot::transport::{
    DiscordTransport, MessageEdit, Outcome, OutgoingMessage, RejectionKind, SILENT,
};
use crate::chat::driver::{Effect, Post, Surface};
use crate::runtime::logging;

pub struct DiscordSurface<T>(pub Arc<T>);

fn failed<T>(event: &'static str, outcome: &Outcome<T>) {
    if let Some(label) = outcome.failure_label() {
        logging::event("WARN", event, json!({"outcome": label}));
    }
}

/// The driver's view of an outcome.
fn effect<T, U>(event: &'static str, outcome: Outcome<T>, map: impl FnOnce(T) -> U) -> Effect<U> {
    failed(event, &outcome);
    match outcome {
        Outcome::Delivered(value) => Effect::Done(map(value)),
        Outcome::DefinitelyRejected(RejectionKind::NotSent | RejectionKind::RateLimited) => {
            Effect::NotSent
        }
        Outcome::DefinitelyRejected(RejectionKind::UnknownMessage) => Effect::UnknownMessage,
        Outcome::DefinitelyRejected(kind) => Effect::Rejected(kind.label()),
        Outcome::Ambiguous(kind) => Effect::Ambiguous(kind.label()),
    }
}

fn unknown_id<T>() -> Effect<T> {
    Effect::Rejected("invalid_id".into())
}

impl<T: DiscordTransport + 'static> DiscordSurface<T> {
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

    async fn post(&self, channel_id: &str, post: Post<'_>) -> Effect<String> {
        let Some(channel) = parse_id(channel_id) else {
            return unknown_id();
        };
        let message = OutgoingMessage {
            content: Some(post.text.to_owned()),
            embeds: Vec::new(),
            // Names only; the asker is not pinged by the reply either.
            allowed_mentions: mentions::none(),
            reply_to: post.reply_to.and_then(parse_id),
            attachments: Vec::new(),
        };
        let outcome = if post.silent {
            self.0
                .create_flagged_message(channel, &message, SILENT)
                .await
        } else {
            self.0.create_message(channel, &message).await
        };
        effect("chat_post_failed", outcome, id_text)
    }

    async fn edit(&self, channel_id: &str, message_id: &str, text: &str) -> Effect<()> {
        let (Some(channel), Some(message)) = (parse_id(channel_id), parse_id(message_id)) else {
            return unknown_id();
        };
        let edit = MessageEdit {
            content: Some(text.to_owned()),
            embeds: None,
            allowed_mentions: mentions::none(),
        };
        let outcome = self.0.edit_message(channel, message, &edit).await;
        effect("chat_edit_failed", outcome, |()| ())
    }

    async fn delete(&self, channel_id: &str, message_id: &str) -> Effect<()> {
        let (Some(channel), Some(message)) = (parse_id(channel_id), parse_id(message_id)) else {
            return unknown_id();
        };
        let outcome = self.0.delete_message(channel, message).await;
        effect("chat_delete_failed", outcome, |()| ())
    }

    /// Fire and forget: a failed trigger is neither logged nor retried.
    async fn typing(&self, channel_id: &str) {
        if let Some(channel) = parse_id(channel_id) {
            let _ = self.0.trigger_typing(channel).await;
        }
    }
}
