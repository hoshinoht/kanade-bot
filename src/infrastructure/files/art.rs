//! Boss art for Discord cards, from the operator's boss directory
//! (`KANADE_BOSS_DIR`: `portraits/`, `artwork/entry/`). Names match exactly,
//! case included, on every platform: the directory listing is compared, so
//! a case-insensitive checkout cannot hide a name CI would miss.

use std::fs;
use std::io::Read;
use std::path::PathBuf;

use serde_json::json;

use crate::bot::delivery::cards::{ArtFile, ArtKind, ArtSource, MAX_ART_BYTES};
use crate::runtime::logging;

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
}

/// Catalog basenames only: ASCII alphanumerics, `_` and `-`.
fn plain(basename: &str) -> bool {
    (1..=64).contains(&basename.len())
        && basename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn kind_name(kind: ArtKind) -> &'static str {
    match kind {
        ArtKind::Portrait => "portrait",
        ArtKind::Entry => "entry",
    }
}

fn skipped(kind: ArtKind, basename: &str, reason: &str) {
    logging::event(
        "WARN",
        "card_art_skipped",
        json!({"kind": kind_name(kind), "boss": basename, "reason": reason}),
    );
}

impl ArtSource for BossArt {
    fn find(&self, kind: ArtKind, basename: &str, read: bool) -> Option<ArtFile> {
        if !plain(basename) {
            return None;
        }
        let dir = self.dir(kind);
        let names: Vec<String> = fs::read_dir(&dir)
            .ok()?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        let file_name = SUFFIXES
            .iter()
            .map(|suffix| format!("{basename}.{suffix}"))
            .find(|name| names.contains(name))?;
        let root = self.root.canonicalize().ok()?;
        let path = dir.join(&file_name).canonicalize().ok()?;
        let meta = fs::metadata(&path).ok()?;
        if !path.starts_with(&root) || !meta.is_file() {
            return None;
        }
        if meta.len() > MAX_ART_BYTES {
            skipped(kind, basename, "too_large");
            return None;
        }
        let bytes = if read {
            let mut bytes = Vec::new();
            let opened = fs::File::open(&path)
                .and_then(|file| file.take(MAX_ART_BYTES + 1).read_to_end(&mut bytes));
            if opened.is_err() || bytes.len() as u64 > MAX_ART_BYTES {
                skipped(kind, basename, "unreadable");
                return None;
            }
            Some(bytes)
        } else {
            None
        };
        Some(ArtFile { file_name, bytes })
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

    fn name(file: Option<ArtFile>) -> Option<String> {
        file.map(|file| file.file_name)
    }

    #[test]
    fn names_match_exactly_including_case() {
        let root = temp();
        fs::write(root.join("portraits/MaleficStar.png"), b"star").unwrap();
        fs::write(root.join("artwork/entry/Kalos.webp"), b"kalos").unwrap();
        let art = BossArt::new(&root);
        assert_eq!(
            art.find(ArtKind::Portrait, "MaleficStar", false),
            Some(ArtFile {
                file_name: "MaleficStar.png".into(),
                bytes: None
            })
        );
        assert_eq!(name(art.find(ArtKind::Portrait, "maleficstar", true)), None);
        assert_eq!(name(art.find(ArtKind::Portrait, "Kalos", true)), None);
        assert_eq!(
            art.find(ArtKind::Entry, "Kalos", true)
                .and_then(|file| file.bytes)
                .as_deref(),
            Some(&b"kalos"[..])
        );
        assert_eq!(
            name(art.find(ArtKind::Portrait, "../portraits/MaleficStar", true)),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_or_missing_art_is_skipped() {
        let root = temp();
        let big = vec![0_u8; usize::try_from(MAX_ART_BYTES).unwrap() + 1];
        fs::write(root.join("portraits/Seren.png"), big).unwrap();
        let fits = vec![0_u8; usize::try_from(MAX_ART_BYTES).unwrap()];
        fs::write(root.join("artwork/entry/Seren.png"), fits).unwrap();
        let art = BossArt::new(&root);
        assert_eq!(art.find(ArtKind::Portrait, "Seren", false), None);
        assert_eq!(art.find(ArtKind::Portrait, "Seren", true), None);
        assert!(
            art.find(ArtKind::Entry, "Seren", true).is_some(),
            "at the cap"
        );
        assert_eq!(
            BossArt::new(root.join("absent")).find(ArtKind::Entry, "Seren", true),
            None
        );
        fs::remove_dir_all(root).unwrap();
    }
}
