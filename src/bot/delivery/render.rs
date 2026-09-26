//! A planned reminder or digest send as a Discord post: its card
//! (`cards/`), art uploaded, allow-list exactly the intent's mentions.

use super::cards::{self, ArtSource, CardContext};
use crate::bot::mentions;
use crate::bot::transport::OutgoingMessage;
use crate::domain::notify::NotificationIntent;

/// The message for `intent`. `heading` is a day-of card's stored heading
/// line (v4's when `None`). Kinds rendered elsewhere come back empty.
pub fn render(
    intent: &NotificationIntent,
    ctx: &CardContext<'_>,
    heading: Option<&str>,
    art: Option<&dyn ArtSource>,
) -> OutgoingMessage {
    match cards::build(&intent.content, ctx, heading, &intent.mentions) {
        Some(card) => card.message(&intent.mentions, art),
        None => OutgoingMessage {
            content: Some(String::new()),
            embeds: Vec::new(),
            allowed_mentions: mentions::for_intent(intent),
            reply_to: None,
            attachments: Vec::new(),
        },
    }
}
