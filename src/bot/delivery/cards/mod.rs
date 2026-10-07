//! Reminder and digest cards, ported from v4 `bot.agent.formatting`
//! (`day_of_card`, `countdown_card`, `digest_card`) with the v5 attendance
//! tally in v5 mode, and their redesigned forms (`redesign/`) when the
//! message style asks for them. A [`Card`] is plain data; [`Card::message`]
//! turns it into a post with its art uploaded, [`Card::edit`] into an edit
//! that keeps the posted attachments.

mod art;
mod common;
mod countdown;
mod day_of;
mod digest;
mod heading;
mod record;
pub mod redesign;

use std::sync::Arc;

use twilight_model::channel::message::Embed;
use twilight_model::channel::message::embed::{
    EmbedField, EmbedFooter, EmbedImage, EmbedThumbnail,
};

pub use art::{
    ArtFile, ArtKind, ArtRef, ArtSource, CardArt, EmbedArt, IMAGE_PREFIX, MAX_ART_BYTES, Picture,
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
    COUNTDOWN_PHRASE_SEED, DAY_OF_HEADING_SEED, DIGEST_PHRASE_SEED, HeaderKind, HeadingRewrite,
    HeadingSource, PersonaSource, PhraseKind, PhraseRejection, Trial, Verdict, accept_phrase,
    accept_phrase_with, failure_reason, seed_heading,
};
pub use record::{CardRecord, DAY_OF_KIND, DigestPhraseStore, PostedCard, ReminderCardStore};
pub use redesign::{DifficultyMarks, StyleSource};

use crate::bot::mentions;
use crate::bot::transport::{MessageEdit, OutgoingMessage, Upload};
use crate::domain::catalog::BossTable;
use crate::domain::notify::IntentContent;
use crate::domain::settings::MessageStyle;

/// A card as plain data (v4 `Card`); mentions live in `content` only. The
/// classic style has exactly one embed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Card {
    pub content: String,
    pub embeds: Vec<CardEmbed>,
}

/// One embed of a card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardEmbed {
    pub title: Option<String>,
    pub description: Option<String>,
    pub fields: Vec<CardField>,
    pub footer: Option<String>,
    pub colour: u32,
    /// Small, top right: the lead boss's portrait.
    pub thumbnail: Option<ArtRef>,
    /// Large, bottom: the lead boss's entry artwork (day-of only).
    pub image: Option<ArtRef>,
    /// The boss token the art shows, for the admin preview's art links.
    pub lead: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardField {
    pub name: String,
    pub value: String,
    pub inline: bool,
}

impl CardField {
    /// A full-width field (every classic field).
    pub fn wide(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            inline: false,
        }
    }

    pub fn inline(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            inline: true,
            ..Self::wide(name, value)
        }
    }
}

/// What cards read besides the schedule: the catalog, the art, the header
/// rewrite, the live message style and the difficulty marks. The default
/// uses fixed phrase fallbacks and the classic style.
#[derive(Clone, Default)]
pub struct CardKit {
    pub catalog: Option<Arc<BossTable>>,
    pub art: Option<Arc<dyn ArtSource>>,
    pub heading: HeadingRewrite,
    /// Read per card; `None` is classic.
    pub style: Option<StyleSource>,
    pub marks: DifficultyMarks,
}

impl CardKit {
    /// The message style cards are built in now.
    pub fn style(&self) -> MessageStyle {
        self.style
            .as_ref()
            .map_or(MessageStyle::Classic, |style| style())
    }
}

impl std::fmt::Debug for CardKit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardKit")
            .field("catalog", &self.catalog.is_some())
            .field("art", &self.art.is_some())
            .field("heading", &self.heading)
            .field("style", &self.style())
            .field("marks", &self.marks)
            .finish()
    }
}

/// The card for `content` in `ctx.style`; `None` for kinds rendered
/// elsewhere. `header` is the stored day-of heading or countdown/digest
/// phrase; `mentioned` is the allow-list, already quiet-gated.
pub fn build(
    content: &IntentContent,
    ctx: &CardContext<'_>,
    header: Option<&str>,
    mentioned: &[String],
) -> Option<Card> {
    let redesigned = ctx.style == MessageStyle::Redesigned;
    match content {
        IntentContent::DayOf { run_ids } => {
            let seed;
            let heading = match header {
                Some(line) => line,
                None => {
                    let first = card_runs(ctx, run_ids).first().map(|run| run.datetime)?;
                    seed = seed_heading(&local_day(first, ctx.zone));
                    &seed
                }
            };
            Some(if redesigned {
                redesign::day_of_card(ctx, run_ids, heading, mentioned)
            } else {
                day_of_card(ctx, run_ids, heading, mentioned)
            })
        }
        IntentContent::Countdown { run_id, minutes } => {
            let run = ctx.run(run_id)?;
            Some(if redesigned {
                redesign::countdown_card(ctx, run, mentioned, header)
            } else {
                countdown_card(ctx, run, *minutes, mentioned, header)
            })
        }
        IntentContent::Digest {
            week_start,
            inclusion,
        } => Some(if redesigned {
            redesign::digest_card(ctx, *week_start, inclusion, header)
        } else {
            digest_card(ctx, *week_start, inclusion, header)
        }),
        IntentContent::Notice(_) | IntentContent::ProposalCard { .. } | IntentContent::Plain => {
            None
        }
    }
}

impl CardEmbed {
    fn embed(&self, thumbnail: Option<&str>, image: Option<&str>) -> Embed {
        let url = |name: &str| format!("attachment://{name}");
        Embed {
            author: None,
            color: Some(self.colour),
            description: self.description.clone(),
            fields: self
                .fields
                .iter()
                .map(|field| EmbedField {
                    inline: field.inline,
                    name: field.name.clone(),
                    value: field.value.clone(),
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
            title: self.title.clone(),
            url: None,
            video: None,
        }
    }
}

impl Card {
    /// A one-embed card (the classic shape).
    pub fn single(content: String, embed: CardEmbed) -> Self {
        Self {
            content,
            embeds: vec![embed],
        }
    }

    /// The post, with the pictures [`fetch_art`] read (`read = true`); a
    /// picture that could not be read is left off rather than failing it.
    /// A picture several embeds show is uploaded once.
    pub fn message(&self, mentioned: &[String], art: &CardArt) -> OutgoingMessage {
        let mut uploads: Vec<Upload> = Vec::new();
        let mut attach = |picture: Option<&Picture>| {
            let picture = picture?;
            if !uploads
                .iter()
                .any(|upload| upload.filename == picture.attachment)
            {
                uploads.push(Upload {
                    filename: picture.attachment.clone(),
                    bytes: picture.bytes.clone()?,
                });
            }
            Some(picture.attachment.clone())
        };
        let embeds = self
            .embeds
            .iter()
            .enumerate()
            .map(|(index, embed)| {
                let pictures = art.embeds.get(index);
                let thumbnail = attach(pictures.and_then(|p| p.thumbnail.as_ref()));
                let image = attach(pictures.and_then(|p| p.image.as_ref()));
                embed.embed(thumbnail.as_deref(), image.as_deref())
            })
            .collect();
        OutgoingMessage {
            content: Some(self.content.clone()),
            embeds,
            allowed_mentions: mentions::allow_users(mentioned),
            reply_to: None,
            attachments: uploads,
        }
    }

    /// A re-render of a posted card: nothing is uploaded again and nobody is
    /// notified. Pictures (from [`fetch_art`] with `read = false`) are
    /// referenced by the name they were posted under.
    pub fn edit(&self, art: &CardArt) -> MessageEdit {
        let name = |picture: Option<&Picture>| picture.map(|p| p.attachment.clone());
        let embeds = self
            .embeds
            .iter()
            .enumerate()
            .map(|(index, embed)| {
                let pictures = art.embeds.get(index);
                embed.embed(
                    name(pictures.and_then(|p| p.thumbnail.as_ref())).as_deref(),
                    name(pictures.and_then(|p| p.image.as_ref())).as_deref(),
                )
            })
            .collect();
        MessageEdit {
            content: Some(self.content.clone()),
            embeds: Some(embeds),
            allowed_mentions: mentions::none(),
        }
    }
}
