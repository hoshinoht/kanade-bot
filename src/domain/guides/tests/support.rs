//! Invented catalog, in-memory art and slot builders.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::super::{
    ArtFile, ArtKind, Block, BossEntry, GuideSources, PublishedSlot, RenderedSlot, SlotState,
    Upload, sha256_hex,
};
use crate::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec, GuideSpec};

pub const CHANNEL: &str = "100000000000000001";
pub const OLD: &str = "100000000000000002";

/// A block id: a valid hyphenated UUID numbered `n`.
pub fn id(n: usize) -> String {
    format!("00000000-0000-4000-8000-{n:012}")
}

fn difficulty(prefix: &str, label: &str) -> DifficultySpec {
    DifficultySpec {
        prefix: prefix.into(),
        label: label.into(),
    }
}

/// `MaleficStar` (mixed case, level, colour, alias), `Kalos` (portrait
/// basename `KalosFace`, no level) and `B0`..`B19` (plain).
pub fn catalog() -> BossTable {
    let mut bosses = vec![
        BossSpec {
            short: "MaleficStar".into(),
            full: Some("Radiant Malefic Star".into()),
            level: Some(280),
            difficulties: Some(vec!["h".into(), "x".into()]),
            aliases: vec!["Shiny".into()],
            guide: Some(GuideSpec {
                colour: Some(0x39BFFF),
            }),
            ..BossSpec::default()
        },
        BossSpec {
            short: "Kalos".into(),
            full: Some("Gatekeeper Kalos".into()),
            portrait: Some("KalosFace".into()),
            ..BossSpec::default()
        },
    ];
    bosses.extend((0..20).map(|n| BossSpec {
        short: format!("B{n}"),
        level: Some(200),
        ..BossSpec::default()
    }));
    BossTable::from_spec(&CatalogSpec {
        difficulties: vec![
            difficulty("n", "Normal"),
            difficulty("h", "Hard"),
            difficulty("x", "Extreme"),
        ],
        bosses,
    })
    .unwrap()
}

/// Art and leads held in memory, looked up exactly as named.
#[derive(Default)]
pub struct Sources {
    pub art: HashMap<(ArtKind, String), ArtFile>,
    pub leads: HashMap<String, String>,
}

impl Sources {
    pub fn with_art(mut self, kind: ArtKind, basename: &str, bytes: Vec<u8>) -> Self {
        self.art.insert(
            (kind, basename.to_owned()),
            ArtFile {
                file_name: format!("{basename}.png"),
                bytes: Arc::from(bytes),
            },
        );
        self
    }

    pub fn with_lead(mut self, key: &str, lead: &str) -> Self {
        self.leads.insert(key.to_owned(), lead.to_owned());
        self
    }
}

impl GuideSources for Sources {
    fn lead(&self, key: &str) -> Option<String> {
        self.leads.get(key).cloned()
    }

    fn art(&self, kind: ArtKind, basename: &str) -> Option<ArtFile> {
        self.art.get(&(kind, basename.to_owned())).cloned()
    }
}

pub fn text(n: usize, content: &str) -> Block {
    Block::Text {
        id: id(n),
        content: content.to_owned(),
    }
}

/// A boss entry; `large_image` adds the entry art below the portrait.
pub fn entry(key: &str, large_image: bool) -> BossEntry {
    BossEntry {
        key: key.to_owned(),
        note: None,
        large_image,
    }
}

pub fn bosses(n: usize, heading: Option<&str>, entries: Vec<BossEntry>) -> Block {
    Block::Bosses {
        id: id(n),
        heading: heading.map(str::to_owned),
        entries,
    }
}

pub fn at() -> DateTime<Utc> {
    DateTime::from_timestamp(1_790_000_000, 0).unwrap()
}

/// A desired slot whose output is named by `hash`, with `files` uploads.
pub fn desired(hash: &str, files: &[&str]) -> RenderedSlot {
    RenderedSlot {
        block_id: id(0),
        part: 0,
        content: hash.to_owned(),
        embeds: Vec::new(),
        uploads: files
            .iter()
            .map(|file| Upload {
                file_name: (*file).to_owned(),
                sha256: sha256_hex(file.as_bytes()),
                bytes: Arc::from(file.as_bytes()),
            })
            .collect(),
        hash: hash.to_owned(),
    }
}

/// A posted slot showing `slot` at `position` in `channel`, message
/// `1000 + position`.
pub fn posted(position: u32, channel: &str, slot: &RenderedSlot) -> PublishedSlot {
    PublishedSlot {
        position,
        channel_id: channel.to_owned(),
        message_id: Some(message(position)),
        hash: slot.hash.clone(),
        uploads: slot.manifest(),
        state: SlotState::Posted,
        source: None,
        publish_id: "run".into(),
        updated_at: at(),
    }
}

pub fn message(position: u32) -> String {
    (1000 + position).to_string()
}

/// `slots` published in order in [`CHANNEL`].
pub fn published(slots: &[RenderedSlot]) -> Vec<PublishedSlot> {
    slots
        .iter()
        .zip(0..)
        .map(|(slot, position)| posted(position, CHANNEL, slot))
        .collect()
}

pub fn with_state(mut slot: PublishedSlot, state: SlotState) -> PublishedSlot {
    if state == SlotState::Unknown {
        slot.message_id = None;
        slot.source = Some("guide-post:op".into());
    }
    slot.state = state;
    slot
}
