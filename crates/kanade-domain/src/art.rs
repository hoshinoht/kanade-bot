//! The boss art port cards read pictures through; the filesystem adapter is
//! infrastructure (`files::BossArt`).

use super::guides::ArtKind;

/// Largest art file uploaded; bigger files are skipped and logged. The
/// shipped art peaks at ~1.2 MB (entry) and ~0.2 MB (portraits), so a day-of
/// post stays under ~3 MiB: small enough to upload well inside the 10 s
/// attempt timeout, since a timed-out upload is ambiguous and never resent.
pub const MAX_ART_BYTES: u64 = 3 * 512 * 1024;

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
    /// is not a regular file of at most [`MAX_ART_BYTES`]. Call through the
    /// cards' `fetch_art`, which runs it on the blocking pool.
    fn find(&self, kind: ArtKind, basename: &str, read: bool) -> Option<ArtFile>;
}
