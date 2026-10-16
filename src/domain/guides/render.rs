//! Blocks → Discord messages ("slots"), shared by the API preview and the
//! publish worker. Pure apart from [`GuideSources`], whose art reads block:
//! callers run [`render`] off the async runtime. Values over a Discord limit
//! are refused with the block and field named, never clipped.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Serialize;

use super::blocks::{self, Block, BlockError, BossEntry, units};
use super::hash;
use crate::domain::catalog::{Boss, BossTable};

/// Most messages one guide renders (`guide_slots.position` 0..=199).
pub const MAX_SLOTS: usize = 200;
/// Discord's embeds per message.
pub const MAX_EMBEDS: usize = 10;
/// Discord's total embed text per message.
pub const EMBED_TOTAL_LIMIT: usize = 6000;
pub const TITLE_LIMIT: usize = 256;
pub const DESCRIPTION_LIMIT: usize = 4096;
/// Files per message (the delivery cards' `MAX_ATTACHMENTS`).
pub const MAX_UPLOADS: usize = 10;
/// Upload bytes per message: well under Discord's per-file and request caps.
pub const MAX_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
/// A spacer's content: Discord refuses an empty message.
pub const SPACER: &str = "\u{200B}";
/// v4's embed colour for a boss without a guide colour.
pub const DEFAULT_COLOUR: u32 = 0x98A1B3;

/// The two pictures a boss has. Portraits are found by the catalog's
/// portrait basename (else the key), entry art by the key; exact case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArtKind {
    Portrait,
    Entry,
}

impl ArtKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Portrait => "portrait",
            Self::Entry => "entry",
        }
    }
}

/// One art file as read: `<basename>.<ext>` and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtFile {
    pub file_name: String,
    pub bytes: Arc<[u8]>,
}

/// What rendering reads besides the catalog; the bot and API adapt the
/// boss directory and `boss/knowledge/` to it.
pub trait GuideSources {
    /// The boss's knowledge `lead`, when its document has one.
    fn lead(&self, key: &str) -> Option<String>;
    /// Blocking: the art file, matched exactly (case included); `None` when
    /// it is missing, unreadable or over the art size cap.
    fn art(&self, kind: ArtKind, basename: &str) -> Option<ArtFile>;
}

/// One boss embed. Pictures name an upload of the same message
/// (`attachment://<file name>`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GuideEmbed {
    pub title: String,
    pub description: String,
    pub colour: u32,
    pub thumbnail: Option<String>,
    pub image: Option<String>,
}

impl GuideEmbed {
    /// What the embed adds to the message's [`EMBED_TOTAL_LIMIT`].
    pub fn units(&self) -> usize {
        units(&self.title) + units(&self.description)
    }
}

/// A file uploaded with a message, named `<key>-<kind>.<ext>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upload {
    pub file_name: String,
    /// 64 lowercase hex.
    pub sha256: String,
    pub bytes: Arc<[u8]>,
}

/// A stored slot's upload manifest entry: what an edit must leave as is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, serde::Deserialize)]
pub struct UploadRef {
    pub file_name: String,
    pub sha256: String,
}

/// One message a guide wants, in guide order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedSlot {
    pub block_id: String,
    /// The message's index within its block (bosses blocks may need several).
    pub part: u32,
    pub content: String,
    pub embeds: Vec<GuideEmbed>,
    /// Each distinct file once, by file name.
    pub uploads: Vec<Upload>,
    /// [`hash::slot_hash`] of the rendered output.
    pub hash: String,
}

impl RenderedSlot {
    /// The upload manifest, by file name.
    pub fn manifest(&self) -> Vec<UploadRef> {
        self.uploads
            .iter()
            .map(|upload| UploadRef {
                file_name: upload.file_name.clone(),
                sha256: upload.sha256.clone(),
            })
            .collect()
    }

    /// Same files as a stored manifest (both are kept by file name).
    pub fn same_uploads(&self, manifest: &[UploadRef]) -> bool {
        self.uploads.len() == manifest.len()
            && self.uploads.iter().zip(manifest).all(|(upload, stored)| {
                upload.file_name == stored.file_name && upload.sha256 == stored.sha256
            })
    }
}

/// Something the preview shows that never fails a render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderWarning {
    /// The entry's picture was not found: the embed goes without it.
    MissingArt {
        block_id: String,
        entry: usize,
        key: String,
        kind: ArtKind,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    pub slots: Vec<RenderedSlot>,
    pub warnings: Vec<RenderWarning>,
}

/// One located picture, read and hashed once per render.
#[derive(Clone)]
struct Picture {
    file_name: String,
    sha256: String,
    bytes: Arc<[u8]>,
}

/// One message being filled.
#[derive(Default)]
struct Part {
    content: String,
    embeds: Vec<GuideEmbed>,
    uploads: Vec<Upload>,
    embed_units: usize,
    upload_bytes: usize,
}

impl Part {
    /// Whether `embed` with `pictures` still fits beside what is here.
    fn fits(&self, embed: &GuideEmbed, pictures: &[&Picture]) -> bool {
        let new: Vec<&&Picture> = pictures
            .iter()
            .filter(|picture| !self.has(&picture.file_name))
            .collect();
        self.embeds.len() < MAX_EMBEDS
            && self.embed_units + embed.units() <= EMBED_TOTAL_LIMIT
            && self.uploads.len() + new.len() <= MAX_UPLOADS
            && self.upload_bytes + new.iter().map(|picture| picture.bytes.len()).sum::<usize>()
                <= MAX_UPLOAD_BYTES
    }

    fn has(&self, file_name: &str) -> bool {
        self.uploads
            .iter()
            .any(|upload| upload.file_name == file_name)
    }

    fn add(&mut self, embed: GuideEmbed, pictures: &[&Picture]) {
        for picture in pictures {
            if !self.has(&picture.file_name) {
                self.upload_bytes += picture.bytes.len();
                self.uploads.push(Upload {
                    file_name: picture.file_name.clone(),
                    sha256: picture.sha256.clone(),
                    bytes: Arc::clone(&picture.bytes),
                });
            }
        }
        self.embed_units += embed.units();
        self.embeds.push(embed);
    }
}

/// Render `blocks` (validated as [`blocks::validate`]) into messages within
/// Discord's limits.
///
/// # Errors
/// Every [`BlockError`] of the draft, plus derived descriptions or titles
/// over their limit and more than [`MAX_SLOTS`] messages.
pub fn render(
    blocks: &[Block],
    catalog: &BossTable,
    sources: &dyn GuideSources,
) -> Result<Rendered, Vec<BlockError>> {
    let mut errors = blocks::validate(blocks, catalog);
    if !errors.is_empty() {
        return Err(errors);
    }
    // By kind and boss key: the key fixes both the basename and the upload name.
    let mut art: HashMap<(ArtKind, String), Option<Picture>> = HashMap::new();
    let mut slots = Vec::new();
    let mut warnings = Vec::new();
    for block in blocks {
        let block_id = block.id();
        let parts = match block {
            Block::Text { content, .. } => vec![text_part(content)],
            Block::Spacer { .. } => vec![text_part(SPACER)],
            Block::Bosses {
                heading, entries, ..
            } => {
                let mut parts = vec![Part {
                    content: heading.clone().unwrap_or_default(),
                    ..Part::default()
                }];
                for (at, entry) in entries.iter().enumerate() {
                    // Validated: every key is in the catalog.
                    let Some(boss) = catalog.boss(&entry.key) else {
                        continue;
                    };
                    let mut embed = boss_embed(catalog, boss, entry, sources.lead(&entry.key));
                    for (field, value, limit) in [
                        ("title", &embed.title, TITLE_LIMIT),
                        ("description", &embed.description, DESCRIPTION_LIMIT),
                    ] {
                        blocks::limit(
                            &mut errors,
                            block_id,
                            &format!("entries[{at}].{field}"),
                            value,
                            limit,
                        );
                    }
                    let mut pictures = Vec::new();
                    for (kind, wanted) in [
                        (ArtKind::Portrait, true),
                        (ArtKind::Entry, entry.large_image),
                    ] {
                        if !wanted {
                            continue;
                        }
                        let basename = match kind {
                            ArtKind::Portrait => boss.portrait().unwrap_or(boss.short()),
                            ArtKind::Entry => boss.short(),
                        };
                        let found = art
                            .entry((kind, boss.short().to_owned()))
                            .or_insert_with(|| picture(sources, kind, basename, boss.short()));
                        match found {
                            Some(found) => {
                                let name = Some(found.file_name.clone());
                                match kind {
                                    ArtKind::Portrait => embed.thumbnail = name,
                                    ArtKind::Entry => embed.image = name,
                                }
                                pictures.push(found.clone());
                            }
                            None => warnings.push(RenderWarning::MissingArt {
                                block_id: block_id.to_owned(),
                                entry: at,
                                key: entry.key.clone(),
                                kind,
                            }),
                        }
                    }
                    let pictures: Vec<&Picture> = pictures.iter().collect();
                    let last = parts
                        .last_mut()
                        .filter(|part| part.embeds.is_empty() || part.fits(&embed, &pictures));
                    match last {
                        Some(part) => part.add(embed, &pictures),
                        None => {
                            let mut part = Part::default();
                            part.add(embed, &pictures);
                            parts.push(part);
                        }
                    }
                }
                parts
            }
        };
        for (part, mut message) in parts.into_iter().enumerate() {
            message
                .uploads
                .sort_by(|a, b| a.file_name.cmp(&b.file_name));
            let hash = hash::slot_hash(&message.content, &message.embeds, &message.uploads);
            slots.push(RenderedSlot {
                block_id: block_id.to_owned(),
                part: u32::try_from(part).unwrap_or(u32::MAX),
                content: message.content,
                embeds: message.embeds,
                uploads: message.uploads,
                hash,
            });
        }
    }
    if slots.len() > MAX_SLOTS {
        errors.push(BlockError::TooManySlots {
            count: slots.len(),
            limit: MAX_SLOTS,
        });
    }
    if errors.is_empty() {
        Ok(Rendered { slots, warnings })
    } else {
        Err(errors)
    }
}

fn text_part(content: &str) -> Part {
    Part {
        content: content.to_owned(),
        ..Part::default()
    }
}

/// v4's guide embed: `{full} · Lv{level}`, the difficulty names, the
/// lowercase tokens and the aliases, then the knowledge lead and the note.
fn boss_embed(
    catalog: &BossTable,
    boss: &Boss,
    entry: &BossEntry,
    lead: Option<String>,
) -> GuideEmbed {
    let mut title = boss.full().to_owned();
    if let Some(level) = boss.level() {
        title.push_str(&format!(" · Lv{level}"));
    }
    let names: Vec<String> = boss
        .difficulties()
        .iter()
        .map(|letter| catalog.difficulty_name(letter))
        .collect();
    let tokens: Vec<String> = boss
        .difficulties()
        .iter()
        .map(|letter| boss.canonical(letter).to_lowercase())
        .collect();
    let mut description = format!("{}\n`{}`", names.join(" · "), tokens.join(" · "));
    if !boss.aliases().is_empty() {
        description.push_str("\nalso answers to: ");
        description.push_str(&boss.aliases().join(", "));
    }
    for paragraph in [lead.as_deref(), entry.note.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
    {
        description.push_str("\n\n");
        description.push_str(paragraph);
    }
    GuideEmbed {
        title,
        description,
        colour: boss.guide_colour().unwrap_or(DEFAULT_COLOUR),
        thumbnail: None,
        image: None,
    }
}

/// Read one picture and name its upload `<key>-<kind>.<ext>`.
fn picture(
    sources: &dyn GuideSources,
    kind: ArtKind,
    basename: &str,
    key: &str,
) -> Option<Picture> {
    let file = sources.art(kind, basename)?;
    let ext = file.file_name.rsplit_once('.').map_or("", |(_, ext)| ext);
    Some(Picture {
        file_name: format!("{key}-{}.{ext}", kind.as_str()),
        sha256: hash::sha256_hex(&file.bytes),
        bytes: file.bytes,
    })
}
