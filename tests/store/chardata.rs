//! The character-data file itself: private and owned like the main store,
//! its own checksummed ledger, a rollback journal deleted after each write
//! (never WAL), the call budget surviving a reopen, an unlink leaving no
//! byte of the invented data in the file, and older files migrating with
//! their rows intact. Invented data only.

use std::os::unix::fs::PermissionsExt;

use chrono::TimeDelta;
use kanade::domain::chardata::{
    CharDataStore, ConsentSurface, DAILY_CALLS, ESTIMATE_TTL, Post, RefreshState,
};
use kanade::infrastructure::store::chardata_conformance::{
    active, at, member_estimate, party_estimate, snapshot,
};
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
    assert_eq!(ledger, [1, 2, 3]);
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
    sqlx::query("INSERT INTO chardata_migrations VALUES (4, 'x', 'then')")
        .execute(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
    assert!(matches!(
        SqliteCharData::open(&other).await,
        Err(SqliteStoreError::FutureVersion { found: 4, known: 3 })
    ));
}

/// A file at version 1 (links without `consent_surface`) migrates to 2 on
/// open: its links keep reading, with no surface recorded; consents from
/// then on record theirs, and a pending link can never carry one.
#[tokio::test]
async fn a_version_one_file_reads_its_links_without_a_surface() {
    let dir = TempDir::new();
    let config = dir.config("chardata");
    SqliteCharData::open(&config)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut conn = raw(&config.db_path).await;
    for sql in DOWN_TO_TWO.iter().chain(&[
        "DELETE FROM chardata_migrations WHERE version = 2",
        "ALTER TABLE links DROP COLUMN consent_surface",
        "INSERT INTO links VALUES ('c1', '1001', 'wisteria', 'msea', 'active', 'member', '1001',
            '2026-09-01T00:00:00+00:00', 1, '2026-09-01T00:00:00+00:00',
            '2026-09-01T00:00:00+00:00', 1)",
    ]) {
        sqlx::query(sql).execute(&mut conn).await.unwrap();
    }
    conn.close().await.unwrap();
    let store = SqliteCharData::open(&config).await.unwrap();
    let old = store.link("c1".into()).await.unwrap().unwrap();
    assert_eq!(old.consent_surface, None);
    assert_eq!(old.consent_version, Some(1));
    let portal = kanade::domain::chardata::Link {
        consent_surface: Some(ConsentSurface::Portal),
        ..active("c2")
    };
    store.put_link(portal, 0).await.unwrap();
    let read = store.link("c2".into()).await.unwrap().unwrap();
    assert_eq!(read.consent_surface, Some(ConsentSurface::Portal));
    store.close().await.unwrap();
    let mut conn = raw(&config.db_path).await;
    let refused = sqlx::query(
        "UPDATE links SET status = 'pending', consent_version = NULL, consented_at = NULL
         WHERE character_id = 'c2'",
    )
    .execute(&mut conn)
    .await;
    assert!(refused.is_err(), "a pending link never carries a surface");
    conn.close().await.unwrap();
}

/// Takes a current file back to version 2 (drops the `0003` tables).
const DOWN_TO_TWO: [&str; 4] = [
    "DROP TABLE estimate_party_members",
    "DROP TABLE estimate_parties",
    "DROP TABLE estimate_members",
    "DELETE FROM chardata_migrations WHERE version = 3",
];

async fn ledger(conn: &mut sqlx::SqliteConnection) -> Vec<i64> {
    sqlx::query_scalar("SELECT version FROM chardata_migrations ORDER BY version")
        .fetch_all(conn)
        .await
        .unwrap()
}

async fn estimate_tables(conn: &mut sqlx::SqliteConnection) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'estimate_%'
         ORDER BY name",
    )
    .fetch_all(conn)
    .await
    .unwrap()
}

/// A version 2 file holding links, snapshots, refresh state, posts and
/// calls migrates to 3 on open: every row reads back as written, the
/// estimate tables come up empty and working, and the file passes
/// `integrity_check` and `foreign_key_check`.
#[tokio::test]
async fn a_version_two_file_migrates_to_three_with_its_rows() {
    let dir = TempDir::new();
    let config = dir.config("chardata");
    let store = SqliteCharData::open(&config).await.unwrap();
    let data = snapshot("c1", at(2, 0));
    let state = RefreshState {
        character_id: "c1".into(),
        next_attempt_at: Some(at(3, 0)),
        last_error_code: Some("OPENAPI00009".into()),
    };
    let post = Post::new("a1b2c3".into(), "900".into(), "c1".into(), at(2, 0));
    store.put_link(active("c1"), 0).await.unwrap();
    assert!(store.put_snapshot(data.clone()).await.unwrap());
    assert!(store.put_refresh_state(state.clone()).await.unwrap());
    assert!(store.create_post(post.clone()).await.unwrap());
    assert!(store.reserve_call(at(2, 0), DAILY_CALLS).await.unwrap());
    store.close().await.unwrap();
    let mut conn = raw(&config.db_path).await;
    for sql in DOWN_TO_TWO {
        sqlx::query(sql).execute(&mut conn).await.unwrap();
    }
    assert_eq!(ledger(&mut conn).await, [1, 2]);
    assert!(estimate_tables(&mut conn).await.is_empty());
    conn.close().await.unwrap();

    let store = SqliteCharData::open(&config).await.unwrap();
    assert_eq!(store.links().await.unwrap(), [active("c1")]);
    assert_eq!(
        store.snapshot("c1".into(), at(3, 0)).await.unwrap(),
        Some(data)
    );
    assert_eq!(store.refresh_states().await.unwrap(), [state]);
    assert_eq!(store.posts().await.unwrap(), [post]);
    assert_eq!(store.calls_since(at(1, 0)).await.unwrap(), 1);
    let now = at(3, 0);
    assert!(store.estimates(now, 10).await.unwrap().members.is_empty());
    let estimate = member_estimate("c1", "Wisteria", at(2, 0));
    store.put_member_estimate(estimate.clone()).await.unwrap();
    assert_eq!(store.estimates(now, 10).await.unwrap().members, [estimate]);
    store.close().await.unwrap();

    let mut conn = raw(&config.db_path).await;
    assert_eq!(ledger(&mut conn).await, [1, 2, 3]);
    assert_eq!(
        estimate_tables(&mut conn).await,
        [
            "estimate_members",
            "estimate_parties",
            "estimate_party_members"
        ]
    );
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut conn)
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    let broken = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut conn)
        .await
        .unwrap();
    assert!(broken.is_empty(), "foreign keys hold");
    conn.close().await.unwrap();
}

/// A party total's member rows go with it, whether a character's
/// estimates are deleted or the total is swept.
#[tokio::test]
async fn party_members_go_with_their_totals() {
    let dir = TempDir::new();
    let config = dir.config("chardata");
    let members = async || {
        let mut conn = raw(&config.db_path).await;
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT set_key, character_id FROM estimate_party_members
             ORDER BY set_key, character_id",
        )
        .fetch_all(&mut conn)
        .await
        .unwrap();
        conn.close().await.unwrap();
        rows
    };
    let store = SqliteCharData::open(&config).await.unwrap();
    for characters in [["c1", "c2"], ["c2", "c3"]] {
        store
            .put_party_estimate(party_estimate(&characters, at(2, 0)))
            .await
            .unwrap();
    }
    store.delete_estimates("c1".into()).await.unwrap();
    store.close().await.unwrap();
    let pair = |set: &str, id: &str| (set.to_owned(), id.to_owned());
    assert_eq!(members().await, [pair("c2,c3", "c2"), pair("c2,c3", "c3")]);

    let store = SqliteCharData::open(&config).await.unwrap();
    assert_eq!(
        store
            .sweep_estimates(at(2, 0) + ESTIMATE_TTL)
            .await
            .unwrap(),
        1
    );
    store.close().await.unwrap();
    assert!(members().await.is_empty());
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
         AND name IN ('links', 'snapshots', 'refresh_state', 'posts', 'calls', 'chardata_migrations',
             'estimate_members', 'estimate_parties', 'estimate_party_members')",
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
    assert!(compose.contains("KANADE_CHARACTER_DATA_DB_PATH: /chardata/db/chardata.sqlite"));
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
