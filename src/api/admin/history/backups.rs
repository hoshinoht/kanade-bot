//! Backup manifests in `KANADE_BACKUP_DIR` for the checkpoints view. The
//! directory is read afresh on every request ("Verify again" refetches), and
//! the handler re-checks each manifest's head against the chain.
//!
//! Only `<file>.manifest.json` beside an existing regular `<file>` is listed;
//! anything else (volume tarballs, hidden staging directories) is ignored.
//! Unreadable or foreign manifests and manifests whose snapshot is gone are
//! skipped with one counting WARN, never a failed page.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::{
    api::dto::{
        history::{BackupAnchor, BackupRow},
        iso_instant,
    },
    infrastructure::store::BackupManifest,
    runtime::logging,
};

const SUFFIX: &str = ".manifest.json";
/// Directory entries examined per request.
const MAX_ENTRIES: usize = 4096;
/// Backups listed (newest first) and anchor-checked per request.
const MAX_LISTED: usize = 100;
/// Manifests are ~200 bytes; anything far larger is not one.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Found {
    pub file: String,
    pub manifest: BackupManifest,
    /// The manifest's `created_at`, else the snapshot's mtime.
    pub created_at: DateTime<Utc>,
}

enum Skip {
    Unreadable,
    MissingSnapshot,
}

fn dir_unreadable() {
    logging::event("WARN", "backup_dir_unreadable", json!({}));
}

/// [`scan`] on the blocking pool; a failed scan task lists nothing.
pub(super) async fn list(dir: PathBuf) -> Vec<Found> {
    match tokio::task::spawn_blocking(move || scan(&dir)).await {
        Ok(found) => found,
        Err(_) => {
            dir_unreadable();
            Vec::new()
        }
    }
}

/// The listed backups, newest first. Blocking: run off the async workers.
pub(super) fn scan(dir: &Path) -> Vec<Found> {
    let Ok(entries) = fs::read_dir(dir) else {
        dir_unreadable();
        return Vec::new();
    };
    let (mut found, mut unreadable, mut missing, mut truncated) = (Vec::new(), 0, 0, false);
    for (index, entry) in entries.enumerate() {
        if index == MAX_ENTRIES {
            truncated = true;
            break;
        }
        let Ok(entry) = entry else {
            unreadable += 1;
            continue;
        };
        let name = entry.file_name();
        let Some(file) = name
            .to_str()
            .filter(|name| !name.starts_with('.'))
            .and_then(|name| name.strip_suffix(SUFFIX))
            .filter(|file| !file.is_empty())
        else {
            continue;
        };
        match read(dir, file) {
            Ok(backup) => found.push(backup),
            Err(Skip::Unreadable) => unreadable += 1,
            Err(Skip::MissingSnapshot) => missing += 1,
        }
    }
    if unreadable > 0 || missing > 0 || truncated {
        // Counts only: names and error text may carry paths.
        logging::event(
            "WARN",
            "backup_manifests_skipped",
            json!({"unreadable": unreadable, "missing_snapshot": missing, "truncated": truncated}),
        );
    }
    found.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.file.cmp(&a.file))
    });
    found.truncate(MAX_LISTED);
    found
}

fn read(dir: &Path, file: &str) -> Result<Found, Skip> {
    let path = dir.join(format!("{file}{SUFFIX}"));
    let meta = fs::symlink_metadata(&path).map_err(|_| Skip::Unreadable)?;
    if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
        return Err(Skip::Unreadable);
    }
    let manifest = BackupManifest::read(&path).map_err(|_| Skip::Unreadable)?;
    let snapshot = fs::symlink_metadata(dir.join(file))
        .ok()
        .filter(fs::Metadata::is_file)
        .ok_or(Skip::MissingSnapshot)?;
    let created_at = manifest
        .created_at
        .or_else(|| snapshot.modified().ok().and_then(utc))
        .unwrap_or(DateTime::UNIX_EPOCH);
    Ok(Found {
        file: file.to_owned(),
        manifest,
        created_at,
    })
}

fn utc(at: SystemTime) -> Option<DateTime<Utc>> {
    let since = at.duration_since(UNIX_EPOCH).ok()?;
    DateTime::from_timestamp(i64::try_from(since.as_secs()).ok()?, 0)
}

/// `matches` (the chain holds the head), `older_schema` (it does, but the
/// backup predates this store's schema) or `mismatch` (the head is not in
/// the chain: history truncated or forked since).
pub(super) fn anchor(anchored: bool, backup_schema: i64, store_schema: i64) -> BackupAnchor {
    match (anchored, backup_schema < store_schema) {
        (false, _) => BackupAnchor::Mismatch,
        (true, true) => BackupAnchor::OlderSchema,
        (true, false) => BackupAnchor::Matches,
    }
}

pub(super) fn row(backup: &Found, anchored: bool, store_schema: i64) -> BackupRow {
    let manifest = &backup.manifest;
    BackupRow {
        file: backup.file.clone(),
        format: crate::infrastructure::store::sqlite::BACKUP_MANIFEST_FORMAT,
        created_at: iso_instant(backup.created_at),
        history_head: (&manifest.history_head).into(),
        revision: manifest.revision,
        schema_version: manifest.schema_version,
        anchored,
        anchor: anchor(anchored, manifest.schema_version, store_schema),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatch_outranks_an_older_schema() {
        assert_eq!(anchor(true, 21, 21), BackupAnchor::Matches);
        assert_eq!(anchor(true, 22, 21), BackupAnchor::Matches);
        assert_eq!(anchor(true, 20, 21), BackupAnchor::OlderSchema);
        assert_eq!(anchor(false, 20, 21), BackupAnchor::Mismatch);
        assert_eq!(anchor(false, 21, 21), BackupAnchor::Mismatch);
    }

    #[test]
    fn skipped_manifests_are_counted_without_names_or_paths() {
        let root = std::env::temp_dir().join(format!("kanade-backups-{}", uuid::Uuid::new_v4()));
        let dir = root.join("backups");
        fs::create_dir_all(&dir).unwrap();
        let valid = r#"{"format":"kanade.backup.v1","history_head":{"seq":1,"hash":"ab"},"revision":2,"schema_version":21,"created_at":"2026-10-03T00:00:00+00:00"}"#;
        fs::write(dir.join("ok.sqlite"), b"s").unwrap();
        fs::write(dir.join("ok.sqlite.manifest.json"), valid).unwrap();
        fs::write(dir.join("bad.sqlite"), b"s").unwrap();
        fs::write(dir.join("bad.sqlite.manifest.json"), "not json").unwrap();
        fs::write(dir.join("gone.sqlite.manifest.json"), valid).unwrap();
        fs::write(root.join("m.json"), valid).unwrap();
        fs::write(dir.join("link.sqlite"), b"s").unwrap();
        std::os::unix::fs::symlink(root.join("m.json"), dir.join("link.sqlite.manifest.json"))
            .unwrap();
        fs::write(root.join("s.sqlite"), b"s").unwrap();
        fs::write(dir.join("snap.sqlite.manifest.json"), valid).unwrap();
        std::os::unix::fs::symlink(root.join("s.sqlite"), dir.join("snap.sqlite")).unwrap();

        logging::capture();
        let found = scan(&dir);
        let lines = logging::captured();
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            found.iter().map(|b| b.file.as_str()).collect::<Vec<_>>(),
            ["ok.sqlite"]
        );
        assert_eq!(
            lines,
            [json!({
                "level": "WARN",
                "event": "backup_manifests_skipped",
                "unreadable": 2,
                "missing_snapshot": 2,
                "truncated": false,
            })]
        );
    }

    #[test]
    fn a_missing_dir_lists_nothing_and_warns() {
        logging::capture();
        let dir = std::env::temp_dir().join(format!("kanade-no-backups-{}", uuid::Uuid::new_v4()));
        assert!(scan(&dir).is_empty());
        let lines = logging::captured();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["event"], "backup_dir_unreadable");
        assert_eq!(lines[0].as_object().map(serde_json::Map::len), Some(2));
    }
}
