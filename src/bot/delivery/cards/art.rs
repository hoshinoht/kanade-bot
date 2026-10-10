//! Boss art on cards (v4 `lead_portrait`, `lead_entry_art`): the lead boss's
//! portrait as the thumbnail and, on day-of cards only, its entry artwork as
//! the image. Files are uploaded with the post and referenced as
//! `attachment://<name>`; missing, oversized or unreadable art only drops
//! the picture. Lookups block on the filesystem, so they run off the async
//! runtime ([`fetch_art`]), once per picture.

use std::sync::Arc;

use super::Card;
use crate::domain::catalog::BossTable;

/// v4 `IMAGE_PREFIX`: the entry art's attachment name, so it never collides
/// with the portrait's.
pub const IMAGE_PREFIX: &str = "image-";
/// Largest art file uploaded; bigger files are skipped and logged. The
/// shipped art peaks at ~1.2 MB (entry) and ~0.2 MB (portraits), so a day-of
/// post stays under ~3 MiB: small enough to upload well inside the 10 s
/// attempt timeout, since a timed-out upload is ambiguous and never resent.
pub const MAX_ART_BYTES: u64 = 3 * 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtKind {
    /// `boss/portraits/<portrait or key>.<ext>`.
    Portrait,
    /// `boss/artwork/entry/<key>.<ext>`.
    Entry,
}

/// One picture a card wants: its kind and the catalog basename (exact case).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtRef {
    pub kind: ArtKind,
    pub basename: String,
}

/// One located picture: its file name (`<basename>.<ext>`) and, when read,
/// its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtFile {
    pub file_name: String,
    pub bytes: Option<Vec<u8>>,
}

/// Where boss art is read from (the configured boss directory in serve).
/// Lookups are exact-case on every platform.
pub trait ArtSource: Send + Sync {
    /// Blocking: resolve the art once and, with `read`, read it. `None` if it
    /// is not a regular file of at most [`MAX_ART_BYTES`]. Call through
    /// [`fetch_art`].
    fn find(&self, kind: ArtKind, basename: &str, read: bool) -> Option<ArtFile>;
}

/// A picture resolved for one card: its attachment name and, for a post,
/// its bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Picture {
    pub attachment: String,
    pub bytes: Option<Arc<[u8]>>,
}

/// One embed's thumbnail and image, resolved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmbedArt {
    pub thumbnail: Option<Picture>,
    pub image: Option<Picture>,
}

/// The card's pictures, resolved, one entry per embed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardArt {
    pub embeds: Vec<EmbedArt>,
}

/// Resolve (and with `read`, read) the card's pictures on the blocking pool,
/// each distinct picture once. Any failure leaves the picture off.
pub async fn fetch_art(art: Option<&Arc<dyn ArtSource>>, card: &Card, read: bool) -> CardArt {
    let Some(art) = art.cloned() else {
        return CardArt::default();
    };
    let mut wanted: Vec<ArtRef> = Vec::new();
    for embed in &card.embeds {
        for picture in [&embed.thumbnail, &embed.image].into_iter().flatten() {
            if !wanted.contains(picture) {
                wanted.push(picture.clone());
            }
        }
    }
    if wanted.is_empty() {
        return CardArt::default();
    }
    let found = tokio::task::spawn_blocking(move || {
        wanted
            .into_iter()
            .map(|wanted| {
                let picture = art
                    .find(wanted.kind, &wanted.basename, read)
                    .filter(|file| !read || file.bytes.is_some())
                    .map(|file| Picture {
                        attachment: attachment_name(wanted.kind, &file.file_name),
                        bytes: file.bytes.map(Arc::from),
                    });
                (wanted, picture)
            })
            .collect::<Vec<_>>()
    })
    .await;
    let Ok(found) = found else {
        return CardArt::default();
    };
    let picture = |wanted: &Option<ArtRef>| {
        let wanted = wanted.as_ref()?;
        found
            .iter()
            .find(|(art, _)| art == wanted)
            .and_then(|(_, picture)| picture.clone())
    };
    CardArt {
        embeds: card
            .embeds
            .iter()
            .map(|embed| EmbedArt {
                thumbnail: picture(&embed.thumbnail),
                image: picture(&embed.image),
            })
            .collect(),
    }
}

/// The lead boss's portrait: the catalog `portrait` basename, else its key.
pub fn lead_portrait(bosses: &[String], catalog: Option<&BossTable>) -> Option<ArtRef> {
    let (_, boss) = catalog?.split(bosses.first()?)?;
    Some(ArtRef {
        kind: ArtKind::Portrait,
        basename: boss.portrait().unwrap_or(boss.short()).to_owned(),
    })
}

/// The lead boss's entry artwork, named by its key (v4 `entry_art_path`).
pub fn lead_entry_art(bosses: &[String], catalog: Option<&BossTable>) -> Option<ArtRef> {
    let (_, boss) = catalog?.split(bosses.first()?)?;
    Some(ArtRef {
        kind: ArtKind::Entry,
        basename: boss.short().to_owned(),
    })
}

/// The attachment name a located file travels under.
pub fn attachment_name(kind: ArtKind, file_name: &str) -> String {
    match kind {
        ArtKind::Portrait => file_name.to_owned(),
        ArtKind::Entry => format!("{IMAGE_PREFIX}{file_name}"),
    }
}
