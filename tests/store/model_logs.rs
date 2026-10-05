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

#[tokio::test]
async fn an_out_of_range_wire_reasoning_count_is_unknown_and_stored_as_null() {
    use axum::{Json, Router, routing::post};
    use kanade::infrastructure::llm::{
        ChatRequest, HttpProviderConfig, LlmProvider, Message, ModelCapabilities,
        OpenAiCompatibleProvider,
    };
    use serde_json::json;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("loopback");
    let address = listener.local_addr().expect("address");
    let router = Router::new().route("/v1/chat/completions", post(|| async {
        Json(json!({
            "model": "m", "choices": [{"message": {"content": "answer", "reasoning_content": "Summary."}, "finish_reason": "stop"}],
            "usage": {"completion_tokens_details": {"reasoning_tokens": 1_u64 << 63}}
        }))
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.expect("serve");
    });
    let provider =
        OpenAiCompatibleProvider::new(HttpProviderConfig::new(format!("http://{address}/v1")))
            .expect("provider");
    let request = ChatRequest {
        model: "m".into(),
        messages: vec![Message::User {
            content: "hi".into(),
        }],
        tools: vec![],
        output_schema: None,
        max_output_tokens: 16,
        reasoning: None,
        sampling: None,
    };
    let response = provider
        .complete_with(&request, &ModelCapabilities::minimal())
        .await
        .expect("response");
    assert_eq!(response.reasoning_tokens, None);
    server.abort();
    assert!(server.await.expect_err("server aborted").is_cancelled());

    let dir = TempDir::new();
    let config = dir.config("wire-reasoning-count");
    let store = SqliteStore::open(&config).await.expect("store");
    raw(&config, "INSERT INTO extractions (id, at, member_ids, model, prompt, raw_response, request_count, outcome, guardrail, message_ids, proposal_ids) VALUES ('template', '2026-09-20T00:00:00+00:00', '[]', 'm', 'p', 'r', 1, 'no_change', '{}', '[]', '[]')").await.expect("template");
    let mut log = store
        .load_extraction("template")
        .await
        .expect("load")
        .expect("template");
    log.id = "parsed".into();
    log.reasoning_content = response.reasoning_content;
    log.reasoning_tokens = response.reasoning_tokens;
    store
        .record_extraction(log.clone())
        .await
        .expect("row accepted");
    assert_eq!(
        store.load_extraction("parsed").await.expect("load"),
        Some(log)
    );
    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .read_only(true)
        .connect()
        .await
        .expect("raw read");
    let tokens: Option<i64> =
        sqlx::query_scalar("SELECT reasoning_tokens FROM extractions WHERE id = 'parsed'")
            .fetch_one(&mut conn)
            .await
            .expect("stored count");
    assert_eq!(
        tokens, None,
        "stored as SQL NULL, not zero or a refused row"
    );
    conn.close().await.expect("close raw");
    store.close().await.expect("close store");
}

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
         VALUES ('c-old', 0, 'm', '[]', '[]', '[]');
         ALTER TABLE extractions DROP COLUMN reasoning_content;
         ALTER TABLE extractions DROP COLUMN reasoning_tokens;
         ALTER TABLE chat_rounds DROP COLUMN reasoning_content;
         ALTER TABLE chat_rounds DROP COLUMN reasoning_tokens;
         ALTER TABLE extractions DROP COLUMN session_id;
         ALTER TABLE extractions DROP COLUMN request_ids;
         ALTER TABLE chat_interactions DROP COLUMN session_id;
         ALTER TABLE chat_rounds DROP COLUMN request_ids;
         DROP TABLE settings_changes;
         DELETE FROM schema_migrations WHERE version IN (21, 22, 23, 24);
         UPDATE store_meta SET schema_version = 20;",
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
    assert_eq!(
        (
            extraction.reasoning_content.as_ref(),
            extraction.reasoning_tokens
        ),
        (None, None)
    );
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
    assert_eq!(
        (
            chat.rounds[0].reasoning_content.as_ref(),
            chat.rounds[0].reasoning_tokens
        ),
        (None, None)
    );
    // Rows from before 0024 load with no correlation recorded.
    assert_eq!(
        (&extraction.session_id, extraction.request_ids.len()),
        (&None, 0)
    );
    assert_eq!(
        (&chat.session_id, chat.rounds[0].request_ids.len()),
        (&None, 0)
    );
    assert!(listed.items[0].request_ids.is_empty());
    let chats = store
        .list_chats(&ChatFilter {
            limit: 10,
            ..ChatFilter::default()
        })
        .await
        .expect("lists");
    assert_eq!(chats.items.len(), 1);
    store.close().await.expect("close");
    let store = SqliteStore::open(&config)
        .await
        .expect("reopen after additive migration");
    assert_eq!(store.schema_version().await.expect("version"), 24);
    let mut extraction = extraction;
    extraction.id = "x-reasoning".into();
    extraction.reasoning_content = Some("Stored extraction reasoning.".into());
    extraction.reasoning_tokens = Some(24);
    extraction.session_id = Some("kanade-extraction-0000abcd-3".into());
    extraction.request_ids = vec!["kanade-extraction-0000abcd-3-1".into()];
    store
        .record_extraction(extraction.clone())
        .await
        .expect("write reasoning");
    let mut chat = chat;
    chat.id = "c-reasoning".into();
    chat.rounds[0].reasoning_content = Some("Stored chat reasoning.".into());
    chat.rounds[0].reasoning_tokens = Some(32);
    chat.session_id = Some("kanade-chat-0000abcd-4".into());
    chat.rounds[0].request_ids = vec!["kanade-chat-0000abcd-4-1".into()];
    store
        .record_chat(chat.clone())
        .await
        .expect("write reasoning");
    store.close().await.expect("close");
    let store = SqliteStore::open(&config)
        .await
        .expect("reopen written reasoning");
    assert_eq!(
        store.load_extraction(&extraction.id).await.expect("load"),
        Some(extraction)
    );
    assert_eq!(store.load_chat(&chat.id).await.expect("load"), Some(chat));
    store.close().await.expect("close");
}

/// 0022 rebuilds `chat_interactions` on an existing store whose connection
/// enforces foreign keys: every row, its rounds, tools and historical masked
/// view survive the reopen, and `profanity` rows can then be written.
#[tokio::test]
async fn the_profanity_migration_keeps_existing_chat_rows_and_their_children() {
    use kanade::domain::model_log::ChatOutcome;

    let dir = TempDir::new();
    let config = dir.config("pre-profanity");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    tamper(
        &config,
        "INSERT INTO chat_interactions (id, at, channel_id, member_id, question, reply, \
         outcome, clean_retry, withheld, guardrail, request_count, persona) VALUES \
         ('c-plain', '2026-09-20T00:00:00+00:00', '9', '1', 'q', 'a', 'answered', 0, 0, \
          '{\"context\": {}}', 1, 'kanade'), \
         ('c-masked', '2026-09-21T00:00:00+00:00', '9', '2', 'q2', 'a2', 'content_blocked', \
          1, 1, '{\"pseudonymized\": true}', 2, NULL);
         INSERT INTO chat_rounds (interaction_id, ord, model, tool_bundles, tools, tool_calls) \
         VALUES ('c-plain', 0, 'm', '[]', '[\"get_schedule\"]', '[]');
         INSERT INTO chat_tools VALUES ('c-plain', 'get_schedule');
         INSERT INTO chat_masked VALUES ('c-masked', '[]', 'a2', '[]');
         ALTER TABLE extractions DROP COLUMN session_id;
         ALTER TABLE extractions DROP COLUMN request_ids;
         ALTER TABLE chat_interactions DROP COLUMN session_id;
         ALTER TABLE chat_rounds DROP COLUMN request_ids;
         DROP TABLE settings_changes;
         DELETE FROM schema_migrations WHERE version >= 22;
         UPDATE store_meta SET schema_version = 21;",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("migrates");
    assert_eq!(store.schema_version().await.expect("version"), 24);
    let plain = store
        .load_chat("c-plain")
        .await
        .expect("reads")
        .expect("kept");
    assert_eq!(plain.outcome, ChatOutcome::Answered);
    assert_eq!(plain.persona.as_deref(), Some("kanade"));
    assert_eq!(plain.rounds.len(), 1);
    assert_eq!(plain.rounds[0].tools, ["get_schedule"]);
    let masked = store
        .load_chat("c-masked")
        .await
        .expect("reads")
        .expect("kept");
    assert!(masked.withheld && masked.clean_retry);
    assert!(
        store
            .load_masked_chat("c-masked")
            .await
            .expect("reads")
            .is_some(),
        "the historical masked view is kept"
    );
    let tooled = store
        .list_chats(&ChatFilter {
            tool: Some("get_schedule".into()),
            limit: 10,
            ..ChatFilter::default()
        })
        .await
        .expect("lists");
    assert_eq!(tooled.items.len(), 1, "chat_tools still joins");
    let mut rude = plain.clone();
    rude.id = "c-rude".into();
    rude.outcome = ChatOutcome::Profanity;
    rude.guardrail = serde_json::json!({"profanity": {"side": "question", "word": "frick", "sent": "Language!"}});
    rude.rounds = Vec::new();
    store
        .record_chat(rude.clone())
        .await
        .expect("profanity row");
    store.close().await.expect("close");
    let store = SqliteStore::open(&config).await.expect("reopens");
    assert_eq!(store.load_chat("c-rude").await.expect("load"), Some(rude));
    store.close().await.expect("close");
}
