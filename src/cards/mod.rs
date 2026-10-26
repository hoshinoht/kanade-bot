//! Card rendering the bot posts and the admin portal previews: reminder and
//! digest cards (v4 `bot.agent.formatting`'s `day_of_card`,
//! `countdown_card`, `digest_card` with the v5 attendance tally, laid out by
//! `redesign/`), their headings, art and estimate chart, the proposal-card
//! wording (`format`), the character-data board copy (`chardata`) and the
//! shared date and boss text (`text`). A [`Card`] is plain data;
//! [`Card::message`] turns it into a post with its art uploaded,
//! [`Card::edit`] into an edit that keeps the posted attachments. A card
//! with a Components V2 layout (the digest) is posted and edited as that
//! layout; its embed stays the fallback and the admin preview's rendering.

mod art;
pub mod chardata;
pub mod chart;
mod common;
pub mod estimates;
pub mod format;
mod heading;
pub mod preview;
pub mod redesign;
pub mod render;
pub mod text;

use std::sync::Arc;

use twilight_model::channel::message::embed::{
    EmbedField, EmbedFooter, EmbedImage, EmbedThumbnail,
};
use twilight_model::channel::message::{Component, Embed};

pub use art::{
    ArtRef, CardArt, EmbedArt, IMAGE_PREFIX, Picture, attachment_name, fetch_art, lead_entry_art,
    lead_portrait,
};
pub use common::{CardContext, People, REACT_HINT, UNNAMED, card_runs};
pub use heading::{
    COUNTDOWN_PHRASE_SEED, Chosen, DAY_OF_HEADING_SEED, DIGEST_PHRASE_SEED, HeaderContext,
    HeaderKind, HeaderLine, HeaderSwitchSource, HeaderSwitches, HeadingRewrite, HeadingSource,
    MAX_SLOTS, PersonaSource, PhraseKind, PhraseRejection, SlotFill, SlotRule, Teasable, Trial,
    TrialOrigin, Verdict, accept_header_with, accept_phrase, accept_phrase_with, failure_reason,
    fill_slots, resolve_heading, seed_heading,
};
pub use redesign::{DifficultyMarks, PortalSwitch, V2Kit};

use crate::cards::text::local_day;
use crate::discord::mentions;
use crate::discord::transport::{MessageEdit, OutgoingMessage, Upload};
use kanade_domain::art::ArtSource;
use kanade_domain::catalog::BossTable;
use kanade_domain::notify::IntentContent;

/// A card as plain data (v4 `Card`); mentions live in `content` only.
/// `components`, when set, is the Components V2 form that is sent instead
/// of the content and embeds. `rows` are legacy button rows under the
/// content and embeds: `Some` on run cards (day-of, countdown: "Open in
/// portal"), so every edit sends them and a row the card no longer has
/// (the portal closed) is cleared; `None` elsewhere, and an edit leaves the
/// message's components alone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Card {
    pub content: String,
    pub embeds: Vec<CardEmbed>,
    pub components: Vec<Component>,
    pub rows: Option<Vec<Component>>,
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
    /// A full-width field.
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
/// rewrite, the difficulty marks and the Components V2 parts. The default
/// uses fixed phrase fallbacks.
#[derive(Clone, Default)]
pub struct CardKit {
    pub catalog: Option<Arc<BossTable>>,
    pub art: Option<Arc<dyn ArtSource>>,
    pub heading: HeadingRewrite,
    pub marks: DifficultyMarks,
    /// The avatar, portal origin and switch and which posted messages are
    /// V2; shared by every clone of the kit.
    pub v2: V2Kit,
    /// v5: the live run lengths, read per digest re-render so a run past its
    /// end counts as ended; `None` keeps v4's counts.
    pub run_lengths: Option<kanade_domain::completion::RunLengthsNow>,
    /// The cached estimates behind the run cards' share chart; `None`
    /// (module `character_data` or its estimates off) posts none.
    pub estimates: Option<Arc<dyn crate::cards::estimates::PartyEstimates>>,
    /// The live header switches (Config › Notifications); `None` is all off.
    pub switches: Option<HeaderSwitchSource>,
}

impl CardKit {
    /// The header switches as configured now.
    pub fn switches(&self) -> HeaderSwitches {
        self.switches
            .as_ref()
            .map_or_else(HeaderSwitches::default, |switches| switches())
    }

    /// When runs end now under `policy`; `None` without live run lengths.
    pub fn run_ends(
        &self,
        policy: &kanade_domain::schedule::SchedulePolicy,
    ) -> Option<kanade_domain::completion::RunEnds> {
        let lengths = self.run_lengths.as_ref()?;
        Some(kanade_domain::completion::RunEnds::new(
            lengths(),
            self.catalog.clone(),
            policy.clone(),
        ))
    }
}

impl std::fmt::Debug for CardKit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardKit")
            .field("catalog", &self.catalog.is_some())
            .field("art", &self.art.is_some())
            .field("heading", &self.heading)
            .field("marks", &self.marks)
            .field("estimates", &self.estimates.is_some())
            .finish()
    }
}

/// The card for `content`; `None` for kinds rendered elsewhere. `header`
/// is the stored day-of heading or countdown/digest phrase; `mentioned` is
/// the allow-list, already quiet-gated.
pub fn build(
    content: &IntentContent,
    ctx: &CardContext<'_>,
    header: Option<&str>,
    mentioned: &[String],
) -> Option<Card> {
    let mut card = match content {
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
            redesign::day_of_card(ctx, run_ids, heading, mentioned)
        }
        IntentContent::Countdown { run_id, .. } => {
            redesign::countdown_card(ctx, ctx.run(run_id)?, mentioned, header)
        }
        IntentContent::Digest {
            week_start,
            inclusion,
        } => {
            return Some(redesign::digest_card(ctx, *week_start, inclusion, header));
        }
        IntentContent::Notice(_) | IntentContent::ProposalCard { .. } | IntentContent::Plain => {
            return None;
        }
    };
    // A run card owns its rows, so an edit clears a link it no longer has.
    card.rows.get_or_insert_default();
    Some(card)
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
    /// A one-embed card.
    pub fn single(content: String, embed: CardEmbed) -> Self {
        Self {
            content,
            embeds: vec![embed],
            ..Self::default()
        }
    }

    /// The button rows a legacy post or edit sends, only beside text: rows
    /// without content would be read as a V2 layout (see
    /// [`crate::discord::transport::legacy_rows`]).
    fn legacy_rows(&self) -> Option<Vec<Component>> {
        self.rows.clone().filter(|_| !self.content.is_empty())
    }

    /// `prefix` before the content, or as a first line of a V2 layout's
    /// container (a `/debug ping` test card).
    #[must_use]
    pub fn prefixed(mut self, prefix: &str) -> Self {
        self.content = format!("{prefix}{}", self.content);
        if let Some(Component::Container(container)) = self.components.first_mut() {
            let label = prefix.trim().trim_end_matches('—').trim();
            container
                .components
                .insert(0, redesign::text(redesign::subtext(label)));
            if !redesign::within_budget(&self.components) {
                self.components.clear();
            }
        }
        self
    }

    /// The post, with the pictures [`fetch_art`] read (`read = true`); a
    /// picture that could not be read is left off rather than failing it.
    /// A picture several embeds show is uploaded once.
    pub fn message(&self, mentioned: &[String], art: &CardArt) -> OutgoingMessage {
        if !self.components.is_empty() {
            // A V2 layout shows no art: nothing is uploaded.
            return OutgoingMessage::v2(self.components.clone(), mentions::allow_users(mentioned));
        }
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
                    description: None,
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
            components: self.legacy_rows().unwrap_or_default(),
        }
    }

    /// A re-render of a posted card: nothing is uploaded again and nobody is
    /// notified. Pictures (from [`fetch_art`] with `read = false`) are
    /// referenced by the name they were posted under. A V2 layout is sent
    /// as a V2 edit, which also converts a legacy post.
    pub fn edit(&self, art: &CardArt) -> MessageEdit {
        if !self.components.is_empty() {
            return MessageEdit::v2(self.components.clone(), mentions::none());
        }
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
            components: self.legacy_rows(),
        }
    }
}
