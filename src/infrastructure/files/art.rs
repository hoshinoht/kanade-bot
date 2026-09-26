//! Boss art for Discord cards, from the operator's boss directory
//! (`KANADE_BOSS_DIR`: `portraits/`, `artwork/entry/`). Names match exactly,
//! case included, on every platform: the directory listing is compared, so
//! a case-insensitive checkout cannot hide a name CI would miss.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::bot::delivery::cards::{ArtKind, ArtSource, MAX_ART_BYTES};

/// Accepted extensions, in lookup order (as the portal's `/art`).
const SUFFIXES: [&str; 4] = ["png", "webp", "jpg", "jpeg"];

#[derive(Clone, Debug)]
pub struct BossArt {
    root: PathBuf,
}

impl BossArt {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn dir(&self, kind: ArtKind) -> PathBuf {
        match kind {
            ArtKind::Portrait => self.root.join("portraits"),
            ArtKind::Entry => self.root.join("artwork").join("entry"),
        }
    }

    /// `path` if it is a regular file inside the boss directory within bounds.
    fn usable(&self, path: &Path) -> Option<PathBuf> {
        let root = self.root.canonicalize().ok()?;
        let file = path.canonicalize().ok()?;
        let meta = fs::metadata(&file).ok()?;
        (file.starts_with(&root) && meta.is_file() && meta.len() <= MAX_ART_BYTES).then_some(file)
    }
}

/// Catalog basenames only: ASCII alphanumerics, `_` and `-`.
fn plain(basename: &str) -> bool {
    (1..=64).contains(&basename.len())
        && basename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

impl ArtSource for BossArt {
    fn locate(&self, kind: ArtKind, basename: &str) -> Option<String> {
        if !plain(basename) {
            return None;
        }
        let dir = self.dir(kind);
        let names: Vec<String> = fs::read_dir(&dir)
            .ok()?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        SUFFIXES
            .iter()
            .map(|suffix| format!("{basename}.{suffix}"))
            .find(|name| names.contains(name) && self.usable(&dir.join(name)).is_some())
    }

    fn read(&self, kind: ArtKind, file_name: &str) -> Option<Vec<u8>> {
        let (stem, _) = file_name.rsplit_once('.')?;
        if self.locate(kind, stem).as_deref() != Some(file_name) {
            return None;
        }
        let path = self.usable(&self.dir(kind).join(file_name))?;
        let mut bytes = Vec::new();
        fs::File::open(path)
            .ok()?
            .take(MAX_ART_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() as u64 <= MAX_ART_BYTES).then_some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("kanade-art-{}-{n}", std::process::id()));
        fs::create_dir_all(dir.join("portraits")).unwrap();
        fs::create_dir_all(dir.join("artwork/entry")).unwrap();
        dir
    }

    #[test]
    fn names_match_exactly_including_case() {
        let root = temp();
        fs::write(root.join("portraits/MaleficStar.png"), b"star").unwrap();
        fs::write(root.join("artwork/entry/Kalos.webp"), b"kalos").unwrap();
        let art = BossArt::new(&root);
        assert_eq!(
            art.locate(ArtKind::Portrait, "MaleficStar").as_deref(),
            Some("MaleficStar.png")
        );
        assert_eq!(art.locate(ArtKind::Portrait, "maleficstar"), None);
        assert_eq!(art.locate(ArtKind::Portrait, "Kalos"), None);
        assert_eq!(
            art.read(ArtKind::Entry, "Kalos.webp").as_deref(),
            Some(&b"kalos"[..])
        );
        assert_eq!(art.read(ArtKind::Entry, "kalos.webp"), None);
        assert_eq!(
            art.locate(ArtKind::Portrait, "../portraits/MaleficStar"),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_or_missing_art_is_skipped() {
        let root = temp();
        let big = vec![0_u8; usize::try_from(MAX_ART_BYTES).unwrap() + 1];
        fs::write(root.join("portraits/Seren.png"), big).unwrap();
        let art = BossArt::new(&root);
        assert_eq!(art.locate(ArtKind::Portrait, "Seren"), None);
        assert_eq!(art.read(ArtKind::Portrait, "Seren.png"), None);
        assert_eq!(
            BossArt::new(root.join("absent")).locate(ArtKind::Entry, "Seren"),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }
}
