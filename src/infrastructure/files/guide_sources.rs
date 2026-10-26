//! What guide rendering reads from disk ([`GuideSources`]): boss art from
//! the operator's boss directory (exact case, as cards, at most
//! [`MAX_ART_BYTES`](crate::domain::art::MAX_ART_BYTES)) and each
//! boss's knowledge `lead`. The API preview and the publish worker must
//! render with this one adapter, or their slot hashes (and so the
//! confirmed plan hash) would differ.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{BossArt, read_document};
use crate::domain::art::ArtSource;
use crate::domain::guides::{ArtFile, ArtKind, GuideSources};

/// The boss directory (`KANADE_BOSS_DIR`) and `boss/knowledge/`; either may
/// be absent, which renders without pictures or leads.
#[derive(Clone, Debug, Default)]
pub struct GuideFiles {
    art: Option<BossArt>,
    knowledge_dir: Option<PathBuf>,
}

impl GuideFiles {
    pub fn new(boss_dir: Option<&Path>, knowledge_dir: Option<&Path>) -> Self {
        Self {
            art: boss_dir.map(BossArt::new),
            knowledge_dir: knowledge_dir.map(Path::to_path_buf),
        }
    }
}

impl GuideSources for GuideFiles {
    /// The document is found by its lowercased key and must name exactly
    /// `key`; re-read per render, so an edit shows as "Out of date".
    fn lead(&self, key: &str) -> Option<String> {
        let dir = self.knowledge_dir.as_deref()?;
        let plain = !key.is_empty()
            && key.len() <= 64
            && key.bytes().all(|byte| byte.is_ascii_alphanumeric());
        if !plain {
            return None;
        }
        let doc = read_document(&dir.join(format!("{}.yaml", key.to_ascii_lowercase()))).ok()?;
        if doc.get("boss").and_then(Value::as_str) != Some(key) {
            return None;
        }
        doc.get("lead")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|lead| !lead.is_empty())
            .map(str::to_owned)
    }

    fn art(&self, kind: ArtKind, basename: &str) -> Option<ArtFile> {
        let found = self.art.as_ref()?.find(kind, basename, true)?;
        Some(ArtFile {
            file_name: found.file_name,
            bytes: found.bytes?.into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn reads_exact_case_art_and_the_named_lead() {
        let root = std::env::temp_dir().join(format!("kanade-guide-files-{}", std::process::id()));
        let boss = root.join("boss");
        fs::create_dir_all(boss.join("portraits")).unwrap();
        fs::create_dir_all(boss.join("artwork/entry")).unwrap();
        fs::write(boss.join("portraits/MaleficStar.png"), b"portrait").unwrap();
        let knowledge = root.join("knowledge");
        fs::create_dir_all(&knowledge).unwrap();
        fs::write(
            knowledge.join("maleficstar.yaml"),
            "boss: MaleficStar\nlead: '  Invented lead.  '\n",
        )
        .unwrap();
        fs::write(knowledge.join("other.yaml"), "boss: Someone\nlead: x\n").unwrap();
        let files = GuideFiles::new(Some(&boss), Some(&knowledge));

        let art = files.art(ArtKind::Portrait, "MaleficStar").unwrap();
        assert_eq!(
            (art.file_name.as_str(), &*art.bytes),
            ("MaleficStar.png", &b"portrait"[..])
        );
        assert_eq!(
            files.art(ArtKind::Portrait, "maleficstar"),
            None,
            "exact case"
        );
        assert_eq!(files.art(ArtKind::Entry, "MaleficStar"), None, "missing");
        assert_eq!(files.lead("MaleficStar").as_deref(), Some("Invented lead."));
        assert_eq!(files.lead("Other"), None, "the document names another boss");
        assert_eq!(files.lead("../x"), None);
        assert_eq!(GuideFiles::default().lead("MaleficStar"), None);
        assert_eq!(
            GuideFiles::default().art(ArtKind::Portrait, "MaleficStar"),
            None
        );
    }
}
