//! Boss art on cards (v4 `lead_portrait`, `lead_entry_art`): the lead boss's
//! portrait as the thumbnail and, on day-of cards only, its entry artwork as
//! the image. Files are uploaded with the post and referenced as
//! `attachment://<name>`; missing or unreadable art only drops the picture.

use crate::domain::catalog::BossTable;

/// v4 `IMAGE_PREFIX`: the entry art's attachment name, so it never collides
/// with the portrait's.
pub const IMAGE_PREFIX: &str = "image-";
/// Largest art file uploaded; bigger files are skipped (Discord's default
/// upload limit is 10 MiB per message).
pub const MAX_ART_BYTES: u64 = 4 * 1024 * 1024;

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

/// Where boss art is read from (the configured boss directory in serve).
/// Lookups are exact-case on every platform.
pub trait ArtSource: Send + Sync {
    /// The file name (`<basename>.<ext>`) if the art exists as a regular
    /// file of at most [`MAX_ART_BYTES`].
    fn locate(&self, kind: ArtKind, basename: &str) -> Option<String>;

    /// The bytes of a located file; `None` if it cannot be read in bounds.
    fn read(&self, kind: ArtKind, file_name: &str) -> Option<Vec<u8>>;
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
