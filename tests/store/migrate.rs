//! Embedded migrations: fresh files, idempotent reopen and refusals.

use kanade::domain::notify::DeclineNoticeStore;
use kanade::domain::schedule::{Change, ChangeSet};
use kanade::domain::scheduler::{ScheduleStore, Scope, StoreError};
use kanade::infrastructure::store::{SqliteStore, SqliteStoreConfig, SqliteStoreError};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection, Row};

use crate::support::{TempDir, reminder, seed, tamper};

async fn ledger(config: &SqliteStoreConfig) -> Vec<i64> {
    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .read_only(true)
        .connect()
        .await
        .expect("raw connection");
    let rows = sqlx::query("SELECT version FROM schema_migrations ORDER BY version")
        .fetch_all(&mut conn)
        .await
        .expect("ledger");
    conn.close().await.expect("close");
    rows.iter().map(|row| row.get("version")).collect()
}

#[tokio::test]
async fn empty_file_migrates_to_the_newest_version_with_sound_foreign_keys() {
    let dir = TempDir::new();
    let config = dir.config("empty");
    std::fs::write(&config.db_path, b"").expect("empty file");
    std::fs::set_permissions(
        &config.db_path,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .expect("chmod");
    let store = SqliteStore::open(&config).await.expect("opens");
    assert_eq!(store.schema_version().await.expect("version"), 23);
    assert_eq!(store.foreign_key_violations().await.expect("check"), 0);
    let empty = store.load(&Scope::All).await.expect("load");
    assert_eq!(empty.revision, 0);
    store.close().await.expect("close");
    assert_eq!(
        ledger(&config).await,
        [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23
        ]
    );
}

#[tokio::test]
async fn reopen_is_idempotent_and_keeps_rows() {
    let dir = TempDir::new();
    let config = dir.config("reopen");
    let store = SqliteStore::open(&config).await.expect("opens");
    seed(&store).await;
    let before = store.load(&Scope::All).await.expect("load");
    store.close().await.expect("close");
    for _ in 0..2 {
        let store = SqliteStore::open(&config).await.expect("reopens");
        assert_eq!(store.schema_version().await.expect("version"), 23);
        assert_eq!(store.load(&Scope::All).await.expect("load"), before);
        store.close().await.expect("close");
    }
    assert_eq!(
        ledger(&config).await,
        [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23
        ]
    );
}

#[tokio::test]
async fn changed_migration_checksum_refuses_to_open() {
    let dir = TempDir::new();
    let config = dir.config("checksum");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(&config, "UPDATE schema_migrations SET checksum = 'edited'").await;
    let error = SqliteStore::open(&config).await.err().expect("refused");
    assert!(
        matches!(error, SqliteStoreError::ChecksumMismatch { version: 1 }),
        "{error}"
    );
}

#[tokio::test]
async fn future_schema_version_refuses_to_open() {
    let dir = TempDir::new();
    let config = dir.config("future");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(
        &config,
        "INSERT INTO schema_migrations VALUES (24, 'next', '2027-01-01T00:00:00+00:00')",
    )
    .await;
    let error = SqliteStore::open(&config).await.err().expect("refused");
    assert!(
        matches!(
            error,
            SqliteStoreError::FutureVersion {
                found: 24,
                known: 23
            }
        ),
        "{error}"
    );
    assert_eq!(
        ledger(&config).await,
        [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24
        ],
        "a refused open writes nothing"
    );
}

#[tokio::test]
async fn foreign_keys_are_enforced_after_reopen() {
    let dir = TempDir::new();
    let config = dir.config("fk");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    let store = SqliteStore::open(&config).await.expect("reopens");
    seed(&store).await;
    let before = store.load(&Scope::All).await.expect("load");
    let result = store
        .commit(
            before.revision,
            ChangeSet {
                changes: vec![Change::PutReminder(reminder("m-orphan", "absent"))],
            },
            kanade::infrastructure::store::conformance::meta(),
        )
        .await;
    assert!(
        matches!(result, Err(StoreError::Constraint(_))),
        "{result:?}"
    );
    assert_eq!(store.load(&Scope::All).await.expect("load"), before);
    assert_eq!(store.foreign_key_violations().await.expect("check"), 0);
    store.close().await.expect("close");
}

#[tokio::test]
async fn upgrading_from_v19_preserves_declines_and_reenables_foreign_keys() {
    let dir = TempDir::new();
    let config = dir.config("v19-declines");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(
        &config,
        "DROP INDEX decline_notices_pending;
         ALTER TABLE decline_notices DROP COLUMN retract_pending;
         ALTER TABLE decline_notices DROP COLUMN display_name;
         ALTER TABLE decline_notices DROP COLUMN reference_id;
         INSERT INTO decline_notices (run_id, user_id, channel_id, message_id, notified_at)
         VALUES ('run-19', 'member-19', 'channel-19', 'message-19', '2026-09-01T00:00:00.000000+00:00');
         DROP TABLE settings_changes;
         DELETE FROM schema_migrations WHERE version >= 20;
         ALTER TABLE extractions DROP COLUMN reasoning_content;
         ALTER TABLE extractions DROP COLUMN reasoning_tokens;
         ALTER TABLE chat_rounds DROP COLUMN reasoning_content;
         ALTER TABLE chat_rounds DROP COLUMN reasoning_tokens;
         UPDATE store_meta SET schema_version = 19;",
    )
    .await;

    let store = SqliteStore::open(&config).await.expect("v19 migrates");
    let decline = store
        .decline_notice("run-19", "member-19")
        .await
        .expect("decline reads")
        .expect("v19 row remains");
    assert_eq!(decline.channel_id.as_deref(), Some("channel-19"));
    assert_eq!(decline.message_id.as_deref(), Some("message-19"));
    assert_eq!(decline.reference_id, None);
    assert_eq!(decline.display_name, None);
    assert!(!decline.retract_pending);

    seed(&store).await;
    let before = store.load(&Scope::All).await.expect("load");
    let result = store
        .commit(
            before.revision,
            ChangeSet {
                changes: vec![Change::PutReminder(reminder("m-orphan-v19", "absent"))],
            },
            kanade::infrastructure::store::conformance::meta(),
        )
        .await;
    assert!(
        matches!(result, Err(StoreError::Constraint(_))),
        "{result:?}"
    );
    assert_eq!(store.foreign_key_violations().await.expect("check"), 0);
    store.close().await.expect("close");
}

#[tokio::test]
async fn migration_gap_refuses_to_open() {
    let dir = TempDir::new();
    let config = dir.config("gap");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(
        &config,
        "INSERT INTO schema_migrations VALUES (0, 'unknown', '2026-01-01T00:00:00+00:00')",
    )
    .await;
    let error = SqliteStore::open(&config).await.err().expect("refused");
    assert!(
        matches!(error, SqliteStoreError::MigrationGap { version: 0 }),
        "{error}"
    );
}

#[tokio::test]
async fn a_version_one_store_gains_the_later_tables_on_open() {
    let dir = TempDir::new();
    let config = dir.config("upgrade");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    // Roll the file back to what a version-1 build left behind.
    tamper(
        &config,
        "DROP INDEX decline_notices_pending;
         ALTER TABLE decline_notices DROP COLUMN retract_pending;
         ALTER TABLE decline_notices DROP COLUMN display_name;
         ALTER TABLE decline_notices DROP COLUMN reference_id;
         DROP TABLE debug_cards;
         DROP TABLE digest_card_phrases;
         DROP TABLE reminder_cards;
         DROP INDEX delivery_card_runs_run;
         DROP TABLE notice_outbox;
         DROP INDEX delivery_attempts_dedupe_key;
         DROP TABLE proposal_cards;
         DROP TABLE member_aliases;
         ALTER TABLE members DROP COLUMN aliases;
         ALTER TABLE members DROP COLUMN reply_style;
         ALTER TABLE members DROP COLUMN roles;
         ALTER TABLE members DROP COLUMN is_guild_admin;
         DROP TABLE web_sessions;
         DROP TABLE draft_proposals;
         DROP TABLE self_service_tips;
         DROP TABLE chat_allowance_overrides;
         DROP TABLE rescan_jobs;
         DROP TABLE chat_masked;
         DROP TABLE chat_tools;
         DROP TABLE chat_rounds;
         DROP TABLE chat_interactions;
         DROP TABLE extraction_members;
         DROP TABLE extractions;
         DROP TABLE messages;
         DROP TABLE run_status_pins;
         DROP TABLE run_attendance;
         DROP TABLE standing_answers;
         ALTER TABLE fixed_runs DROP COLUMN attendance_default;
         DROP TABLE delivery_card_runs;
         DROP TABLE checkpoints;
         DROP TABLE change_fields;
         DROP TABLE change_log_weeks;
         DROP TABLE change_log;
         DROP TABLE draft_requests;
         DROP TABLE draft_events;
         DROP TABLE draft_ops;
         DROP TABLE drafts;
         DROP TABLE settings_changes;
         DELETE FROM schema_migrations WHERE version >= 2;
         UPDATE store_meta SET schema_version = 1;",
    )
    .await;
    assert_eq!(ledger(&config).await, [1]);
    let store = SqliteStore::open(&config).await.expect("migrates");
    assert_eq!(store.schema_version().await.expect("version"), 23);
    assert_eq!(store.foreign_key_violations().await.expect("check"), 0);
    store.close().await.expect("close");
    assert_eq!(
        ledger(&config).await,
        [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23
        ]
    );
    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .read_only(true)
        .connect()
        .await
        .expect("raw connection");
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'delivery_card_runs'",
    )
    .fetch_one(&mut conn)
    .await
    .expect("schema");
    conn.close().await.expect("close");
    assert_eq!(tables, 1);
}

/// 0023 adds the append-only `settings_changes` to a v22 store: settings
/// rows survive, a recorded save lands with its rows, and stored changes
/// refuse UPDATE and DELETE.
#[tokio::test]
async fn upgrading_from_v22_adds_append_only_settings_changes() {
    use kanade::domain::history::{Actor, Surface};
    use kanade::domain::settings::{
        RowDiff, SettingsChange, SettingsChangeQuery, SettingsStore, keys,
    };

    let dir = TempDir::new();
    let config = dir.config("v22-settings");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(
        &config,
        "DROP TABLE settings_changes;
         DELETE FROM schema_migrations WHERE version >= 23;
         UPDATE store_meta SET schema_version = 22;
         INSERT INTO config (key, value) VALUES ('quiet_mode', '1');",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("v22 migrates");
    assert_eq!(store.schema_version().await.expect("version"), 23);
    let rows = store.settings_rows().await.expect("rows");
    assert_eq!(rows.get(keys::QUIET_MODE).map(String::as_str), Some("1"));
    assert!(
        store
            .settings_changes(SettingsChangeQuery::default())
            .await
            .expect("list")
            .is_empty()
    );
    let id = store
        .put_settings_rows_recorded(
            vec![(keys::QUIET_MODE.into(), "0".into())],
            SettingsChange {
                id: 0,
                at: chrono::DateTime::parse_from_rfc3339("2026-09-29T04:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
                actor: Actor::admin("token"),
                surface: Surface::AdminPortal,
                section: "notifications".into(),
                revision: 1,
                values: [(
                    keys::QUIET_MODE.to_owned(),
                    RowDiff {
                        from: "1".into(),
                        to: "0".into(),
                    },
                )]
                .into(),
            },
        )
        .await
        .expect("recorded save");
    assert_eq!(id, 1);
    store.close().await.expect("close");

    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .connect()
        .await
        .expect("raw connection");
    for refused in [
        "UPDATE settings_changes SET section = 'pings'",
        "DELETE FROM settings_changes",
    ] {
        let error = sqlx::raw_sql(refused).execute(&mut conn).await.err();
        assert!(
            error.is_some_and(|error| error.to_string().contains("append-only")),
            "{refused}"
        );
    }
    conn.close().await.expect("close");
}
