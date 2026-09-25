//! Twilight messages as the extraction pipeline's `IncomingMessage`.

use chrono::{DateTime, TimeDelta, Utc};
use twilight_model::channel::Message;
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, UserMarker},
};
use twilight_model::util::Timestamp;

use crate::bot::ids::id_text;
use crate::extract::pipeline::{AuthorKind, IncomingMessage, MessageOrigin};

/// A message (or edit) older than this when the gateway hands it over is
/// history (a RESUME replay, a late delivery), not live chat.
pub const STALE_AFTER: TimeDelta = TimeDelta::seconds(60);

/// Loop guard input: the bot's own posts, other bots and webhooks are never
/// read.
pub fn author_kind(message: &Message, self_id: Option<Id<UserMarker>>) -> AuthorKind {
    if Some(message.author.id) == self_id {
        AuthorKind::Myself
    } else if message.webhook_id.is_some() {
        AuthorKind::Webhook
    } else if message.author.bot {
        AuthorKind::Bot
    } else {
        AuthorKind::Member
    }
}

pub fn utc(timestamp: Timestamp) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(timestamp.as_micros()).unwrap_or_default()
}

/// Live when its latest change (edit, else post) is at most [`STALE_AFTER`]
/// before `now`.
pub fn origin(message: &Message, now: DateTime<Utc>) -> MessageOrigin {
    let seen = message.edited_timestamp.unwrap_or(message.timestamp);
    if now - utc(seen) > STALE_AFTER {
        MessageOrigin::Replay
    } else {
        MessageOrigin::Live
    }
}

/// `channel` is the message's origin (a thread's parent channel).
pub fn incoming(
    message: &Message,
    channel: Id<ChannelMarker>,
    self_id: Option<Id<UserMarker>>,
    origin: MessageOrigin,
) -> IncomingMessage {
    IncomingMessage {
        id: id_text(message.id),
        channel_id: id_text(channel),
        author_id: id_text(message.author.id),
        author: author_kind(message, self_id),
        created_at: utc(message.timestamp),
        edited_at: message.edited_timestamp.map(utc),
        content: message.content.clone(),
        origin,
        handled_by_chat: false,
    }
}
