//! `kanade restore` end to end: refusals (a live owner, garbage, a corrupt
//! file, a newer schema, a missing identity) leave the store as it was and
//! stage nothing; a dry run changes nothing; `--apply` swaps the snapshot
//! in and keeps the previous store beside it; an `.age` snapshot restores
//! with its identity only. Also `kanade token rotate`.

use std::{
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Output, Stdio},
};

use chrono::{DateTime, Utc};
use kanade::infrastructure::store::{SqliteStore, SqliteStoreConfig};
use kanade_domain::model_log::{ExtractionLog, ExtractionOutcome, ModelLogStore};
use sqlx::{ConnectOptions, Connection, sqlite::SqliteConnectOptions};

use super::backup::{TempDir, mode, names, text};

fn extraction(id: &str) -> ExtractionLog {
    ExtractionLog {
        reasoning_content: None,
        reasoning_tokens: None,
        id: id.into(),
        at: DateTime::<Utc>::from_timestamp(1_791_590_400, 0).unwrap(),
        channel_id: Some("900".into()),
        member_ids: Vec::new(),
        model: "kanata/extract".into(),
        reasoning: None,
        prompt: "Messages: invented".into(),
        raw_response: "{\"changes\": []}".into(),
        latency_ms: Some(10),
        request_count: 1,
        outcome: ExtractionOutcome::NoChange,
        error: None,
        guardrail: serde_json::json!({}),
        message_ids: Vec::new(),
        proposal_ids: Vec::new(),
        refusals: Vec::new(),
        prompt_tokens: None,
        completion_tokens: None,
        cached_tokens: None,
        prompt_estimate: None,
        request_ids: Vec::new(),
        session_id: None,
    }
}

/// Open `config`, record a marker row, close.
async fn mark(config: &SqliteStoreConfig, id: &str) {
    let store = SqliteStore::open(config).await.unwrap();
    store.record_extraction(extraction(id)).await.unwrap();
    store.close().await.unwrap();
}

/// Which of `ids` the store at `config` holds.
async fn holds(config: &SqliteStoreConfig, ids: &[&str]) -> Vec<String> {
    let store = SqliteStore::open(config).await.unwrap();
    let mut found = Vec::new();
    for id in ids {
        if store.load_extraction(id).await.unwrap().is_some() {
            found.push((*id).to_owned());
        }
    }
    store.close().await.unwrap();
    found
}

/// A store holding `snap`, its plain snapshot `snap.sqlite` in backups,
/// then `later` recorded in the live store only.
async fn store_with_snapshot(dir: &TempDir) -> PathBuf {
    mark(&dir.store(), "snap").await;
    let made = dir.backup(&["--name", "snap.sqlite"]);
    assert_eq!(made.status.code(), Some(0), "{}", text(&made.stderr));
    mark(&dir.store(), "later").await;
    dir.backups().join("snap.sqlite")
}

fn restore(dir: &TempDir, args: &[&str]) -> Output {
    dir.kanade(&[&["restore"], args].concat()).output().unwrap()
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn db_dir(dir: &TempDir) -> PathBuf {
    dir.0.join("db")
}

#[tokio::test]
async fn restore_refuses_a_live_owner_before_staging_anything() {
    let dir = TempDir::new();
    let snapshot = store_with_snapshot(&dir).await;
    let owner = SqliteStore::open(&dir.store()).await.unwrap();
    let before = names(&db_dir(&dir));
    let refused = restore(&dir, &["--from", path(&snapshot), "--apply"]);
    assert_eq!(names(&db_dir(&dir)), before, "nothing staged or kept");
    owner.close().await.unwrap();
    assert_eq!(refused.status.code(), Some(69), "{}", text(&refused.stderr));
    assert!(text(&refused.stderr).contains("stop the bot first"));
    assert_eq!(
        holds(&dir.store(), &["snap", "later"]).await,
        ["snap", "later"]
    );
}

#[tokio::test]
async fn restore_refuses_invalid_corrupt_and_newer_snapshots_before_any_write() {
    let dir = TempDir::new();
    let snapshot = store_with_snapshot(&dir).await;
    let bad = dir.0.join("restore");

    let garbage = bad.join("garbage.sqlite");
    std::fs::write(&garbage, b"not a database at all, just bytes").unwrap();

    let corrupt = bad.join("corrupt.sqlite");
    let mut bytes = std::fs::read(&snapshot).unwrap();
    assert!(bytes.len() > 3 * 4096, "a multi-page snapshot");
    for byte in &mut bytes[4096..3 * 4096] {
        *byte = 0xA5;
    }
    std::fs::write(&corrupt, bytes).unwrap();

    let newer = bad.join("newer.sqlite");
    std::fs::copy(&snapshot, &newer).unwrap();
    let mut conn = SqliteConnectOptions::new()
        .filename(&newer)
        .connect()
        .await
        .unwrap();
    sqlx::query("INSERT INTO schema_migrations (version, checksum, applied_at) VALUES (9999, 'x', '2099-01-01T00:00:00Z')")
        .execute(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();

    let link = bad.join("link.sqlite");
    std::os::unix::fs::symlink(&snapshot, &link).unwrap();

    let before = names(&db_dir(&dir));
    for (input, code, says) in [
        (&garbage, 78, "not a usable snapshot"),
        (&corrupt, 78, "not a usable snapshot"),
        (&newer, 78, "newer than this build"),
        (&link, 78, "symbolic links are refused"),
    ] {
        let refused = restore(&dir, &["--from", path(input), "--apply"]);
        let stderr = text(&refused.stderr);
        assert_eq!(refused.status.code(), Some(code), "{input:?}: {stderr}");
        assert!(stderr.contains(says), "{input:?}: {stderr}");
        assert_eq!(
            names(&db_dir(&dir)),
            before,
            "{input:?}: nothing staged or kept"
        );
    }
    let encrypted = bad.join("snap.sqlite.age");
    std::fs::write(&encrypted, b"age-encryption.org/v1\n").unwrap();
    let refused = restore(&dir, &["--from", path(&encrypted)]);
    assert_eq!(refused.status.code(), Some(64), "{}", text(&refused.stderr));
    assert!(text(&refused.stderr).contains("pass --identity"));
    assert_eq!(
        holds(&dir.store(), &["snap", "later"]).await,
        ["snap", "later"]
    );
}

#[tokio::test]
async fn a_dry_run_changes_nothing_and_apply_swaps_keeping_the_previous_store() {
    let dir = TempDir::new();
    let snapshot = store_with_snapshot(&dir).await;
    let before = names(&db_dir(&dir));

    let dry = restore(&dir, &["--from", path(&snapshot)]);
    assert_eq!(dry.status.code(), Some(0), "{}", text(&dry.stderr));
    let stdout = text(&dry.stdout);
    assert!(stdout.contains("valid snapshot, schema "), "{stdout}");
    assert!(stdout.contains("dry run: nothing changed"), "{stdout}");
    assert_eq!(names(&db_dir(&dir)), before);
    assert_eq!(
        holds(&dir.store(), &["snap", "later"]).await,
        ["snap", "later"]
    );

    let applied = restore(&dir, &["--from", path(&snapshot), "--apply"]);
    assert_eq!(applied.status.code(), Some(0), "{}", text(&applied.stderr));
    let stdout = text(&applied.stdout);
    assert!(
        stdout.contains("restored: KANADE_DB_PATH now holds the snapshot"),
        "{stdout}"
    );
    let listed = names(&db_dir(&dir));
    let side: Vec<_> = listed
        .iter()
        .filter(|name| name.starts_with("kanade.sqlite.pre-restore-") && name.ends_with('Z'))
        .collect();
    assert_eq!(side.len(), 1, "{listed:?}");
    assert!(stdout.contains(side[0].as_str()), "{stdout}");
    assert!(
        listed.iter().all(|name| !name.contains(".restore-")),
        "staging removed: {listed:?}"
    );
    let db = dir.store().db_path;
    assert_eq!(mode(&db), 0o600);
    assert_eq!(holds(&dir.store(), &["snap", "later"]).await, ["snap"]);
    let kept = SqliteStoreConfig {
        db_path: db_dir(&dir).join(side[0]),
        owner_lock_dir: dir.store().owner_lock_dir,
    };
    assert_eq!(holds(&kept, &["snap", "later"]).await, ["snap", "later"]);
}

#[tokio::test]
async fn restore_into_an_empty_store_path_keeps_no_side_copy() {
    let dir = TempDir::new();
    let snapshot = store_with_snapshot(&dir).await;
    for name in names(&db_dir(&dir)) {
        std::fs::remove_file(db_dir(&dir).join(name)).unwrap();
    }
    let applied = restore(&dir, &["--apply", "--from", path(&snapshot)]);
    assert_eq!(applied.status.code(), Some(0), "{}", text(&applied.stderr));
    assert_eq!(names(&db_dir(&dir)), ["kanade.sqlite"]);
    assert_eq!(holds(&dir.store(), &["snap", "later"]).await, ["snap"]);
}

#[tokio::test]
async fn an_encrypted_snapshot_restores_only_with_its_identity() {
    let dir = TempDir::new();
    mark(&dir.store(), "snap").await;
    let (identity, recipients) = dir.keygen("backup.key");
    std::fs::copy(&recipients, dir.backups().join("recipients")).unwrap();
    let made = dir
        .kanade(&["backup", "--name", "snap.sqlite"])
        .output()
        .unwrap();
    assert_eq!(made.status.code(), Some(0), "{}", text(&made.stderr));
    mark(&dir.store(), "later").await;
    let sealed = dir.backups().join("snap.sqlite.age");
    let before = names(&db_dir(&dir));

    let (stranger, _) = dir.keygen("stranger.key");
    let refused = restore(
        &dir,
        &[
            "--from",
            path(&sealed),
            "--identity",
            path(&stranger),
            "--apply",
        ],
    );
    assert_eq!(refused.status.code(), Some(78), "{}", text(&refused.stderr));
    assert!(text(&refused.stderr).contains("matches none of this file's recipients"));
    assert_eq!(names(&db_dir(&dir)), before);

    let applied = restore(
        &dir,
        &[
            "--from",
            path(&sealed),
            "--identity",
            path(&identity),
            "--apply",
        ],
    );
    assert_eq!(applied.status.code(), Some(0), "{}", text(&applied.stderr));
    let listed = names(&db_dir(&dir));
    assert_eq!(
        listed.len(),
        2,
        "the store and its side copy only: {listed:?}"
    );
    assert_eq!(holds(&dir.store(), &["snap", "later"]).await, ["snap"]);
}

// ---- token rotate ----

fn rotate(dir: &TempDir, args: &[&str]) -> Output {
    dir.kanade(&[&["token", "rotate"], args].concat())
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn token_rotate_writes_a_private_token_and_overwrites_only_with_force() {
    let dir = TempDir::new();
    let out = dir.0.join("keys/admin_token");
    let made = rotate(&dir, &["--out", path(&out)]);
    assert_eq!(made.status.code(), Some(0), "{}", text(&made.stderr));
    assert_eq!(mode(&out), 0o600);
    let first = std::fs::read_to_string(&out).unwrap();
    let token = first.strip_suffix('\n').unwrap();
    assert!(token.len() >= 32 && !token.contains('\n'), "{first:?}");
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    );
    let stdout = text(&made.stdout);
    assert!(
        stdout.contains(path(&out)) && stdout.contains("restart"),
        "{stdout}"
    );
    assert!(!stdout.contains(token) && !text(&made.stderr).contains(token));

    let again = rotate(&dir, &["--out", path(&out)]);
    assert_eq!(again.status.code(), Some(78), "{}", text(&again.stderr));
    assert!(text(&again.stderr).contains("pass --force"));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), first);

    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o644)).unwrap();
    let forced = rotate(&dir, &["--force", "--out", path(&out)]);
    assert_eq!(forced.status.code(), Some(0), "{}", text(&forced.stderr));
    let second = std::fs::read_to_string(&out).unwrap();
    assert_ne!(second, first);
    assert_eq!(mode(&out), 0o600);
    assert_eq!(names(&dir.0.join("keys")), ["admin_token"], "no temp left");

    let target = dir.0.join("keys/target");
    std::fs::File::create(&target)
        .unwrap()
        .write_all(b"keep\n")
        .unwrap();
    let link = dir.0.join("keys/linked_token");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    for args in [
        vec!["--out", path(&link)],
        vec!["--out", path(&link), "--force"],
    ] {
        let refused = rotate(&dir, &args);
        assert_eq!(refused.status.code(), Some(78), "{}", text(&refused.stderr));
        assert!(text(&refused.stderr).contains("symbolic link"));
    }
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep\n");
}
