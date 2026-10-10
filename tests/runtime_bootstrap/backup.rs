//! `kanade backup` end to end: the binary snapshots a stopped store into the
//! backup directory and refuses while another process owns it; with a
//! recipients file it publishes only the age-encrypted snapshot, and the
//! `keygen`/`encrypt`/`decrypt` tools round-trip through the same binary.

use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use kanade::infrastructure::store::{BackupManifest, SqliteStore, SqliteStoreConfig};
use sqlx::{ConnectOptions, sqlite::SqliteConnectOptions};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let path = root.join(format!("kanade-backup-cli-{}", uuid::Uuid::new_v4()));
        for dir in ["", "db", "run", "backups", "keys", "restore"] {
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
        self.kanade(&[&["backup"], args].concat()).output().unwrap()
    }

    /// The binary with only the store and backup paths set.
    fn kanade(&self, args: &[&str]) -> Command {
        let store = self.store();
        let mut command = Command::new(super::binary());
        command
            .args(args)
            .env_clear()
            .env("KANADE_DB_PATH", &store.db_path)
            .env("KANADE_OWNER_LOCK_DIR", &store.owner_lock_dir)
            .env("KANADE_BACKUP_DIR", self.backups());
        command
    }

    /// `backup keygen` into `keys/<name>`; returns (identity, recipients file).
    fn keygen(&self, name: &str) -> (PathBuf, PathBuf) {
        let identity = self.0.join("keys").join(name);
        let made = self.backup(&["keygen", "--out", identity.to_str().unwrap()]);
        assert_eq!(made.status.code(), Some(0), "{}", text(&made.stderr));
        let public = text(&made.stdout);
        assert!(
            public.starts_with("age1") && public.ends_with('\n'),
            "{public}"
        );
        let recipients = self.0.join("keys").join(format!("{name}.pub"));
        std::fs::write(&recipients, format!("# backup key\n{public}")).unwrap();
        (identity, recipients)
    }

    fn decrypt(&self, identity: &Path, input: &Path, output: &Path) -> Output {
        let path = |path: &Path| path.to_str().unwrap().to_owned();
        self.backup(&[
            "decrypt",
            "--identity",
            &path(identity),
            "--in",
            &path(input),
            "--out",
            &path(output),
        ])
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
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

#[tokio::test]
async fn an_encrypted_backup_decrypts_to_a_valid_snapshot() {
    let dir = TempDir::new();
    let store = SqliteStore::open(&dir.store()).await.unwrap();
    let schema = store.schema_version().await.unwrap();
    store.close().await.unwrap();
    let (identity, recipients) = dir.keygen("backup.key");
    assert_eq!(mode(&identity), 0o600);

    let name = "kanade-20261011T010203Z-pre-abc1234.sqlite";
    let done = dir
        .kanade(&["backup", "--name", name])
        .env("KANADE_BACKUP_RECIPIENTS_FILE", &recipients)
        .output()
        .unwrap();
    assert_eq!(done.status.code(), Some(0), "{}", text(&done.stderr));
    assert!(
        text(&done.stdout).starts_with(&format!("backup {name}.age: history head 0 (")),
        "{}",
        text(&done.stdout)
    );
    // Only the ciphertext and its plaintext manifest: no snapshot, no staging.
    let sealed = dir.backups().join(format!("{name}.age"));
    assert_eq!(
        names(&dir.backups()),
        [format!("{name}.age"), format!("{name}.age.manifest.json")]
    );
    assert_eq!(mode(&sealed), 0o600);
    assert!(
        std::fs::read(&sealed)
            .unwrap()
            .starts_with(b"age-encryption.org/v1\n")
    );
    let manifest = BackupManifest::read(&BackupManifest::path_for(&sealed)).unwrap();
    assert_eq!(manifest.schema_version, schema);
    assert!(manifest.created_at.is_some());

    let plain = dir.0.join("restore").join(name);
    let opened = dir.decrypt(&identity, &sealed, &plain);
    assert_eq!(opened.status.code(), Some(0), "{}", text(&opened.stderr));
    assert_eq!(mode(&plain), 0o600);
    let mut conn = SqliteConnectOptions::new()
        .filename(&plain)
        .read_only(true)
        .connect()
        .await
        .unwrap();
    let check: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&mut conn)
        .await
        .unwrap();
    assert_eq!(check, ["ok"]);
    let restored: i64 = sqlx::query_scalar("SELECT schema_version FROM store_meta WHERE id = 1")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(restored, schema);
    drop(conn);

    // Never overwrites, even with the right identity.
    let before = std::fs::read(&plain).unwrap();
    let again = dir.decrypt(&identity, &sealed, &plain);
    assert_eq!(again.status.code(), Some(78), "{}", text(&again.stderr));
    assert!(text(&again.stderr).contains("--out already exists; it is never overwritten"));
    assert_eq!(std::fs::read(&plain).unwrap(), before);

    // Another key pair's identity cannot open it, and writes nothing.
    let (stranger, _) = dir.keygen("stranger.key");
    let elsewhere = dir.0.join("restore/stranger.sqlite");
    let refused = dir.decrypt(&stranger, &sealed, &elsewhere);
    assert_eq!(refused.status.code(), Some(78), "{}", text(&refused.stderr));
    assert!(
        text(&refused.stderr).contains("the identity matches none of this file's recipients"),
        "{}",
        text(&refused.stderr)
    );
    assert!(!elsewhere.exists());
}

#[tokio::test]
async fn a_bad_recipients_file_refuses_the_backup() {
    let dir = TempDir::new();
    SqliteStore::open(&dir.store())
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let (identity, _) = dir.keygen("backup.key");
    let recipients = dir.0.join("keys/bad.pub");
    for (contents, expected) in [
        (
            "# nothing here\n\n".to_owned(),
            "KANADE_BACKUP_RECIPIENTS_FILE holds no age recipients",
        ),
        (
            "age1notakey\n".to_owned(),
            "KANADE_BACKUP_RECIPIENTS_FILE: line 1 is not an age X25519 recipient",
        ),
        // The private identity pasted in by mistake: refused, never echoed.
        (
            std::fs::read_to_string(&identity).unwrap(),
            "is a private age identity",
        ),
    ] {
        std::fs::write(&recipients, &contents).unwrap();
        let refused = dir
            .kanade(&["backup"])
            .env("KANADE_BACKUP_RECIPIENTS_FILE", &recipients)
            .output()
            .unwrap();
        assert_eq!(refused.status.code(), Some(78), "{}", text(&refused.stderr));
        let stderr = text(&refused.stderr);
        assert!(stderr.contains(expected), "{stderr}");
        assert!(!stderr.contains("AGE-SECRET-KEY"), "{stderr}");
        assert!(names(&dir.backups()).is_empty(), "nothing is written");
    }
    let missing = dir
        .kanade(&["backup"])
        .env("KANADE_BACKUP_RECIPIENTS_FILE", dir.0.join("keys/absent"))
        .output()
        .unwrap();
    assert_eq!(missing.status.code(), Some(78));
    assert!(
        text(&missing.stderr)
            .contains("KANADE_BACKUP_RECIPIENTS_FILE must name a readable recipients file")
    );
    assert!(names(&dir.backups()).is_empty());
}

/// `backup encrypt …` with `input` on stdin; returns its output.
fn encrypt(mut command: Command, input: Vec<u8>) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output().unwrap();
    // A refusal exits before reading stdin (a broken pipe here); successful
    // runs are checked by their round trip.
    let _ = writer.join().unwrap();
    output
}

#[test]
fn the_encrypt_filter_round_trips_arbitrary_bytes() {
    let dir = TempDir::new();
    let (identity, recipients) = dir.keygen("volume.key");
    // Every byte value, past several 64 KiB age chunks, not a multiple of one.
    let input: Vec<u8> = (0..300_001u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let recipients_arg = recipients.to_str().unwrap();
    let by_flag = encrypt(
        dir.kanade(&["backup", "encrypt", "--recipients", recipients_arg]),
        input.clone(),
    );
    let mut by_setting = dir.kanade(&["backup", "encrypt"]);
    by_setting.env("KANADE_BACKUP_RECIPIENTS_FILE", &recipients);
    let by_setting = encrypt(by_setting, input.clone());
    for (index, sealed) in [by_flag, by_setting].into_iter().enumerate() {
        assert_eq!(sealed.status.code(), Some(0), "{}", text(&sealed.stderr));
        assert!(sealed.stdout.starts_with(b"age-encryption.org/v1\n"));
        let tarball = dir.0.join(format!("keys/volume-{index}.tar.gz.age"));
        std::fs::write(&tarball, &sealed.stdout).unwrap();
        let plain = dir.0.join(format!("restore/volume-{index}.tar.gz"));
        let opened = dir.decrypt(&identity, &tarball, &plain);
        assert_eq!(opened.status.code(), Some(0), "{}", text(&opened.stderr));
        assert_eq!(std::fs::read(&plain).unwrap(), input);
    }

    // No recipients at all, or a bad file: refused before any output.
    let none = encrypt(dir.kanade(&["backup", "encrypt"]), b"x".to_vec());
    assert_eq!(none.status.code(), Some(64), "{}", text(&none.stderr));
    assert!(none.stdout.is_empty());
    let empty = dir.0.join("keys/empty.pub");
    std::fs::write(&empty, "").unwrap();
    let refused = encrypt(
        dir.kanade(&["backup", "encrypt", "--recipients", empty.to_str().unwrap()]),
        b"x".to_vec(),
    );
    assert_eq!(refused.status.code(), Some(78), "{}", text(&refused.stderr));
    assert!(text(&refused.stderr).contains("--recipients holds no age recipients"));
    assert!(refused.stdout.is_empty());
}
