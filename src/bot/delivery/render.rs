//! A planned reminder or digest send as a Discord post: its card
//! (`cards/`), art uploaded, allow-list exactly the intent's mentions.

use std::sync::Arc;

use super::cards::{self, ArtSource, CardContext, fetch_art};
use crate::bot::mentions;
use crate::bot::transport::OutgoingMessage;
use crate::domain::notify::NotificationIntent;

/// The message for `intent`. `heading` is a day-of card's stored heading
/// line (v4's when `None`). Kinds rendered elsewhere come back empty.
/// Art is resolved and read once, off the async runtime.
pub async fn render(
    intent: &NotificationIntent,
    ctx: &CardContext<'_>,
    heading: Option<&str>,
    art: Option<&Arc<dyn ArtSource>>,
) -> OutgoingMessage {
    match cards::build(&intent.content, ctx, heading, &intent.mentions) {
        Some(card) => {
            let pictures = fetch_art(art, &card, true).await;
            card.message(&intent.mentions, &pictures)
        }
        None => OutgoingMessage {
            content: Some(String::new()),
            embeds: Vec::new(),
            allowed_mentions: mentions::for_intent(intent),
            reply_to: None,
            attachments: Vec::new(),
        },
    }
}

/// The placeholder for a suppressed send, which the executor returns on
/// before posting: nothing is rendered or read for it.
pub(super) fn unrendered() -> OutgoingMessage {
    OutgoingMessage {
        content: None,
        embeds: Vec::new(),
        allowed_mentions: mentions::none(),
        reply_to: None,
        attachments: Vec::new(),
    }
}
