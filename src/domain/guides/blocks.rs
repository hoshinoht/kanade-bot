//! A guide draft: an ordered list of blocks, each with a stable id the
//! editor keeps across reorders, stored as one JSON array. Every length is
//! counted in UTF-16 units, as Discord counts message and embed text.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::domain::catalog::BossTable;

/// Most blocks in one guide.
pub const MAX_BLOCKS: usize = 50;
/// The stored JSON array's largest size, in bytes.
pub const MAX_BLOCKS_BYTES: usize = 64 * 1024;
/// Discord's message content limit; also a text block's and a heading's.
pub const CONTENT_LIMIT: usize = 2000;
/// An admin note on one boss entry.
pub const NOTE_LIMIT: usize = 1000;

/// Discord's length of `text`: UTF-16 units (a surrogate pair counts 2).
pub fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// One block of a guide draft. Text and spacer blocks render one message;
/// a bosses block one or more (4.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Block {
    /// Discord Markdown, posted as one message's content.
    Text { id: String, content: String },
    /// Boss embeds, one per entry; `heading` is the first message's content.
    Bosses {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        heading: Option<String>,
        entries: Vec<BossEntry>,
    },
    /// A visual gap: one message whose content is a zero-width space.
    Spacer { id: String },
}

impl Block {
    /// The block's stable id (a hyphenated UUID).
    pub fn id(&self) -> &str {
        match self {
            Self::Text { id, .. } | Self::Bosses { id, .. } | Self::Spacer { id } => id,
        }
    }
}

/// One boss in a bosses block (decision 17).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BossEntry {
    /// The catalog short name, exact case (e.g. `MaleficStar`).
    pub key: String,
    /// The admin's note, appended to the embed description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Also show the boss's entry artwork as the large image (approved
    /// boards, default 4). The portrait thumbnail is always shown when the
    /// art exists; there is no "no picture" choice. The older `image` field
    /// is refused as unknown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub large_image: bool,
}

/// Why a draft (or its rendering) is refused, naming the block and field.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BlockError {
    #[error("a guide has at most {limit} blocks, not {count}")]
    TooManyBlocks { count: usize, limit: usize },
    #[error("the blocks take {bytes} bytes; at most {limit}")]
    TooLarge { bytes: usize, limit: usize },
    #[error("block id {block_id:?} is not a UUID")]
    BadId { block_id: String },
    #[error("block id {block_id} is used twice")]
    DuplicateId { block_id: String },
    #[error("block {block_id}: {field} is empty")]
    Empty { block_id: String, field: String },
    #[error("block {block_id}: {field} is {units} units; at most {limit}")]
    TooLong {
        block_id: String,
        field: String,
        units: usize,
        limit: usize,
    },
    #[error("block {block_id}: {field} names unknown boss {key:?}")]
    UnknownBoss {
        block_id: String,
        field: String,
        key: String,
    },
    #[error("the guide renders {count} messages; at most {limit}")]
    TooManySlots { count: usize, limit: usize },
}

/// Every problem with `blocks` against `catalog`, in block order; empty
/// when the draft may be saved. Boss keys match the catalog exactly.
pub fn validate(blocks: &[Block], catalog: &BossTable) -> Vec<BlockError> {
    let mut errors = Vec::new();
    if blocks.len() > MAX_BLOCKS {
        errors.push(BlockError::TooManyBlocks {
            count: blocks.len(),
            limit: MAX_BLOCKS,
        });
    }
    let bytes = serde_json::to_vec(blocks).map_or(usize::MAX, |json| json.len());
    if bytes > MAX_BLOCKS_BYTES {
        errors.push(BlockError::TooLarge {
            bytes,
            limit: MAX_BLOCKS_BYTES,
        });
    }
    let mut seen = HashSet::new();
    for block in blocks {
        let block_id = block.id();
        if block_id.len() != 36 || uuid::Uuid::try_parse(block_id).is_err() {
            errors.push(BlockError::BadId {
                block_id: block_id.to_owned(),
            });
        } else if !seen.insert(block_id) {
            errors.push(BlockError::DuplicateId {
                block_id: block_id.to_owned(),
            });
        }
        match block {
            Block::Text { content, .. } => text(&mut errors, block_id, "content", content),
            Block::Bosses {
                heading, entries, ..
            } => {
                if let Some(heading) = heading {
                    text(&mut errors, block_id, "heading", heading);
                }
                if entries.is_empty() {
                    errors.push(empty(block_id, "entries"));
                }
                for (at, entry) in entries.iter().enumerate() {
                    if catalog.boss(&entry.key).is_none() {
                        errors.push(BlockError::UnknownBoss {
                            block_id: block_id.to_owned(),
                            field: format!("entries[{at}].key"),
                            key: entry.key.clone(),
                        });
                    }
                    if let Some(note) = &entry.note {
                        let field = format!("entries[{at}].note");
                        if note.trim().is_empty() {
                            errors.push(empty(block_id, &field));
                        }
                        limit(&mut errors, block_id, &field, note, NOTE_LIMIT);
                    }
                }
            }
            Block::Spacer { .. } => {}
        }
    }
    errors
}

/// Message content: not blank (Discord refuses whitespace-only content; a
/// gap is a spacer block) and within [`CONTENT_LIMIT`].
fn text(errors: &mut Vec<BlockError>, block_id: &str, field: &str, value: &str) {
    if value.trim().is_empty() {
        errors.push(empty(block_id, field));
    }
    limit(errors, block_id, field, value, CONTENT_LIMIT);
}

fn empty(block_id: &str, field: &str) -> BlockError {
    BlockError::Empty {
        block_id: block_id.to_owned(),
        field: field.to_owned(),
    }
}

pub(super) fn limit(
    errors: &mut Vec<BlockError>,
    block_id: &str,
    field: &str,
    value: &str,
    limit: usize,
) {
    let units = units(value);
    if units > limit {
        errors.push(BlockError::TooLong {
            block_id: block_id.to_owned(),
            field: field.to_owned(),
            units,
            limit,
        });
    }
}
