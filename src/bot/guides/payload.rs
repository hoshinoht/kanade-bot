//! A rendered slot as Discord payloads (a create with its files, an edit
//! without them; both mention nobody), and whether a channel message shows
//! a slot (the unconfirmed-create search).

use std::sync::Arc;

use twilight_model::channel::Message;
use twilight_model::channel::message::Embed;
use twilight_model::channel::message::embed::{EmbedImage, EmbedThumbnail};
use twilight_model::id::{Id, marker::UserMarker};

use crate::discord::mentions;
use crate::discord::transport::{MessageEdit, OutgoingMessage, Upload};
use kanade_domain::guides::{GuideEmbed, RenderedSlot};

fn embed(embed: &GuideEmbed) -> Embed {
    let url = |name: &str| format!("attachment://{name}");
    Embed {
        author: None,
        color: Some(embed.colour),
        description: (!embed.description.is_empty()).then(|| embed.description.clone()),
        fields: Vec::new(),
        footer: None,
        image: embed.image.as_deref().map(|name| EmbedImage {
            height: None,
            proxy_url: None,
            url: url(name),
            width: None,
        }),
        kind: "rich".to_owned(),
        provider: None,
        thumbnail: embed.thumbnail.as_deref().map(|name| EmbedThumbnail {
            height: None,
            proxy_url: None,
            url: url(name),
            width: None,
        }),
        timestamp: None,
        title: Some(embed.title.clone()),
        url: None,
        video: None,
    }
}

/// The create: content, embeds and every file of the slot.
pub(super) fn create(slot: &RenderedSlot) -> OutgoingMessage {
    OutgoingMessage {
        content: (!slot.content.is_empty()).then(|| slot.content.clone()),
        embeds: slot.embeds.iter().map(embed).collect(),
        allowed_mentions: mentions::none(),
        reply_to: None,
        attachments: slot
            .uploads
            .iter()
            .map(|upload| Upload {
                filename: upload.file_name.clone(),
                bytes: Arc::clone(&upload.bytes),
                description: None,
            })
            .collect(),
        components: Vec::new(),
    }
}

/// The edit: content and embeds; the posted files stay (an edit only runs
/// when the slot's file set is unchanged).
pub(super) fn edit(slot: &RenderedSlot) -> MessageEdit {
    MessageEdit {
        content: Some(slot.content.clone()),
        embeds: Some(slot.embeds.iter().map(embed).collect()),
        allowed_mentions: mentions::none(),
        components: None,
    }
}

/// Whitespace runs as one space, trimmed: Discord may trim what it stores.
fn normal(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The file an image URL names: `attachment://<name>` as sent, or the last
/// path segment of the CDN URL Discord answers with.
fn file_name(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/').next().unwrap_or(path)
}

/// The bot's own message showing `slot`: the same text and the same embed
/// titles, descriptions and pictures.
pub(super) fn shows(message: &Message, me: Id<UserMarker>, slot: &RenderedSlot) -> bool {
    message.author.id == me
        && normal(&message.content) == normal(&slot.content)
        && message.embeds.len() == slot.embeds.len()
        && message.embeds.iter().zip(&slot.embeds).all(|(got, want)| {
            normal(got.title.as_deref().unwrap_or_default()) == normal(&want.title)
                && normal(got.description.as_deref().unwrap_or_default())
                    == normal(&want.description)
                && got
                    .thumbnail
                    .as_ref()
                    .map(|picture| file_name(&picture.url))
                    == want.thumbnail.as_deref()
                && got.image.as_ref().map(|picture| file_name(&picture.url))
                    == want.image.as_deref()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_names_come_from_sent_and_cdn_urls() {
        assert_eq!(
            file_name("attachment://Kalos-portrait.png"),
            "Kalos-portrait.png"
        );
        assert_eq!(
            file_name("https://cdn.example/attachments/1/2/Kalos-portrait.png?ex=1&is=2"),
            "Kalos-portrait.png"
        );
        assert_eq!(normal("  a \n b  "), "a b");
        assert_eq!(normal("\u{200B}"), "\u{200B}", "a spacer is not whitespace");
    }
}
