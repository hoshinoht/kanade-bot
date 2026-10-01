//! SQLite-only model-log and proposal guards: the 0007 triggers against raw
//! SQL, trigger errors surfacing as `Constraint`, and retention running in
//! more than one bounded batch.

use chrono::{TimeZone, Utc};
use kanade::domain::model_log::{
    ChatFilter, ExtractionFilter, ModelLogStore, PRUNE_BATCH, PruneCounts, WatchedMessage,
};
use kanade::domain::scheduler::StoreError;
use kanade::infrastructure::store::{SqliteStore, SqliteStoreConfig};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};

use crate::support::{TempDir, tamper};

async fn raw(config: &SqliteStoreConfig, sql: &str) -> Result<(), sqlx::Error> {
    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .connect()
        .await?;
    let result = sqlx::raw_sql(sql).execute(&mut conn).await.map(|_| ());
    conn.close().await?;
    result
}

async fn count(config: &SqliteStoreConfig, table: &str) -> i64 {
    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .read_only(true)
        .connect()
        .await
        .expect("raw connection");
    // Test-only: `table` is one of the fixed names below.
    let found = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(&mut conn)
        .await
        .expect("count");
    conn.close().await.expect("close");
    found
}

/// A trigger refusal: sqlx's extended code `SQLITE_CONSTRAINT_TRIGGER`.
fn assert_trigger(result: Result<(), sqlx::Error>, message: &str, what: &str) {
    let error = result.expect_err(what);
    let db = error.as_database_error().expect("database error");
    assert_eq!(db.code().as_deref(), Some("1811"), "{what}: {error}");
    assert!(db.message().contains(message), "{what}: {error}");
}

const DRAFT: &str = "INSERT INTO drafts (id, kind, title, author_kind, author_id, base_seq, \
    base_hash, base_revision, version, status, created_at, updated_at) VALUES";
const HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[tokio::test]
async fn proposal_facts_are_system_admin_rows_and_never_change() {
    let dir = TempDir::new();
    let config = dir.config("proposal-triggers");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(
        &config,
        &format!(
            "{DRAFT} ('by-admin', 'admin', 't', 'admin', 'root', 0, '{HASH}', 0, 1, 'open', \
             '2026-09-20T00:00:00+00:00', '2026-09-20T00:00:00+00:00');
             {DRAFT} ('by-request', 'request', 't', 'system', 'x', 0, '{HASH}', 0, 1, 'open', \
             '2026-09-20T00:00:00+00:00', '2026-09-20T00:00:00+00:00');
             {DRAFT} ('by-system', 'admin', 't', 'system', 'extraction', 0, '{HASH}', 0, 1, \
             'submitted', '2026-09-20T00:00:00+00:00', '2026-09-20T00:00:00+00:00');"
        ),
    )
    .await;
    let insert = |id: &str| {
        format!(
            "INSERT INTO draft_proposals (draft_id, source, source_id, supersede_key, expires_at) \
             VALUES ('{id}', 'extraction', 'x-1', NULL, '2026-09-21T00:00:00+00:00')"
        )
    };
    for id in ["by-admin", "by-request", "absent"] {
        assert_trigger(
            raw(&config, &insert(id)).await,
            "system-authored admin-kind draft",
            id,
        );
    }
    raw(&config, &insert("by-system"))
        .await
        .expect("a system-authored admin row takes facts");
    for statement in [
        "UPDATE draft_proposals SET source_id = 'x-2'",
        "DELETE FROM draft_proposals",
    ] {
        assert_trigger(
            raw(&config, statement).await,
            "proposal facts never change",
            statement,
        );
    }
}

#[tokio::test]
async fn logs_refuse_updates_but_allow_deletes() {
    let dir = TempDir::new();
    let config = dir.config("log-triggers");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(
        &config,
        "INSERT INTO extractions (id, at, member_ids, model, prompt, raw_response, request_count, \
         outcome, guardrail, message_ids, proposal_ids) VALUES ('x-1', \
         '2026-09-20T00:00:00+00:00', '[]', 'm', 'p', 'r', 1, 'no_change', '{}', '[]', '[]');
         INSERT INTO chat_interactions (id, at, question, reply, outcome, clean_retry, withheld, \
         guardrail, request_count) VALUES ('c-1', '2026-09-20T00:00:00+00:00', 'q', 'a', \
         'answered', 0, 0, '{}', 1);
         INSERT INTO chat_rounds (interaction_id, ord, model, tool_bundles, tools, tool_calls) \
         VALUES ('c-1', 0, 'm', '[]', '[]', '[]');",
    )
    .await;
    for (statement, message) in [
        (
            "UPDATE extractions SET outcome = 'failed'",
            "extraction logs are insert-only",
        ),
        (
            "UPDATE chat_interactions SET reply = 'x'",
            "chat logs are insert-only",
        ),
        (
            "UPDATE chat_rounds SET model = 'x'",
            "chat logs are insert-only",
        ),
    ] {
        assert_trigger(raw(&config, statement).await, message, statement);
    }
    raw(
        &config,
        "DELETE FROM chat_rounds; DELETE FROM chat_interactions; DELETE FROM extractions;",
    )
    .await
    .expect("retention deletes are allowed");
    assert_eq!(count(&config, "extractions").await, 0);
    assert_eq!(count(&config, "chat_interactions").await, 0);
}

#[tokio::test]
async fn a_trigger_refusal_is_a_constraint_error() {
    let dir = TempDir::new();
    let config = dir.config("trigger-constraint");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(
        &config,
        "CREATE TRIGGER refuse_boom BEFORE INSERT ON messages WHEN NEW.id = 'boom'
         BEGIN SELECT RAISE(ABORT, 'refused by trigger'); END;",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let result = store
        .upsert_message(WatchedMessage {
            id: "boom".into(),
            channel_id: "900".into(),
            author_id: "1".into(),
            created_at: Utc
                .with_ymd_and_hms(2026, 9, 20, 0, 0, 0)
                .single()
                .expect("instant"),
            edited_at: None,
            content: "x".into(),
            processed_at: None,
        })
        .await;
    assert!(
        matches!(&result, Err(StoreError::Constraint(detail)) if detail.contains("refused by trigger")),
        "{result:?}"
    );
    store.close().await.expect("close");
}

#[tokio::test]
async fn retention_runs_in_bounded_batches_until_done() {
    let dir = TempDir::new();
    let config = dir.config("prune-batches");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    let old = PRUNE_BATCH * 2 + 1;
    // `n` old rows at 2026-01-01 plus one of each kept: a recent log, and an
    // old unprocessed message.
    tamper(
        &config,
        &format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {old})
             INSERT INTO extractions (id, at, member_ids, model, prompt, raw_response, \
             request_count, outcome, guardrail, message_ids, proposal_ids) \
             SELECT printf('x-%05d', i), printf('2026-01-01T00:%02d:%02d+00:00', i / 60, i % 60), \
             '[\"1\"]', 'm', 'p', 'r', 1, 'no_change', '{{}}', '[]', '[]' FROM n;
             INSERT INTO extraction_members (extraction_id, member_id) \
             SELECT id, '1' FROM extractions;
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {old})
             INSERT INTO chat_interactions (id, at, question, reply, outcome, clean_retry, \
             withheld, guardrail, request_count) \
             SELECT printf('c-%05d', i), printf('2026-01-01T00:%02d:%02d+00:00', i / 60, i % 60), \
             'q', 'a', 'answered', 0, 0, '{{}}', 1 FROM n;
             INSERT INTO chat_rounds (interaction_id, ord, model, tool_bundles, tools, tool_calls) \
             SELECT id, 0, 'm', '[]', '[\"t\"]', '[]' FROM chat_interactions;
             INSERT INTO chat_tools (interaction_id, tool) SELECT id, 't' FROM chat_interactions;
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {old})
             INSERT INTO messages (id, channel_id, author_id, created_at, content, processed_at) \
             SELECT printf('m-%05d', i), '900', '1', \
             printf('2026-01-01T00:%02d:%02d+00:00', i / 60, i % 60), 'x', \
             '2026-01-02T00:00:00+00:00' FROM n;
             INSERT INTO extractions (id, at, member_ids, model, prompt, raw_response, \
             request_count, outcome, guardrail, message_ids, proposal_ids) VALUES ('x-new', \
             '2026-09-20T00:00:00+00:00', '[]', 'm', 'p', 'r', 1, 'no_change', '{{}}', '[]', '[]');
             INSERT INTO chat_interactions (id, at, question, reply, outcome, clean_retry, \
             withheld, guardrail, request_count) VALUES ('c-new', '2026-09-20T00:00:00+00:00', \
             'q', 'a', 'answered', 0, 0, '{{}}', 1);
             INSERT INTO messages (id, channel_id, author_id, created_at, content) VALUES \
             ('m-pending', '900', '1', '2026-01-01T00:00:00+00:00', 'x');"
        ),
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let before = Utc
        .with_ymd_and_hms(2026, 6, 1, 0, 0, 0)
        .single()
        .expect("instant");
    let old = u64::from(old);
    assert_eq!(
        store.prune_model_logs(before).await.expect("prune"),
        PruneCounts {
            extractions: old,
            chats: old,
            messages: old,
            notices: 0,
        },
        "more than two batches of each"
    );
    assert_eq!(
        store.prune_model_logs(before).await.expect("again"),
        PruneCounts::default()
    );
    store.close().await.expect("close");
    for (table, left) in [
        ("extractions", 1),
        ("extraction_members", 0),
        ("chat_interactions", 1),
        ("chat_rounds", 0),
        ("chat_tools", 0),
        ("messages", 1),
    ] {
        assert_eq!(count(&config, table).await, left, "{table}");
    }
}

#[tokio::test]
async fn rows_logged_before_context_facts_still_read_after_a_reopen() {
    let dir = TempDir::new();
    let config = dir.config("pre-context-rows");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    // Guardrails as written before `guardrail.context` existed.
    tamper(
        &config,
        "INSERT INTO extractions (id, at, member_ids, model, prompt, raw_response, request_count, \
         outcome, guardrail, message_ids, proposal_ids) VALUES ('x-old', \
         '2026-09-20T00:00:00+00:00', '[]', 'm', 'p', 'r', 1, 'no_change', \
         '{\"external_unmasked\": true}', '[]', '[]');
         INSERT INTO chat_interactions (id, at, question, reply, outcome, clean_retry, withheld, \
         guardrail, request_count) VALUES ('c-old', '2026-09-20T00:00:00+00:00', 'q', 'a', \
         'answered', 0, 0, '{}', 1);
         INSERT INTO chat_rounds (interaction_id, ord, model, tool_bundles, tools, tool_calls) \
         VALUES ('c-old', 0, 'm', '[]', '[]', '[]');",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let extraction = store
        .load_extraction("x-old")
        .await
        .expect("reads")
        .expect("kept");
    assert_eq!(
        extraction.guardrail,
        serde_json::json!({"external_unmasked": true})
    );
    assert!(extraction.guardrail.get("context").is_none());
    let listed = store
        .list_extractions(&ExtractionFilter {
            limit: 10,
            ..ExtractionFilter::default()
        })
        .await
        .expect("lists");
    assert_eq!(listed.items.len(), 1);
    let chat = store
        .load_chat("c-old")
        .await
        .expect("reads")
        .expect("kept");
    assert_eq!(chat.guardrail, serde_json::json!({}));
    let chats = store
        .list_chats(&ChatFilter {
            limit: 10,
            ..ChatFilter::default()
        })
        .await
        .expect("lists");
    assert_eq!(chats.items.len(), 1);
    store.close().await.expect("close");
}
