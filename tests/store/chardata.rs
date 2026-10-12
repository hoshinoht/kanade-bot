//! The character-data file itself: private and owned like the main store,
//! its own checksummed ledger, a rollback journal deleted after each write
//! (never WAL), the call budget surviving a reopen, and an unlink leaving no
//! byte of the invented data in the file. Invented data only.

use std::os::unix::fs::PermissionsExt;

use chrono::TimeDelta;
use kanade::domain::chardata::{CharDataStore, DAILY_CALLS};
use kanade::infrastructure::store::chardata_conformance::{active, at, snapshot};
use kanade::infrastructure::store::{SqliteCharData, SqliteStoreError};
use sqlx::{ConnectOptions, Connection, sqlite::SqliteConnectOptions};

use crate::support::TempDir;

async fn raw(path: &std::path::Path) -> sqlx::SqliteConnection {
    SqliteConnectOptions::new()
        .filename(path)
        .connect()
        .await
        .unwrap()
}

#[tokio::test]
async fn the_file_is_private_owned_and_never_wal() {
    let dir = TempDir::new();
    let config = dir.config("chardata");
    let store = SqliteCharData::open(&config).await.unwrap();
    let mode = std::fs::metadata(&config.db_path)
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert!(matches!(
        SqliteCharData::open(&config).await,
        Err(SqliteStoreError::Owned { .. })
    ));
    store.put_link(active("c1"), 0).await.unwrap();
    let journal = |suffix: &str| {
        let mut name = config.db_path.clone().into_os_string();
        name.push(suffix);
        std::path::PathBuf::from(name).exists()
    };
    assert!(!journal("-wal") && !journal("-journal"));
    store.close().await.unwrap();
    let mut conn = raw(&config.db_path).await;
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(mode, "delete");
    let ledger: Vec<i64> = sqlx::query_scalar("SELECT version FROM chardata_migrations")
        .fetch_all(&mut conn)
        .await
        .unwrap();
    assert_eq!(ledger, [1]);
    conn.close().await.unwrap();
}

#[tokio::test]
async fn a_changed_or_newer_ledger_refuses_to_open() {
    let dir = TempDir::new();
    let config = dir.config("chardata");
    SqliteCharData::open(&config)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut conn = raw(&config.db_path).await;
    sqlx::query("UPDATE chardata_migrations SET checksum = 'edited' WHERE version = 1")
        .execute(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
    assert!(matches!(
        SqliteCharData::open(&config).await,
        Err(SqliteStoreError::ChecksumMismatch { version: 1 })
    ));
    let other = dir.config("newer");
    SqliteCharData::open(&other)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut conn = raw(&other.db_path).await;
    sqlx::query("INSERT INTO chardata_migrations VALUES (2, 'x', 'then')")
        .execute(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
    assert!(matches!(
        SqliteCharData::open(&other).await,
        Err(SqliteStoreError::FutureVersion { found: 2, known: 1 })
    ));
}

#[tokio::test]
async fn the_call_budget_survives_a_reopen() {
    let dir = TempDir::new();
    let config = dir.config("chardata");
    let store = SqliteCharData::open(&config).await.unwrap();
    for n in 0..DAILY_CALLS {
        let when = at(2, 0) + TimeDelta::seconds(i64::from(n));
        assert!(store.reserve_call(when, DAILY_CALLS).await.unwrap());
    }
    store.close().await.unwrap();
    let store = SqliteCharData::open(&config).await.unwrap();
    assert!(
        !store.reserve_call(at(2, 12), DAILY_CALLS).await.unwrap(),
        "the hard stop holds after a restart"
    );
    assert_eq!(store.calls_since(at(1, 0)).await.unwrap(), DAILY_CALLS);
    store.close().await.unwrap();
}

#[tokio::test]
async fn an_unlink_leaves_no_byte_of_the_data() {
    let dir = TempDir::new();
    let config = dir.config("chardata");
    let store = SqliteCharData::open(&config).await.unwrap();
    let data = snapshot("c1", at(2, 0));
    store.put_link(active("c1"), 0).await.unwrap();
    assert!(store.put_snapshot(data.clone()).await.unwrap());
    store.close().await.unwrap();
    let cp = (data.combat_power.unwrap() as u32).to_be_bytes();
    let needles: [&[u8]; 4] = [b"ocid-invented-f00d", b"Wisteria", b"wisteria", &cp];
    let bytes = std::fs::read(&config.db_path).unwrap();
    for needle in needles {
        assert!(contains(&bytes, needle), "the data was written");
    }

    let store = SqliteCharData::open(&config).await.unwrap();
    assert!(store.unlink("c1".into(), at(3, 0)).await.unwrap().is_some());
    store.close().await.unwrap();
    let bytes = std::fs::read(&config.db_path).unwrap();
    for needle in needles {
        assert!(!contains(&bytes, needle), "{needle:?} left in the file");
    }
    let mut journal = config.db_path.clone().into_os_string();
    journal.push("-journal");
    assert!(!std::path::Path::new(&journal).exists(), "no journal left");
}

/// `kanade backup` snapshots the main store only: with the module's file
/// beside it, full of data, the backup holds none of its tables or bytes.
#[tokio::test]
async fn a_backup_holds_no_character_data() {
    let dir = TempDir::new();
    let main = kanade::infrastructure::store::SqliteStore::open(&dir.config("kanade"))
        .await
        .unwrap();
    let file = SqliteCharData::open(&dir.config("chardata")).await.unwrap();
    file.put_link(active("c1"), 0).await.unwrap();
    file.put_snapshot(snapshot("c1", at(2, 0))).await.unwrap();
    let copy = dir.path().join("backup.sqlite3");
    main.backup(&copy).await.unwrap();
    file.close().await.unwrap();
    main.close().await.unwrap();
    let bytes = std::fs::read(&copy).unwrap();
    assert!(!contains(&bytes, b"ocid-invented-f00d"));
    let mut conn = raw(&copy).await;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table'
         AND name IN ('links', 'snapshots', 'refresh_state', 'posts', 'calls', 'chardata_migrations')",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    assert!(tables.is_empty(), "{tables:?}");
    conn.close().await.unwrap();
}

/// Compose mounts the module's file on its own volume, which neither the
/// data volume (tarballed by the runbook) nor the `backup` service holds.
#[test]
fn compose_pins_the_separate_volume() {
    let compose = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("deploy/compose.yaml"),
    )
    .unwrap();
    assert!(compose.contains("KANADE_CHARACTER_DATA_DB_PATH: /chardata/chardata.sqlite"));
    assert!(compose.contains("      - chardata:/chardata\n"));
    assert!(compose.contains("  chardata:\n    name: kanade_v5_chardata\n"));
    let backup = compose
        .split("\n  backup:\n")
        .nth(1)
        .and_then(|rest| rest.split("\n  cloudflared:\n").next())
        .unwrap();
    assert!(
        !backup.contains("chardata"),
        "the backup service never mounts it"
    );
    assert!(compose.contains(
        "KANADE_NEXON_API_KEY_FILE: ${KANADE_NEXON_API_KEY_HOST_FILE:+/run/secrets/nexon_api_key}"
    ));
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
