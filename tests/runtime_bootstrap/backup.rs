//! `kanade backup` end to end: the binary snapshots a stopped store into the
//! backup directory and refuses while another process owns it.

use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use kanade::infrastructure::store::{BackupManifest, SqliteStore, SqliteStoreConfig};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let path = root.join(format!("kanade-backup-cli-{}", uuid::Uuid::new_v4()));
        for dir in ["", "db", "run", "backups"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(path.join(dir))
                .unwrap();
        }
        Self(path)
    }

    fn store(&self) -> SqliteStoreConfig {
        SqliteStoreConfig {
            db_path: self.0.join("db/kanade.sqlite"),
            owner_lock_dir: self.0.join("run"),
        }
    }

    fn backups(&self) -> PathBuf {
        self.0.join("backups")
    }

    fn backup(&self, args: &[&str]) -> Output {
        let store = self.store();
        Command::new(super::binary())
            .arg("backup")
            .args(args)
            .env_clear()
            .env("KANADE_DB_PATH", &store.db_path)
            .env("KANADE_OWNER_LOCK_DIR", &store.owner_lock_dir)
            .env("KANADE_BACKUP_DIR", self.backups())
            .output()
            .unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test]
async fn backup_writes_a_snapshot_and_manifest_and_never_overwrites() {
    let dir = TempDir::new();
    let store = SqliteStore::open(&dir.store()).await.unwrap();
    let schema = store.schema_version().await.unwrap();
    store.close().await.unwrap();

    let name = "kanade-20261003T070900Z-pre-98c2b31.sqlite";
    let done = dir.backup(&["--name", name]);
    assert_eq!(done.status.code(), Some(0), "{}", text(&done.stderr));
    assert!(
        text(&done.stdout).starts_with(&format!("backup {name}: history head 0 (")),
        "{}",
        text(&done.stdout)
    );
    let snapshot = dir.backups().join(name);
    let manifest = BackupManifest::read(&BackupManifest::path_for(&snapshot)).unwrap();
    assert_eq!(manifest.history_head.seq, 0);
    assert_eq!(manifest.schema_version, schema);
    assert!(manifest.created_at.is_some());
    assert_eq!(
        std::fs::metadata(&snapshot).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let again = dir.backup(&["--name", name]);
    assert_eq!(again.status.code(), Some(78), "{}", text(&again.stderr));
    assert!(text(&again.stderr).contains("choose another --name"));

    // The default name is timestamped.
    let default = dir.backup(&[]);
    assert_eq!(default.status.code(), Some(0), "{}", text(&default.stderr));
    let listed = names(&dir.backups());
    assert_eq!(listed.len(), 4, "{listed:?}");
    assert!(
        listed
            .iter()
            .any(|file| file.starts_with("kanade-2") && file.ends_with("Z.sqlite")),
        "{listed:?}"
    );
}

#[tokio::test]
async fn backup_refuses_while_the_store_is_owned() {
    let dir = TempDir::new();
    let store = SqliteStore::open(&dir.store()).await.unwrap();
    let refused = dir.backup(&["--name", "held.sqlite"]);
    store.close().await.unwrap();
    assert_eq!(refused.status.code(), Some(69), "{}", text(&refused.stderr));
    assert!(text(&refused.stderr).contains("stop the bot first"));
    assert!(names(&dir.backups()).is_empty(), "nothing is written");
}

#[test]
fn backup_never_creates_a_store() {
    let dir = TempDir::new();
    let refused = dir.backup(&[]);
    assert_eq!(refused.status.code(), Some(78), "{}", text(&refused.stderr));
    assert!(text(&refused.stderr).contains("no store at KANADE_DB_PATH"));
    assert!(names(&dir.0.join("db")).is_empty());
    assert!(names(&dir.backups()).is_empty());
}
