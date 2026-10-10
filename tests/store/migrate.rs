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
    assert_eq!(store.schema_version().await.expect("version"), 36);
    assert_eq!(store.foreign_key_violations().await.expect("check"), 0);
    let empty = store.load(&Scope::All).await.expect("load");
    assert_eq!(empty.revision, 0);
    store.close().await.expect("close");
    assert_eq!(
        ledger(&config).await,
        [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36
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
        assert_eq!(store.schema_version().await.expect("version"), 36);
        assert_eq!(store.load(&Scope::All).await.expect("load"), before);
        store.close().await.expect("close");
    }
    assert_eq!(
        ledger(&config).await,
        [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36
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
        "INSERT INTO schema_migrations VALUES (37, 'next', '2027-01-01T00:00:00+00:00')",
    )
    .await;
    let error = SqliteStore::open(&config).await.err().expect("refused");
    assert!(
        matches!(
            error,
            SqliteStoreError::FutureVersion {
                found: 37,
                known: 36
            }
        ),
        "{error}"
    );
    assert_eq!(
        ledger(&config).await,
        [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37
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
         ALTER TABLE extractions DROP COLUMN session_id;
         ALTER TABLE extractions DROP COLUMN request_ids;
         ALTER TABLE chat_interactions DROP COLUMN session_id;
         ALTER TABLE chat_rounds DROP COLUMN request_ids;
         ALTER TABLE web_sessions DROP COLUMN superseded_until;
         ALTER TABLE web_sessions DROP COLUMN client_tag;
         ALTER TABLE web_sessions DROP COLUMN device;
         ALTER TABLE web_sessions DROP COLUMN avatar_hash;
         DROP TABLE settings_changes;
         DROP TABLE rewrites;
         DROP TABLE header_overrides;
         DROP TABLE idempotency_replays;
         DROP TABLE auth_audit;
         DROP TABLE owner_requests;
         DROP TABLE run_prompts;
         ALTER TABLE fixed_runs DROP COLUMN owner_pinned;
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
         DROP TABLE rewrites;
         DROP TABLE header_overrides;
         DROP TABLE idempotency_replays;
         DROP TABLE auth_audit;
         DROP TABLE owner_requests;
         DROP TABLE run_prompts;
         ALTER TABLE fixed_runs DROP COLUMN owner_pinned;
         DELETE FROM schema_migrations WHERE version >= 2;
         UPDATE store_meta SET schema_version = 1;",
    )
    .await;
    assert_eq!(ledger(&config).await, [1]);
    let store = SqliteStore::open(&config).await.expect("migrates");
    assert_eq!(store.schema_version().await.expect("version"), 36);
    assert_eq!(store.foreign_key_violations().await.expect("check"), 0);
    store.close().await.expect("close");
    assert_eq!(
        ledger(&config).await,
        [
            1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36
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
        "ALTER TABLE extractions DROP COLUMN session_id;
         ALTER TABLE extractions DROP COLUMN request_ids;
         ALTER TABLE chat_interactions DROP COLUMN session_id;
         ALTER TABLE chat_rounds DROP COLUMN request_ids;
         ALTER TABLE web_sessions DROP COLUMN superseded_until;
         ALTER TABLE web_sessions DROP COLUMN client_tag;
         ALTER TABLE web_sessions DROP COLUMN device;
         ALTER TABLE web_sessions DROP COLUMN avatar_hash;
DROP TABLE settings_changes;
         DROP TABLE rewrites;
         DROP TABLE header_overrides;
         DROP TABLE idempotency_replays;
         DROP TABLE auth_audit;
         DROP TABLE owner_requests;
         DROP TABLE run_prompts;
         ALTER TABLE fixed_runs DROP COLUMN owner_pinned;
         DELETE FROM schema_migrations WHERE version >= 23;
         UPDATE store_meta SET schema_version = 22;
         INSERT INTO config (key, value) VALUES ('quiet_mode', '1');",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("v22 migrates");
    assert_eq!(store.schema_version().await.expect("version"), 36);
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

/// 0025 adds the nullable `web_sessions.avatar_hash` to a v24 store: a
/// session signed in before it reads back without an avatar, a new one keeps
/// its hash across a reopen, and malformed hashes are refused.
#[tokio::test]
async fn upgrading_from_v24_keeps_sessions_and_adds_the_avatar_hash() {
    use kanade::infrastructure::store::web_sessions::{
        LoginMethod, SessionOrigin, WebSession, WebSessionStore,
    };

    let dir = TempDir::new();
    let config = dir.config("v24-sessions");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    let old = "a".repeat(64);
    tamper(
        &config,
        &format!(
            "ALTER TABLE web_sessions DROP COLUMN superseded_until;
             ALTER TABLE web_sessions DROP COLUMN client_tag;
             ALTER TABLE web_sessions DROP COLUMN device;
             ALTER TABLE web_sessions DROP COLUMN avatar_hash;
             DROP TABLE rewrites;
             DROP TABLE header_overrides;
             DROP TABLE idempotency_replays;
             DROP TABLE auth_audit;
             DROP TABLE owner_requests;
             DROP TABLE run_prompts;
             ALTER TABLE fixed_runs DROP COLUMN owner_pinned;
             DELETE FROM schema_migrations WHERE version >= 25;
             UPDATE store_meta SET schema_version = 24;
             INSERT INTO web_sessions (id_hash, origin, method, subject, display, created_at,
                 last_seen_at, checked_at, expires_at)
             VALUES ('{old}', 'admin', 'discord', '1003', 'Cara',
                 '2026-09-29T04:00:00.000000+00:00', '2026-09-29T04:00:00.000000+00:00',
                 '2026-09-29T04:00:00.000000+00:00', '2026-09-29T16:00:00.000000+00:00');"
        ),
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("v24 migrates");
    assert_eq!(store.schema_version().await.expect("version"), 36);
    let before = store.load_session(&old).await.expect("load").expect("kept");
    assert_eq!(
        (before.subject.as_str(), before.avatar_hash.as_deref()),
        ("1003", None)
    );
    let signed_in = WebSession {
        id_hash: "b".repeat(64),
        avatar_hash: Some("a_0123456789abcdef0123456789abcdef".into()),
        ..before.clone()
    };
    store.put_session(&signed_in, None).await.expect("put");
    store.close().await.expect("close");

    let store = SqliteStore::open(&config).await.expect("reopens");
    assert_eq!(
        store.load_session(&signed_in.id_hash).await.expect("load"),
        Some(signed_in.clone())
    );
    assert_eq!(
        store
            .load_session(&old)
            .await
            .expect("load")
            .unwrap()
            .origin,
        SessionOrigin::Admin
    );
    assert_eq!(before.method, LoginMethod::Discord);
    store.close().await.expect("close");

    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .connect()
        .await
        .expect("raw connection");
    for bad in [
        "0123",
        "<script>alert(1)</script>0123456789",
        "0123456789ABCDEF0123456789ABCDEF",
        "0123456789abcdef0123456789abcd_f",
        "_a0123456789abcdef0123456789abcdef",
        "a_a_23456789abcdef0123456789abcdef",
        "b_0123456789abcdef0123456789abcdef",
    ] {
        let refused = sqlx::query("UPDATE web_sessions SET avatar_hash = ?1")
            .bind(bad)
            .execute(&mut conn)
            .await;
        assert!(refused.is_err(), "{bad}");
    }
    for good in [
        "0123456789abcdef0123456789abcdef",
        "a_0123456789abcdef0123456789abcdef",
    ] {
        sqlx::query("UPDATE web_sessions SET avatar_hash = ?1")
            .bind(good)
            .execute(&mut conn)
            .await
            .unwrap_or_else(|error| panic!("{good}: {error}"));
    }
    conn.close().await.expect("close");
}

/// 0026 adds the nullable `web_sessions.device` to a v25 store: an older
/// session reads back without one, a new one keeps its label across a
/// reopen, and an empty or over-long label is refused.
#[tokio::test]
async fn upgrading_from_v25_keeps_sessions_and_adds_the_device() {
    use kanade::infrastructure::store::web_sessions::{WebSession, WebSessionStore};

    let dir = TempDir::new();
    let config = dir.config("v25-sessions");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    let old = "a".repeat(64);
    tamper(
        &config,
        &format!(
            "ALTER TABLE web_sessions DROP COLUMN superseded_until;
             ALTER TABLE web_sessions DROP COLUMN client_tag;
             ALTER TABLE web_sessions DROP COLUMN device;
             DROP TABLE rewrites;
             DROP TABLE header_overrides;
             DROP TABLE idempotency_replays;
             DROP TABLE auth_audit;
             DROP TABLE owner_requests;
             DROP TABLE run_prompts;
             ALTER TABLE fixed_runs DROP COLUMN owner_pinned;
             DELETE FROM schema_migrations WHERE version >= 26;
             UPDATE store_meta SET schema_version = 25;
             INSERT INTO web_sessions (id_hash, origin, method, subject, display, created_at,
                 last_seen_at, checked_at, expires_at)
             VALUES ('{old}', 'admin', 'token', 'fingerprint', 'Break-glass token',
                 '2026-09-29T04:00:00.000000+00:00', '2026-09-29T04:00:00.000000+00:00',
                 '2026-09-29T04:00:00.000000+00:00', '2026-09-29T16:00:00.000000+00:00');"
        ),
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("v25 migrates");
    assert_eq!(store.schema_version().await.expect("version"), 36);
    let before = store.load_session(&old).await.expect("load").expect("kept");
    assert_eq!(before.device, None);
    let signed_in = WebSession {
        id_hash: "b".repeat(64),
        device: Some("Firefox · macOS".into()),
        ..before.clone()
    };
    store.put_session(&signed_in, None).await.expect("put");
    store.close().await.expect("close");

    let store = SqliteStore::open(&config).await.expect("reopens");
    assert_eq!(
        store.load_session(&signed_in.id_hash).await.expect("load"),
        Some(signed_in.clone())
    );
    store.close().await.expect("close");

    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .connect()
        .await
        .expect("raw connection");
    for bad in [String::new(), "x".repeat(65)] {
        let refused = sqlx::query("UPDATE web_sessions SET device = ?1")
            .bind(&bad)
            .execute(&mut conn)
            .await;
        assert!(refused.is_err(), "{bad:?}");
    }
    sqlx::query("UPDATE web_sessions SET device = ?1")
        .bind("é".repeat(64))
        .execute(&mut conn)
        .await
        .expect("64 characters, whatever their bytes");
    conn.close().await.expect("close");
}

/// 0030 adds the nullable `web_sessions.client_tag` and `superseded_until`
/// to a v29 store: an older session reads back live and untagged, a rotation
/// keeps the old id through its grace across a reopen, and a malformed tag
/// is refused.
#[tokio::test]
async fn upgrading_from_v29_keeps_sessions_and_adds_rotation_grace() {
    use chrono::{TimeDelta, TimeZone, Utc};
    use kanade::infrastructure::store::web_sessions::{SessionOrigin, WebSession, WebSessionStore};

    let dir = TempDir::new();
    let config = dir.config("v29-sessions");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    let old = "a".repeat(64);
    tamper(
        &config,
        &format!(
            "DROP TABLE idempotency_replays;
             DROP TABLE auth_audit;
             DROP TABLE owner_requests;
             DROP TABLE run_prompts;
             ALTER TABLE fixed_runs DROP COLUMN owner_pinned;
             ALTER TABLE web_sessions DROP COLUMN superseded_until;
             ALTER TABLE web_sessions DROP COLUMN client_tag;
             DELETE FROM schema_migrations WHERE version >= 30;
             UPDATE store_meta SET schema_version = 29;
             INSERT INTO web_sessions (id_hash, origin, method, subject, display, created_at,
                 last_seen_at, checked_at, expires_at)
             VALUES ('{old}', 'public', 'discord', '1003', 'Cara',
                 '2026-09-29T04:00:00.000000+00:00', '2026-09-29T04:00:00.000000+00:00',
                 '2026-09-29T04:00:00.000000+00:00', '2026-09-29T16:00:00.000000+00:00');"
        ),
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("v29 migrates");
    assert_eq!(store.schema_version().await.expect("version"), 36);
    let before = store.load_session(&old).await.expect("load").expect("kept");
    assert_eq!(
        (
            before.origin,
            before.client_tag.as_deref(),
            before.superseded_until
        ),
        (SessionOrigin::Public, None, None)
    );
    let grace = Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 30).unwrap();
    let rotated = WebSession {
        id_hash: "b".repeat(64),
        client_tag: Some("c".repeat(64)),
        ..before.clone()
    };
    assert!(
        store
            .rotate_session(&rotated, &old, grace)
            .await
            .expect("rotate")
    );
    store.close().await.expect("close");

    let store = SqliteStore::open(&config).await.expect("reopens");
    assert_eq!(
        store.load_session(&rotated.id_hash).await.expect("load"),
        Some(rotated.clone())
    );
    assert_eq!(store.load_session(&old).await.expect("load"), None);
    assert_eq!(
        store
            .load_superseded(&old, grace - TimeDelta::microseconds(1))
            .await
            .expect("load"),
        Some(WebSession {
            superseded_until: Some(grace),
            ..before
        })
    );
    assert_eq!(
        store.load_superseded(&old, grace).await.expect("load"),
        None
    );
    store.close().await.expect("close");

    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .connect()
        .await
        .expect("raw connection");
    for bad in [
        "C".repeat(64),
        "c".repeat(63),
        "g".repeat(64),
        String::new(),
    ] {
        let refused = sqlx::query("UPDATE web_sessions SET client_tag = ?1")
            .bind(&bad)
            .execute(&mut conn)
            .await;
        assert!(refused.is_err(), "{bad:?}");
    }
    sqlx::query("UPDATE web_sessions SET client_tag = ?1")
        .bind("0123456789abcdef".repeat(4))
        .execute(&mut conn)
        .await
        .expect("64 lowercase hex");
    conn.close().await.expect("close");
}

/// 0031 adds `idempotency_replays` to a v30 store: existing settings stay, a
/// replay round-trips across a reopen, and malformed rows are refused by the
/// table's CHECKs.
#[tokio::test]
async fn upgrading_from_v30_adds_replays() {
    use chrono::{TimeZone, Utc};
    use kanade::domain::settings::{SettingsStore, keys};
    use kanade::infrastructure::store::replays::{ReplayScope, ReplayStore, StoredReplay};

    let dir = TempDir::new();
    let config = dir.config("v30-replays");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(
        &config,
        "DROP TABLE idempotency_replays;
         DROP TABLE auth_audit;
         DROP TABLE owner_requests;
         DROP TABLE run_prompts;
         ALTER TABLE fixed_runs DROP COLUMN owner_pinned;
         DELETE FROM schema_migrations WHERE version >= 31;
         UPDATE store_meta SET schema_version = 30;
         INSERT INTO config (key, value) VALUES ('quiet_mode', '1');",
    )
    .await;
    assert_eq!(ledger(&config).await.last(), Some(&30));
    let store = SqliteStore::open(&config).await.expect("v30 migrates");
    assert_eq!(store.schema_version().await.expect("version"), 36);
    let rows = store.settings_rows().await.expect("rows");
    assert_eq!(rows.get(keys::QUIET_MODE).map(String::as_str), Some("1"));
    let now = Utc.with_ymd_and_hms(2026, 10, 9, 4, 0, 0).unwrap();
    let replay = StoredReplay::new(
        ReplayScope::Limits,
        "admin:token".into(),
        "k-1".into(),
        "a".repeat(64),
        200,
        r#"{"message":"done"}"#.into(),
        now,
    );
    store.put_replay(replay.clone()).await.expect("put");
    store.close().await.expect("close");

    let store = SqliteStore::open(&config).await.expect("reopens");
    assert_eq!(
        store
            .replay(ReplayScope::Limits, "admin:token", "k-1", now)
            .await
            .expect("read"),
        Some(replay)
    );
    store.close().await.expect("close");

    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .connect()
        .await
        .expect("raw connection");
    for (column, bad) in [
        ("scope", "'other'"),
        ("request", "upper('a') || substr(request, 2)"),
        ("status", "500"),
        ("body", "'not json'"),
        ("expires_at", "created_at"),
    ] {
        let refused = sqlx::query(&format!("UPDATE idempotency_replays SET {column} = {bad}"))
            .execute(&mut conn)
            .await;
        assert!(refused.is_err(), "{column} = {bad}");
    }
    conn.close().await.expect("close");
}

#[tokio::test]
async fn upgrading_from_v34_adds_run_prompts_that_close_once() {
    use chrono::{TimeZone, Utc};
    use kanade::domain::completion::{PromptClose, PromptOutcome, RunPrompt, RunPromptStore};

    let dir = TempDir::new();
    let config = dir.config("v34-run-prompts");
    let store = SqliteStore::open(&config).await.expect("opens");
    seed(&store).await;
    let before = store.load(&Scope::All).await.expect("load");
    store.close().await.expect("close");
    tamper(
        &config,
        "DROP TABLE run_prompts;
         DELETE FROM schema_migrations WHERE version >= 35;
         UPDATE store_meta SET schema_version = 34;",
    )
    .await;
    assert_eq!(ledger(&config).await.last(), Some(&34));
    let store = SqliteStore::open(&config).await.expect("v34 migrates");
    assert_eq!(store.schema_version().await.expect("version"), 36);
    assert_eq!(
        store.load(&Scope::All).await.expect("load"),
        before,
        "the schedule is untouched"
    );
    let at = |hour| Utc.with_ymd_and_hms(2026, 10, 10, hour, 0, 0).unwrap();
    let ask = RunPrompt::open("r-1".into(), 0, at(12), at(13), at(16));
    store.create_run_prompt(ask.clone()).await.expect("create");
    let close = PromptClose {
        outcome: PromptOutcome::Done,
        decided_by: Some("1001".into()),
        at: at(14),
        next: None,
    };
    assert!(
        store
            .close_run_prompt("r-1".into(), 0, close)
            .await
            .expect("close")
    );
    store.close().await.expect("close");

    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .connect()
        .await
        .expect("raw connection");
    for (column, bad) in [
        ("outcome", "'auto_done'"),
        ("decided_by", "NULL"),
        ("cutoff_at", "due_at"),
        ("message_settled", "2"),
    ] {
        let refused = sqlx::query(&format!("UPDATE run_prompts SET {column} = {bad}"))
            .execute(&mut conn)
            .await;
        assert!(refused.is_err(), "{column} = {bad}");
    }
    let second_open = sqlx::query(
        "INSERT INTO run_prompts (run_id, ask, ends_at, due_at, cutoff_at) VALUES \
         ('r-2', 0, 'a', 'b', 'c'), ('r-2', 1, 'a', 'b', 'c')",
    )
    .execute(&mut conn)
    .await;
    assert!(second_open.is_err(), "one open ask per run");
    conn.close().await.expect("close");
}

/// 0036 widens a populated v35 `auth_audit` for `write_refused` on open:
/// every stored row survives with its `seq`, new rows continue the sequence,
/// and the append-only trigger still refuses UPDATE.
#[tokio::test]
async fn upgrading_from_v35_keeps_audit_rows_and_stores_refused_writes() {
    use chrono::{TimeZone, Utc};
    use kanade::infrastructure::store::auth_audit::{
        AuditFilter, AuditKind, AuditRealm, AuditRow, AuthAuditStore,
    };

    let at = |hour| Utc.with_ymd_and_hms(2026, 10, 10, hour, 0, 0).unwrap();
    let row = |hour, realm, event, client: &str| AuditRow {
        seq: 0,
        at: at(hour),
        realm,
        event,
        actor: Some("discord:1001".into()),
        method: None,
        reason: Some("not_in_run".into()),
        request: None,
        client: Some(client.into()),
        device: None,
        request_id: format!("req-{hour}"),
    };
    let page = AuditFilter {
        limit: 200,
        ..AuditFilter::default()
    };
    let dir = TempDir::new();
    let config = dir.config("v35-audit");
    let store = SqliteStore::open(&config).await.expect("opens");
    store
        .append_audit(row(
            1,
            AuditRealm::Admin,
            AuditKind::LoginSucceeded,
            "192.0.2.1",
        ))
        .await
        .expect("admin row");
    store
        .append_audit(row(
            2,
            AuditRealm::Member,
            AuditKind::RateLimited,
            &"a".repeat(64),
        ))
        .await
        .expect("member row");
    let before = store.audit_page(&page).await.expect("page");
    store.close().await.expect("close");
    // Back to 0032's table, rows and all, as a v35 store holds it.
    tamper(
        &config,
        &format!(
            "DROP TRIGGER auth_audit_append_only;
             DROP INDEX auth_audit_at;
             ALTER TABLE auth_audit RENAME TO auth_audit_rows;
             {}
             INSERT INTO auth_audit SELECT * FROM auth_audit_rows;
             DROP TABLE auth_audit_rows;
             DELETE FROM schema_migrations WHERE version >= 36;
             UPDATE store_meta SET schema_version = 35;",
            include_str!("../../src/infrastructure/store/sqlite/migrations/0032_auth_audit.sql")
        ),
    )
    .await;
    assert_eq!(ledger(&config).await.last(), Some(&35));

    let store = SqliteStore::open(&config).await.expect("v35 migrates");
    assert_eq!(store.schema_version().await.expect("version"), 36);
    assert_eq!(store.foreign_key_violations().await.expect("check"), 0);
    assert_eq!(store.audit_page(&page).await.expect("page"), before);
    let refused = AuditRow {
        request: Some("PUT /api/public/runs/r-1/answer".into()),
        ..row(
            3,
            AuditRealm::Member,
            AuditKind::WriteRefused,
            &"a".repeat(64),
        )
    };
    let seq = store.append_audit(refused.clone()).await.expect("stored");
    assert_eq!(seq, 3);
    let listed = store
        .audit_page(&AuditFilter {
            event: Some(AuditKind::WriteRefused),
            ..page.clone()
        })
        .await
        .expect("filtered");
    assert_eq!(listed, [AuditRow { seq, ..refused }]);
    store.close().await.expect("close");

    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .connect()
        .await
        .expect("raw connection");
    let refused = sqlx::query("UPDATE auth_audit SET reason = 'changed'")
        .execute(&mut conn)
        .await;
    assert!(refused.is_err(), "append-only trigger remains");
    conn.close().await.expect("close");
}
