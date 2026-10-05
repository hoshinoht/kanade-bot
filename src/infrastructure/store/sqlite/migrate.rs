//! Ordered, embedded, checksummed schema migrations.
//!
//! All pending migrations apply in one `BEGIN IMMEDIATE` transaction. Each
//! applied version keeps the SHA-256 of its SQL; a changed or unknown applied
//! migration refuses to open rather than guessing.

use ring::digest::{SHA256, digest};
use sqlx::{Connection, Executor, Row, SqliteConnection};

use super::SqliteStoreError;

struct Migration {
    version: i64,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("migrations/0002_card_runs.sql"),
    },
    Migration {
        version: 3,
        sql: include_str!("migrations/0003_change_log.sql"),
    },
    Migration {
        version: 4,
        sql: include_str!("migrations/0004_blame_checkpoints.sql"),
    },
    Migration {
        version: 5,
        sql: include_str!("migrations/0005_drafts.sql"),
    },
    Migration {
        version: 6,
        sql: include_str!("migrations/0006_attendance.sql"),
    },
    Migration {
        version: 7,
        sql: include_str!("migrations/0007_model_logs.sql"),
    },
    Migration {
        version: 8,
        sql: include_str!("migrations/0008_log_retention.sql"),
    },
    Migration {
        version: 9,
        sql: include_str!("migrations/0009_web_sessions.sql"),
    },
    Migration {
        version: 10,
        sql: include_str!("migrations/0010_member_profile.sql"),
    },
    Migration {
        version: 11,
        sql: include_str!("migrations/0011_proposal_cards.sql"),
    },
    Migration {
        version: 12,
        sql: include_str!("migrations/0012_notice_outbox.sql"),
    },
    Migration {
        version: 13,
        sql: include_str!("migrations/0013_notice_outbox_retention.sql"),
    },
    Migration {
        version: 14,
        sql: include_str!("migrations/0014_reminder_cards.sql"),
    },
    Migration {
        version: 15,
        sql: include_str!("migrations/0015_masked_chat.sql"),
    },
    Migration {
        version: 16,
        sql: include_str!("migrations/0016_chat_turn_facts.sql"),
    },
    Migration {
        version: 17,
        sql: include_str!("migrations/0017_debug_cards.sql"),
    },
    Migration {
        version: 18,
        sql: include_str!("migrations/0018_reminder_voice.sql"),
    },
    Migration {
        version: 19,
        sql: include_str!("migrations/0019_model_log_usage.sql"),
    },
    Migration {
        version: 20,
        sql: include_str!("migrations/0020_decline_notices.sql"),
    },
    Migration {
        version: 21,
        sql: include_str!("migrations/0021_model_log_reasoning.sql"),
    },
    Migration {
        version: 22,
        sql: include_str!("migrations/0022_chat_profanity.sql"),
    },
    Migration {
        version: 23,
        sql: include_str!("migrations/0023_settings_changes.sql"),
    },
    Migration {
        version: 24,
        sql: include_str!("migrations/0024_model_log_correlation.sql"),
    },
];

/// The migration that adds `change_fields`, which is backfilled from the
/// records already stored.
const BLAME_INDEX_VERSION: i64 = 4;

const LEDGER: &str = "CREATE TABLE IF NOT EXISTS schema_migrations (
    version    INTEGER PRIMARY KEY,
    checksum   TEXT NOT NULL,
    applied_at TEXT NOT NULL
)";

fn checksum(sql: &str) -> String {
    digest(&SHA256, sql.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Check an applied ledger against the embedded migrations; returns how many
/// are applied.
pub(super) async fn verify(conn: &mut SqliteConnection) -> Result<usize, SqliteStoreError> {
    let applied = sqlx::query("SELECT version, checksum FROM schema_migrations ORDER BY version")
        .fetch_all(&mut *conn)
        .await?;
    let known = MIGRATIONS.last().map_or(0, |last| last.version);
    for (index, row) in applied.iter().enumerate() {
        let version: i64 = row.try_get("version")?;
        let stored: String = row.try_get("checksum")?;
        if version > known {
            return Err(SqliteStoreError::FutureVersion {
                found: version,
                known,
            });
        }
        let migration = &MIGRATIONS[index];
        if migration.version != version {
            return Err(SqliteStoreError::MigrationGap { version });
        }
        if checksum(migration.sql) != stored {
            return Err(SqliteStoreError::ChecksumMismatch { version });
        }
    }
    Ok(applied.len())
}

/// Bring the schema to the newest embedded version; returns that version.
pub(super) async fn apply(conn: &mut SqliteConnection) -> Result<i64, SqliteStoreError> {
    let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;
    (&mut *tx).execute(LEDGER).await?;
    let applied = verify(&mut tx).await?;
    for migration in &MIGRATIONS[applied..] {
        (&mut *tx).execute(migration.sql).await?;
        if migration.version == BLAME_INDEX_VERSION {
            // Index the records written before the blame index existed.
            super::history::backfill_fields(&mut tx)
                .await
                .map_err(|error| {
                    SqliteStoreError::Database(sqlx::Error::Protocol(error.to_string()))
                })?;
        }
        sqlx::query(
            "INSERT INTO schema_migrations (version, checksum, applied_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%S+00:00', 'now'))",
        )
        .bind(migration.version)
        .bind(checksum(migration.sql))
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE store_meta SET schema_version = ?1 WHERE id = 1")
            .bind(migration.version)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(MIGRATIONS.last().map_or(0, |last| last.version))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{ConnectOptions, Connection, Executor, Row, SqliteConnection};

    use super::{LEDGER, MIGRATIONS, apply, checksum, verify};

    struct RemoveFile(PathBuf);

    #[tokio::test]
    async fn migration_24_is_additive_and_checks_correlation_shapes() {
        let mut conn = SqliteConnectOptions::new()
            .filename(":memory:")
            .connect()
            .await
            .expect("db");
        conn.execute(LEDGER).await.expect("ledger");
        for migration in &MIGRATIONS[..23] {
            conn.execute(migration.sql).await.expect("v23 migration");
            sqlx::query("INSERT INTO schema_migrations VALUES (?1, ?2, 'then')")
                .bind(migration.version)
                .bind(checksum(migration.sql))
                .execute(&mut conn)
                .await
                .expect("ledger");
        }
        conn.execute(
            "INSERT INTO extractions (id, at, member_ids, model, prompt, raw_response, \
             request_count, outcome, guardrail, message_ids, proposal_ids) VALUES ('old', \
             '2026-09-01T00:00:00+00:00', '[]', 'm', 'p', 'r', 1, 'unknown', '{}', '[]', '[]');
             INSERT INTO chat_interactions (id, at, question, reply, outcome, clean_retry, \
             withheld, guardrail, request_count) VALUES ('old', '2026-09-01T00:00:00+00:00', \
             'q', 'a', 'answered', 0, 0, '{}', 1);
             INSERT INTO chat_rounds (interaction_id, ord, model, tool_bundles, tools, \
             tool_calls) VALUES ('old', 0, 'm', '[]', '[]', '[]');",
        )
        .await
        .expect("v23 rows");
        assert_eq!(apply(&mut conn).await.expect("additive"), 24);
        let old: (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT e.session_id, e.request_ids, c.session_id, r.request_ids \
                 FROM extractions e, chat_interactions c, chat_rounds r WHERE e.id = 'old'",
        )
        .fetch_one(&mut conn)
        .await
        .expect("old rows preserved");
        assert_eq!(old, (None, None, None, None));
        for (ids, valid) in [
            ("[\"kanade-chat-1-1\"]", true),
            ("[]", false),
            ("{}", false),
            ("not json", false),
        ] {
            let result = sqlx::query(
                "INSERT INTO chat_rounds (interaction_id, ord, model, tool_bundles, tools, \
                 tool_calls, request_ids) VALUES ('old', ?1, 'm', '[]', '[]', '[]', ?2)",
            )
            .bind(if valid { 1 } else { 9 })
            .bind(ids)
            .execute(&mut conn)
            .await;
            assert_eq!(result.is_ok(), valid, "{ids}: {result:?}");
        }
        for (session, valid) in [("kanade-extraction-1", true), ("", false)] {
            let result = sqlx::query(
                "INSERT INTO extractions (id, at, member_ids, model, prompt, raw_response, \
                 request_count, outcome, guardrail, message_ids, proposal_ids, session_id) \
                 VALUES (?1, '2026-09-01T00:00:00+00:00', '[]', 'm', 'p', 'r', 1, 'no_change', \
                 '{}', '[]', '[]', ?1)",
            )
            .bind(session)
            .execute(&mut conn)
            .await;
            assert_eq!(result.is_ok(), valid, "{session:?}: {result:?}");
        }
        assert!(
            conn.execute("UPDATE extractions SET session_id = 'changed'")
                .await
                .is_err(),
            "insert-only trigger remains"
        );
        conn.close().await.expect("close");
    }

    #[tokio::test]
    async fn migration_21_is_additive_and_checks_reasoning_bytes_and_counts() {
        let mut conn = SqliteConnectOptions::new()
            .filename(":memory:")
            .connect()
            .await
            .expect("db");
        conn.execute(LEDGER).await.expect("ledger");
        for migration in &MIGRATIONS[..20] {
            conn.execute(migration.sql).await.expect("v20 migration");
            sqlx::query("INSERT INTO schema_migrations VALUES (?1, ?2, 'then')")
                .bind(migration.version)
                .bind(checksum(migration.sql))
                .execute(&mut conn)
                .await
                .expect("ledger");
        }
        let insert = "INSERT INTO extractions (id, at, member_ids, model, reasoning, prompt, raw_response, request_count, outcome, guardrail, message_ids, proposal_ids";
        conn.execute(format!("{insert}) VALUES ('old', '2026-09-01T00:00:00+00:00', '[]', 'm', 'low', 'p', 'r', 1, 'unknown', '{{}}', '[]', '[]')").as_str()).await.expect("old row");
        assert_eq!(apply(&mut conn).await.expect("additive"), 24);
        let old: (String, Option<String>, Option<i64>) = sqlx::query_as("SELECT reasoning, reasoning_content, reasoning_tokens FROM extractions WHERE id = 'old'").fetch_one(&mut conn).await.expect("old row preserved");
        assert_eq!(old, ("low".into(), None, None));
        for (id, text, count, valid) in [
            ("zero", "summary".to_owned(), 0, true),
            ("negative", "summary".to_owned(), -1, false),
            ("empty", String::new(), 1, false),
            ("large", "奏".repeat(21846), 1, false),
        ] {
            let result = sqlx::query(&format!("{insert}, reasoning_content, reasoning_tokens) VALUES (?1, '2026-09-01T00:00:00+00:00', '[]', 'm', 'low', 'p', 'r', 1, 'no_change', '{{}}', '[]', '[]', ?2, ?3)"))
                .bind(id).bind(text).bind(count).execute(&mut conn).await;
            assert_eq!(result.is_ok(), valid, "{id}: {result:?}");
        }
        assert!(
            conn.execute("UPDATE extractions SET reasoning_content = 'changed'")
                .await
                .is_err(),
            "insert-only trigger remains"
        );
        conn.close().await.expect("close");
    }

    impl Drop for RemoveFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// 0015 rebuilds `extractions`/`extraction_members` in place: rows, the
    /// member links and foreign keys survive, and `identity_leak` is allowed.
    #[tokio::test]
    async fn the_extraction_rebuild_keeps_rows_and_links() {
        let mut conn = SqliteConnection::connect("sqlite::memory:")
            .await
            .expect("memory db");
        conn.execute("PRAGMA foreign_keys = ON").await.expect("fk");
        conn.execute(LEDGER).await.expect("ledger");
        for migration in &MIGRATIONS[..14] {
            conn.execute(migration.sql).await.expect("old migration");
            sqlx::query("INSERT INTO schema_migrations VALUES (?1, ?2, 'then')")
                .bind(migration.version)
                .bind(checksum(migration.sql))
                .execute(&mut conn)
                .await
                .expect("ledger row");
        }
        conn.execute(
            "INSERT INTO extractions (id, at, channel_id, member_ids, model, prompt, \
             raw_response, request_count, outcome, guardrail, message_ids, proposal_ids) \
             VALUES ('x-1', '2026-09-01T00:00:00.000000+00:00', '9', '[\"1\"]', 'm', 'p', \
             'r', 1, 'failed', '{}', '[]', '[]');
             INSERT INTO extraction_members VALUES ('x-1', '1');
             INSERT INTO reminder_cards VALUES (
                 '0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef',
                 'day_of', 'Tonight!', '2026-09-01T00:00:00.000000+00:00');",
        )
        .await
        .expect("v14 rows");
        assert_eq!(apply(&mut conn).await.expect("remaining migrations"), 24);
        let kept: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM extractions e JOIN extraction_members m \
             ON m.extraction_id = e.id WHERE e.id = 'x-1' AND e.refusals = '[]'",
        )
        .fetch_one(&mut conn)
        .await
        .expect("count");
        assert_eq!(kept, 1);
        let cards: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM reminder_cards WHERE heading = 'Tonight!'")
                .fetch_one(&mut conn)
                .await
                .expect("cards");
        assert_eq!(cards, 1, "the reminder-card rebuild preserves old rows");
        let violations = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut conn)
            .await
            .expect("fk check");
        assert!(violations.is_empty());
        let references: String =
            sqlx::query("SELECT sql FROM sqlite_master WHERE name = 'extraction_members'")
                .fetch_one(&mut conn)
                .await
                .expect("schema")
                .get("sql");
        assert!(
            references.contains("REFERENCES \"extractions\""),
            "{references}"
        );
        conn.execute(
            "INSERT INTO extractions (id, at, member_ids, model, prompt, raw_response, \
             request_count, outcome, guardrail, message_ids, proposal_ids) VALUES ('x-2', \
             '2026-09-01T00:00:00.000000+00:00', '[]', 'm', 'p', 'r', 0, 'identity_leak', \
             '{}', '[]', '[]')",
        )
        .await
        .expect("identity_leak is an outcome");
        assert!(
            conn.execute("UPDATE extractions SET model = 'n' WHERE id = 'x-1'")
                .await
                .is_err(),
            "still insert-only"
        );
    }

    #[tokio::test]
    async fn migration_18_allows_countdown_phrases_and_keeps_records_write_once() {
        let path = std::env::temp_dir().join(format!(
            "kanade-migrate-v18-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let _cleanup = RemoveFile(path.clone());
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let mut conn = options.connect().await.expect("fresh file");
        conn.execute("PRAGMA foreign_keys = ON").await.expect("fk");
        conn.execute(LEDGER).await.expect("ledger");
        for migration in &MIGRATIONS[..17] {
            conn.execute(migration.sql).await.expect("v17 migration");
            sqlx::query("INSERT INTO schema_migrations VALUES (?1, ?2, 'then')")
                .bind(migration.version)
                .bind(checksum(migration.sql))
                .execute(&mut conn)
                .await
                .expect("ledger row");
        }
        conn.execute(
            "INSERT INTO reminder_cards VALUES \
             ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', \
              'day_of', 'Today — Tue 29 Sep', 'then'); \
             INSERT INTO reminder_cards VALUES \
             ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', \
              'countdown_60', NULL, 'then');",
        )
        .await
        .expect("v17 cards");

        assert_eq!(apply(&mut conn).await.expect("v18"), 24);
        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT kind, heading FROM reminder_cards ORDER BY dedupe_key")
                .fetch_all(&mut conn)
                .await
                .expect("preserved rows");
        assert_eq!(
            rows,
            [
                ("day_of".into(), Some("Today — Tue 29 Sep".into())),
                ("countdown_60".into(), None),
            ]
        );
        conn.execute(
            "INSERT INTO reminder_cards VALUES \
             ('cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc', \
              'countdown_60', 'Onward!', 'then')",
        )
        .await
        .expect("countdown phrase is allowed");
        sqlx::query(
            "INSERT INTO digest_card_phrases VALUES \
             ('dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd', ?1, 'then')",
        )
        .bind("Let's go!")
        .execute(&mut conn)
        .await
        .expect("digest phrase");
        conn.close().await.expect("close after migration");

        let mut conn = SqliteConnectOptions::new()
            .filename(&path)
            .connect()
            .await
            .expect("reopen v18 file");
        assert_eq!(verify(&mut conn).await.expect("verified ledger"), 24);
        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT kind, heading FROM reminder_cards ORDER BY dedupe_key")
                .fetch_all(&mut conn)
                .await
                .expect("reopened records");
        assert_eq!(
            rows,
            [
                ("day_of".into(), Some("Today — Tue 29 Sep".into())),
                ("countdown_60".into(), None),
                ("countdown_60".into(), Some("Onward!".into())),
            ]
        );
        let phrase: String = sqlx::query_scalar("SELECT phrase FROM digest_card_phrases")
            .fetch_one(&mut conn)
            .await
            .expect("reopened phrase");
        assert_eq!(phrase, "Let's go!");
        assert!(
            conn.execute("UPDATE reminder_cards SET heading = 'changed'")
                .await
                .is_err(),
            "reminder records remain write-once"
        );
        assert!(
            conn.execute("UPDATE digest_card_phrases SET phrase = 'changed'")
                .await
                .is_err(),
            "digest phrases are write-once"
        );
        conn.close().await.expect("close reopened file");
    }

    /// 0019 adds nullable usage columns: old rows (native and imported) read
    /// NULL, a pair is all-or-nothing, negatives are refused and the logs
    /// stay insert-only, across a reopen of the file.
    #[tokio::test]
    async fn migration_19_adds_usage_columns_to_existing_logs() {
        /// Key, prompt, completion, estimate and their SQLite storage types.
        type UsageRow<K> = (K, Option<i64>, Option<i64>, Option<i64>, String);
        const EXTRACTION: &str = "INSERT INTO extractions (id, at, member_ids, model, prompt, \
            raw_response, request_count, outcome, guardrail, message_ids, proposal_ids";
        const ROUND: &str =
            "INSERT INTO chat_rounds (interaction_id, ord, model, tool_bundles, tools, tool_calls";
        let path = std::env::temp_dir().join(format!(
            "kanade-migrate-v19-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let _cleanup = RemoveFile(path.clone());
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let mut conn = options.connect().await.expect("fresh file");
        conn.execute("PRAGMA foreign_keys = ON").await.expect("fk");
        conn.execute(LEDGER).await.expect("ledger");
        for migration in &MIGRATIONS[..18] {
            conn.execute(migration.sql).await.expect("v18 migration");
            sqlx::query("INSERT INTO schema_migrations VALUES (?1, ?2, 'then')")
                .bind(migration.version)
                .bind(checksum(migration.sql))
                .execute(&mut conn)
                .await
                .expect("ledger row");
        }
        conn.execute(
            format!(
                "{EXTRACTION}) VALUES ('x-1', '2026-09-01T00:00:00.000000+00:00', '[]', 'm', \
                 'p', 'r', 1, 'no_change', '{{}}', '[]', '[]');
                 {EXTRACTION}) VALUES ('v4-7', '2026-08-01T00:00:00.000000+00:00', '[]', 'm', \
                 'p', 'r', 1, 'unknown', '{{}}', '[]', '[]');
                 INSERT INTO chat_interactions (id, at, question, reply, outcome, clean_retry, \
                 withheld, guardrail, request_count, prompt_tokens) VALUES ('c-1', \
                 '2026-09-01T00:00:00.000000+00:00', 'q', 'r', 'answered', 0, 0, '{{}}', 1, 9);
                 {ROUND}) VALUES ('c-1', 0, 'kanata/chat', '[]', '[]', '[]');"
            )
            .as_str(),
        )
        .await
        .expect("v18 rows");

        assert_eq!(apply(&mut conn).await.expect("v19"), 24);
        let usage = "prompt_tokens IS NULL AND completion_tokens IS NULL \
            AND prompt_estimate IS NULL";
        let unreported: i64 = sqlx::query_scalar(&format!(
            "SELECT (SELECT COUNT(*) FROM extractions WHERE {usage}) \
             + (SELECT COUNT(*) FROM chat_rounds WHERE {usage})"
        ))
        .fetch_one(&mut conn)
        .await
        .expect("old rows");
        assert_eq!(unreported, 3, "old rows report no usage");
        let totals: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT prompt_tokens, completion_tokens FROM chat_interactions WHERE id = 'c-1'",
        )
        .fetch_one(&mut conn)
        .await
        .expect("interaction totals");
        assert_eq!(totals, (Some(9), None), "interaction half pairs stay valid");

        let usage_columns = ", prompt_tokens, completion_tokens, prompt_estimate)";
        let extraction = |id: &str, values: &str| {
            format!(
                "{EXTRACTION}{usage_columns} VALUES ('{id}', '2026-09-02T00:00:00.000000+00:00', \
                 '[]', 'm', 'p', 'r', 1, 'no_change', '{{}}', '[]', '[]', {values})"
            )
        };
        let round = |ord: u32, values: &str| {
            format!("{ROUND}{usage_columns} VALUES ('c-1', {ord}, 'm', '[]', '[]', '[]', {values})")
        };
        conn.execute(extraction("x-2", "1200, 80, 1100").as_str())
            .await
            .expect("full pair and estimate");
        conn.execute(round(1, "900, 40, 950").as_str())
            .await
            .expect("full round pair and estimate");
        conn.execute(extraction("x-3", "NULL, NULL, 700").as_str())
            .await
            .expect("an estimate without reported usage");
        for (n, values) in [
            "5, NULL, NULL",
            "NULL, 5, NULL",
            "-1, 5, NULL",
            "5, -1, NULL",
            "NULL, NULL, -1",
        ]
        .into_iter()
        .enumerate()
        {
            let refused = conn
                .execute(extraction(&format!("bad-{n}"), values).as_str())
                .await
                .expect_err("extraction usage is checked");
            assert!(refused.to_string().contains("CHECK"), "{values}: {refused}");
            let refused = conn
                .execute(round(10 + u32::try_from(n).expect("small"), values).as_str())
                .await
                .expect_err("round usage is checked");
            assert!(refused.to_string().contains("CHECK"), "{values}: {refused}");
        }
        assert!(
            conn.execute("UPDATE extractions SET prompt_tokens = 1, completion_tokens = 1")
                .await
                .is_err(),
            "extractions stay insert-only"
        );
        assert!(
            conn.execute("UPDATE chat_rounds SET prompt_tokens = 1, completion_tokens = 1")
                .await
                .is_err(),
            "chat rounds stay insert-only"
        );
        let violations = sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&mut conn)
            .await
            .expect("fk check");
        assert!(violations.is_empty());
        conn.close().await.expect("close after migration");

        let mut conn = SqliteConnectOptions::new()
            .filename(&path)
            .connect()
            .await
            .expect("reopen v19 file");
        assert_eq!(verify(&mut conn).await.expect("verified ledger"), 24);
        let rows: Vec<UsageRow<String>> = sqlx::query_as(
            "SELECT id, prompt_tokens, completion_tokens, prompt_estimate, \
             typeof(prompt_tokens) || ',' || typeof(completion_tokens) || ',' \
             || typeof(prompt_estimate) FROM extractions ORDER BY id",
        )
        .fetch_all(&mut conn)
        .await
        .expect("reopened extractions");
        assert_eq!(
            rows,
            [
                ("v4-7".into(), None, None, None, "null,null,null".into()),
                ("x-1".into(), None, None, None, "null,null,null".into()),
                (
                    "x-2".into(),
                    Some(1200),
                    Some(80),
                    Some(1100),
                    "integer,integer,integer".into()
                ),
                (
                    "x-3".into(),
                    None,
                    None,
                    Some(700),
                    "null,null,integer".into()
                ),
            ]
        );
        let rounds: Vec<UsageRow<i64>> = sqlx::query_as(
            "SELECT ord, prompt_tokens, completion_tokens, prompt_estimate, \
             typeof(prompt_tokens) || ',' || typeof(completion_tokens) || ',' \
             || typeof(prompt_estimate) FROM chat_rounds ORDER BY ord",
        )
        .fetch_all(&mut conn)
        .await
        .expect("reopened rounds");
        assert_eq!(
            rounds,
            [
                (0, None, None, None, "null,null,null".into()),
                (
                    1,
                    Some(900),
                    Some(40),
                    Some(950),
                    "integer,integer,integer".into()
                ),
            ]
        );
        conn.close().await.expect("close reopened file");
    }

    /// 0016 adds the chat turn facts to existing rows: nullable facts stay
    /// NULL, `clean` defaults to 0, and the insert-only triggers still hold.
    #[tokio::test]
    async fn chat_turn_facts_default_on_existing_rows() {
        let mut conn = SqliteConnection::connect("sqlite::memory:")
            .await
            .expect("memory db");
        conn.execute("PRAGMA foreign_keys = ON").await.expect("fk");
        conn.execute(LEDGER).await.expect("ledger");
        for migration in &MIGRATIONS[..15] {
            conn.execute(migration.sql).await.expect("old migration");
            sqlx::query("INSERT INTO schema_migrations VALUES (?1, ?2, 'then')")
                .bind(migration.version)
                .bind(checksum(migration.sql))
                .execute(&mut conn)
                .await
                .expect("ledger row");
        }
        conn.execute(
            "INSERT INTO chat_interactions (id, at, question, reply, outcome, clean_retry, \
             withheld, guardrail, request_count) VALUES ('c-1', \
             '2026-09-01T00:00:00.000000+00:00', 'q', 'r', 'answered', 0, 0, '{}', 1);
             INSERT INTO chat_rounds (interaction_id, ord, model, tool_bundles, tools, \
             tool_calls) VALUES ('c-1', 0, 'kanata/chat', '[]', '[]', '[]');
             INSERT INTO chat_masked VALUES ('c-1', '[]', 'r', '[]');",
        )
        .await
        .expect("v15 rows");
        assert_eq!(apply(&mut conn).await.expect("0016+"), 24);
        let row = sqlx::query(
            "SELECT c.persona, c.profile, c.profile_source, c.error_code, r.route, r.clean, \
             r.model FROM chat_interactions c JOIN chat_rounds r ON r.interaction_id = c.id",
        )
        .fetch_one(&mut conn)
        .await
        .expect("kept");
        for column in [
            "persona",
            "profile",
            "profile_source",
            "error_code",
            "route",
        ] {
            assert_eq!(row.get::<Option<String>, _>(column), None, "{column}");
        }
        assert_eq!(row.get::<i64, _>("clean"), 0);
        assert_eq!(row.get::<String, _>("model"), "kanata/chat");
        let masked: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_masked")
            .fetch_one(&mut conn)
            .await
            .expect("masked");
        assert_eq!(masked, 1);
        assert!(
            conn.execute("UPDATE chat_rounds SET clean = 1")
                .await
                .is_err(),
            "still insert-only"
        );
        assert!(
            conn.execute(
                "INSERT INTO chat_rounds (interaction_id, ord, model, tool_bundles, tools, \
                 tool_calls, route) VALUES ('c-1', 1, 'm', '[]', '[]', '[]', 'cloud')",
            )
            .await
            .is_err(),
            "route is checked"
        );
    }

    /// 0022 rebuilds `chat_interactions` for outcome `profanity` with
    /// foreign keys on: every row (masked history included) and its rounds,
    /// tools and masked view keep their parent, the indexes and the
    /// insert-only trigger come back, and the children's schema is untouched.
    #[tokio::test]
    async fn the_chat_rebuild_keeps_rows_children_and_indexes() {
        let mut conn = SqliteConnection::connect("sqlite::memory:")
            .await
            .expect("memory db");
        conn.execute("PRAGMA foreign_keys = ON").await.expect("fk");
        conn.execute(LEDGER).await.expect("ledger");
        for migration in &MIGRATIONS[..21] {
            conn.execute(migration.sql).await.expect("old migration");
            sqlx::query("INSERT INTO schema_migrations VALUES (?1, ?2, 'then')")
                .bind(migration.version)
                .bind(checksum(migration.sql))
                .execute(&mut conn)
                .await
                .expect("ledger row");
        }
        let schema = "SELECT type, name, sql FROM sqlite_master \
             WHERE tbl_name IN ('chat_rounds', 'chat_tools', 'chat_masked') ORDER BY name";
        // The children as every later migration but 0022 leaves them.
        let mut skipped = SqliteConnection::connect("sqlite::memory:")
            .await
            .expect("memory db");
        for migration in MIGRATIONS[..21].iter().chain(&MIGRATIONS[22..]) {
            skipped.execute(migration.sql).await.expect("migration");
        }
        let children: Vec<(String, String, Option<String>)> = sqlx::query_as(schema)
            .fetch_all(&mut skipped)
            .await
            .expect("children");
        conn.execute(
            "INSERT INTO chat_interactions (id, at, channel_id, member_id, question, reply, \
             outcome, error, clean_retry, withheld, guardrail, request_count, latency_ms, \
             persona, profile_source, error_code) VALUES \
             ('c-1', '2026-09-01T00:00:00.000000+00:00', '9', '1', 'q', 'r', 'answered', \
              NULL, 1, 0, '{\"context\":{}}', 2, 40, 'kanade', 'default', NULL), \
             ('c-2', '2026-09-02T00:00:00.000000+00:00', '9', '2', 'q2', 'r2', \
              'content_blocked', 'blocked', 0, 1, '{\"content_filter\":true}', 1, 7, \
              NULL, NULL, 'content_blocked');
             INSERT INTO chat_rounds (interaction_id, ord, model, tool_bundles, tools, \
             tool_calls, route, clean, prompt_tokens, completion_tokens) VALUES \
             ('c-1', 0, 'kanata/chat', '[]', '[\"get_schedule\"]', '[]', 'homelab', 0, 10, 2);
             INSERT INTO chat_tools VALUES ('c-1', 'get_schedule');
             INSERT INTO chat_masked VALUES ('c-2', '[]', 'r2', '[]');",
        )
        .await
        .expect("v21 rows");
        assert_eq!(apply(&mut conn).await.expect("0022"), 24);
        let rows: Vec<(String, String, i64, i64, String, Option<String>)> = sqlx::query_as(
            "SELECT id, outcome, clean_retry, withheld, guardrail, persona \
             FROM chat_interactions ORDER BY id",
        )
        .fetch_all(&mut conn)
        .await
        .expect("rows");
        assert_eq!(
            rows,
            [
                (
                    "c-1".into(),
                    "answered".into(),
                    1,
                    0,
                    r#"{"context":{}}"#.into(),
                    Some("kanade".into())
                ),
                (
                    "c-2".into(),
                    "content_blocked".into(),
                    0,
                    1,
                    r#"{"content_filter":true}"#.into(),
                    None
                ),
            ]
        );
        let joined: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM chat_rounds r JOIN chat_interactions c \
             ON c.id = r.interaction_id) + (SELECT COUNT(*) FROM chat_tools t \
             JOIN chat_interactions c ON c.id = t.interaction_id) + (SELECT COUNT(*) \
             FROM chat_masked m JOIN chat_interactions c ON c.id = m.interaction_id)",
        )
        .fetch_one(&mut conn)
        .await
        .expect("children");
        assert_eq!(joined, 3);
        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&mut conn)
                .await
                .expect("fk check")
                .is_empty()
        );
        let after: Vec<(String, String, Option<String>)> = sqlx::query_as(schema)
            .fetch_all(&mut conn)
            .await
            .expect("children");
        assert_eq!(after, children, "children are not rebuilt");
        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE tbl_name = 'chat_interactions' \
             AND type != 'table' AND sql IS NOT NULL ORDER BY name",
        )
        .fetch_all(&mut conn)
        .await
        .expect("indexes");
        assert_eq!(
            indexes,
            [
                "chat_channel",
                "chat_flags",
                "chat_interactions_no_update",
                "chat_outcome",
                "chat_recent"
            ]
        );
        let leftovers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'chat_interactions_v21'",
        )
        .fetch_one(&mut conn)
        .await
        .expect("leftovers");
        assert_eq!(leftovers, 0);
        conn.execute(
            "INSERT INTO chat_interactions (id, at, question, reply, outcome, clean_retry, \
             withheld, guardrail, request_count) VALUES ('c-3', \
             '2026-09-03T00:00:00.000000+00:00', 'q', 'r', 'profanity', 0, 0, '{}', 0)",
        )
        .await
        .expect("profanity is an outcome");
        assert!(
            conn.execute(
                "INSERT INTO chat_interactions (id, at, question, reply, outcome, \
                 clean_retry, withheld, guardrail, request_count) VALUES ('c-4', \
                 '2026-09-03T00:00:00.000000+00:00', 'q', 'r', 'rude', 0, 0, '{}', 0)",
            )
            .await
            .is_err(),
            "outcomes stay checked"
        );
        assert!(
            conn.execute("UPDATE chat_interactions SET reply = 'x' WHERE id = 'c-1'")
                .await
                .is_err(),
            "still insert-only"
        );
        assert!(
            conn.execute("INSERT INTO chat_tools VALUES ('nobody', 'get_schedule')")
                .await
                .is_err(),
            "children still reference the rebuilt table"
        );
    }

    /// Shipped migrations are frozen byte for byte: a deployed store refuses
    /// to open when an applied migration's checksum changes (a comment edit
    /// once took production down). Append new versions; never edit old ones.
    #[test]
    fn shipped_migrations_never_change() {
        const SHIPPED: &[(i64, &str)] = &[
            (
                1,
                "af627f54aca2d991cbf018410782ad8694b9a2d0ef681424e2e2c426d33884a1",
            ),
            (
                2,
                "a73db1e033e3093c46928dde2f9bc8d97578b967acbc387d35a5224758237521",
            ),
            (
                3,
                "4273dc1f916a8b2e36f3479b00aee8f3015ef5aacdd0f5a28d390011a2d7dda8",
            ),
            (
                4,
                "84632cd590244046b32f3635dc1496bff02b8c74f67da326b1e2d4a5b52e6b33",
            ),
            (
                5,
                "169b322d784e50d0444ea0fc6e60f99b394f49bb49c8236390cb2a5e0d53a522",
            ),
            (
                6,
                "894fcc6610ca0964bf9d95573c1f68269f70e736251ea6101520270221e17539",
            ),
            (
                7,
                "9d7284dd8cc0195ecfb0cc65875ac9ff62466c6ac4e52fa12c6bc2b52c342869",
            ),
            (
                8,
                "3f8e01e0ce526d853d9ac08697999152aa59324e4694132c74f84311c1df5a97",
            ),
            (
                9,
                "f2b69064c75bc424f8742f003380dbf917468bd2a5df26e2bd04f286861c2676",
            ),
            (
                10,
                "83ad2adac9c829e82697268cc8747acfd659b8df5f108b6e40fe8089c42ebd64",
            ),
            (
                11,
                "d3f619353cb49e52e13469268fe652c462d595f2cfb6d342cc957657d787aa69",
            ),
            (
                12,
                "d1361933a094edb7bb25759b8e762516ac40cd9606cd6b6ccf8b25f078632a3d",
            ),
            (
                13,
                "fae7047988d3992bba878ab0f817840c16299c51fbe61e08dc619d06cc2f68c6",
            ),
            (
                14,
                "d059a2aaea6841fe6ca6ba20d956367860b3a9466a46bac96c91e12fe855cfe1",
            ),
            (
                15,
                "f3cdc32e9f712a4f0ed4fd3232cf248c9ba1d0084a0cf4ae2a6ab32b5e282a7c",
            ),
            (
                16,
                "99017fd262b149fb129937aea2bf3d2ba8fa67099cd32ec9fc10764b069909f1",
            ),
            (
                17,
                "b6f9838f7491c655e2a7c8dc1da991cb7a6d50af643ad73dfc848c39132177bf",
            ),
            (
                18,
                "6d0d1c8dd0af2b052599dc4b98bd3084a91e71c1511bbccf7d0ddcb6c9ed4677",
            ),
            (
                19,
                "1f0963e7d81e4168cbe358dcfc8bba9ec02cee6c6ad64cbfdd06e5cc24587e98",
            ),
            (
                20,
                "331ed693c86a13f89fa871ff0db2442ee8b883c9afd1e11b78750441778b259e",
            ),
            (
                21,
                "ff347aead1d6c499d37b3890e099cf071c489b8fe099179130cb26094e7e2cd2",
            ),
            (
                22,
                "db6806e9835448c44e2bf6c4c8d75152efff4b0fcbb4b5a6db78a6dd79a178ee",
            ),
            (
                23,
                "f0f30dc1d0e4aa05ae9cf7e82bd371ea04e24ca07aba862eebf9a565ba63af01",
            ),
            (
                24,
                "276cbefd3cc6d1d93c38a83bd51c8373a0109aa60f0ac708d661f78a44f2aae2",
            ),
        ];
        for (version, sum) in SHIPPED {
            let migration = MIGRATIONS
                .iter()
                .find(|m| m.version == *version)
                .unwrap_or_else(|| panic!("migration {version} is gone"));
            assert_eq!(
                checksum(migration.sql),
                *sum,
                "migration {version} changed; add a new migration instead"
            );
        }
    }
}
