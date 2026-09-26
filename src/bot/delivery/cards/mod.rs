//! Reminder and digest cards, ported from v4 `bot.agent.formatting`
//! (`day_of_card`, `countdown_card`, `digest_card`) with the v5 attendance
//! tally in v5 mode. A [`Card`] is plain data; [`Card::message`] turns it
//! into a post with its art uploaded, [`Card::edit`] into an edit that keeps
//! the posted attachments.

mod art;
mod common;
mod countdown;
mod day_of;
mod digest;
mod heading;
mod record;

use std::sync::Arc;

use twilight_model::channel::message::Embed;
use twilight_model::channel::message::embed::{
    EmbedField, EmbedFooter, EmbedImage, EmbedThumbnail,
};

pub use art::{
    ArtFile, ArtKind, ArtRef, ArtSource, CardArt, IMAGE_PREFIX, MAX_ART_BYTES, Picture,
    attachment_name, fetch_art, lead_entry_art, lead_portrait,
};
pub use common::{
    COLOUR_ALL_SET, COLOUR_COUNTDOWN, COLOUR_DAY_OF, COLOUR_DIGEST, CardContext, People,
    REACT_HINT, UNNAMED, format_bosses, format_offset, local_day, local_time, status_text,
    tally_text,
};
pub use countdown::countdown_card;
pub use day_of::{card_runs, day_of_card};
pub use digest::{DIGEST_EMPTY, DIGEST_FOOTER, digest_card};
pub use heading::{
    DAY_OF_HEADING_SEED, HeadingRewrite, HeadingSource, PersonaSource, failure_reason, seed_heading,
};
pub use record::{CardRecord, DAY_OF_KIND, PostedCard, ReminderCardStore};

use crate::bot::mentions;
use crate::bot::transport::{MessageEdit, OutgoingMessage, Upload};
use crate::domain::catalog::BossTable;
use crate::domain::notify::IntentContent;

/// A card as plain data (v4 `Card`); mentions live in `content` only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Card {
    pub content: String,
    pub description: Option<String>,
    pub fields: Vec<(String, String)>,
    pub footer: Option<String>,
    pub colour: u32,
    /// Small, top right: the lead boss's portrait.
    pub thumbnail: Option<ArtRef>,
    /// Large, bottom: the lead boss's entry artwork (day-of only).
    pub image: Option<ArtRef>,
}

/// What cards read besides the schedule: the catalog, the art and the
/// day-of heading rewrite. The default renders v4's no-table cards.
#[derive(Clone, Default)]
pub struct CardKit {
    pub catalog: Option<Arc<BossTable>>,
    pub art: Option<Arc<dyn ArtSource>>,
    pub heading: HeadingRewrite,
}

impl std::fmt::Debug for CardKit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardKit")
            .field("catalog", &self.catalog.is_some())
            .field("art", &self.art.is_some())
            .field("heading", &self.heading)
            .finish()
    }
}

/// The card for `content`; `None` for kinds rendered elsewhere (notices,
/// proposal cards, plain posts). `heading` is the day-of heading line
/// (v4's when `None`); `mentioned` the allow-list, already quiet-gated.
pub fn build(
    content: &IntentContent,
    ctx: &CardContext<'_>,
    heading: Option<&str>,
    mentioned: &[String],
) -> Option<Card> {
    match content {
        IntentContent::DayOf { run_ids } => {
            let seed;
            let heading = match heading {
                Some(line) => line,
                None => {
                    let first = card_runs(ctx, run_ids).first().map(|run| run.datetime)?;
                    seed = seed_heading(&local_day(first, ctx.zone));
                    &seed
                }
            };
            Some(day_of_card(ctx, run_ids, heading, mentioned))
        }
        IntentContent::Countdown { run_id, minutes } => {
            Some(countdown_card(ctx, ctx.run(run_id)?, *minutes, mentioned))
        }
        IntentContent::Digest {
            week_start,
            inclusion,
        } => Some(digest_card(ctx, *week_start, inclusion)),
        IntentContent::Notice(_) | IntentContent::ProposalCard { .. } | IntentContent::Plain => {
            None
        }
    }
}

impl Card {
    fn embed(&self, thumbnail: Option<&str>, image: Option<&str>) -> Embed {
        let url = |name: &str| format!("attachment://{name}");
        Embed {
            author: None,
            color: Some(self.colour),
            description: self.description.clone(),
            fields: self
                .fields
                .iter()
                .map(|(name, value)| EmbedField {
                    inline: false,
                    name: name.clone(),
                    value: value.clone(),
                })
                .collect(),
            footer: self.footer.as_ref().map(|text| EmbedFooter {
                icon_url: None,
                proxy_icon_url: None,
                text: text.clone(),
            }),
            image: image.map(|name| EmbedImage {
                height: None,
                proxy_url: None,
                url: url(name),
                width: None,
            }),
            kind: "rich".to_owned(),
            provider: None,
            thumbnail: thumbnail.map(|name| EmbedThumbnail {
                height: None,
                proxy_url: None,
                url: url(name),
                width: None,
            }),
            timestamp: None,
            title: None,
            url: None,
            video: None,
        }
    }

    /// The post, with the pictures [`fetch_art`] read (`read = true`); a
    /// picture that could not be read is left off rather than failing it.
    pub fn message(&self, mentioned: &[String], art: &CardArt) -> OutgoingMessage {
        let mut uploads = Vec::new();
        let mut attach = |picture: Option<&Picture>| {
            let picture = picture?;
            uploads.push(Upload {
                filename: picture.attachment.clone(),
                bytes: picture.bytes.clone()?,
            });
            Some(picture.attachment.clone())
        };
        let thumbnail = attach(art.thumbnail.as_ref());
        let image = attach(art.image.as_ref());
        OutgoingMessage {
            content: Some(self.content.clone()),
            embeds: vec![self.embed(thumbnail.as_deref(), image.as_deref())],
            allowed_mentions: mentions::allow_users(mentioned),
            reply_to: None,
            attachments: uploads,
        }
    }

    /// A re-render of a posted card: nothing is uploaded again and nobody is
    /// notified. Pictures (from [`fetch_art`] with `read = false`) are
    /// referenced by the name they were posted under.
    pub fn edit(&self, art: &CardArt) -> MessageEdit {
        let name = |picture: &Option<Picture>| picture.as_ref().map(|p| p.attachment.clone());
        MessageEdit {
            content: Some(self.content.clone()),
            embeds: Some(vec![
                self.embed(name(&art.thumbnail).as_deref(), name(&art.image).as_deref()),
            ]),
            allowed_mentions: mentions::none(),
        }
    }
}
