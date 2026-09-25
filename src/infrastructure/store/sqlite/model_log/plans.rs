//! `EXPLAIN QUERY PLAN` pins for the model-log statements on a fresh
//! in-memory schema (no `ANALYZE` statistics, as a new store has none).

use std::collections::BTreeSet;

use sqlx::{Connection, Row, SqliteConnection};

use super::{chat, extractions, messages, prune};

async fn schema() -> SqliteConnection {
    let mut conn = SqliteConnection::connect("sqlite::memory:")
        .await
        .expect("memory db");
    crate::infrastructure::store::sqlite::migrate::apply(&mut conn)
        .await
        .expect("migrate");
    conn
}

async fn plan(conn: &mut SqliteConnection, sql: &str) -> String {
    sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
        .fetch_all(&mut *conn)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
        .iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn statements() -> Vec<String> {
    [
        extractions::list_sql(),
        chat::list_sql(),
        messages::in_channel_sql(false),
        messages::in_channel_sql(true),
    ]
    .into_iter()
    .chain(extractions::FACET_SQL.iter().map(|sql| (*sql).to_owned()))
    .chain(chat::FACET_SQL.iter().map(|sql| (*sql).to_owned()))
    .chain(prune::statements().into_iter().map(|(sql, _)| sql))
    .collect()
}

#[tokio::test]
async fn lists_walk_the_time_index_without_sorting() {
    let mut conn = schema().await;
    for (sql, index) in [
        (
            extractions::list_sql(),
            "SCAN e USING INDEX extractions_recent",
        ),
        (chat::list_sql(), "SCAN c USING INDEX chat_recent"),
    ] {
        let plan = plan(&mut conn, &sql).await;
        assert!(plan.contains(index), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    }
    let pending = plan(&mut conn, &messages::in_channel_sql(true)).await;
    assert!(pending.contains("messages_unprocessed"), "{pending}");
}

#[tokio::test]
async fn prune_batches_seek_the_oldest_rows() {
    let mut conn = schema().await;
    for (sql, _) in prune::statements() {
        let plan = plan(&mut conn, &sql).await;
        assert!(!plan.contains("SCAN "), "{sql}\n{plan}");
    }
}

/// Every non-automatic index on the log tables serves some statement.
#[tokio::test]
async fn no_log_index_is_dead() {
    let mut conn = schema().await;
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND sql IS NOT NULL \
         AND tbl_name IN ('messages', 'extractions', 'extraction_members', \
         'chat_interactions', 'chat_rounds', 'chat_tools')",
    )
    .fetch_all(&mut conn)
    .await
    .expect("indexes");
    let mut used = BTreeSet::new();
    for sql in statements() {
        let plan = plan(&mut conn, &sql).await;
        used.extend(
            indexes
                .iter()
                .filter(|index| plan.contains(&format!("INDEX {index}")))
                .cloned(),
        );
    }
    let dead: Vec<&String> = indexes
        .iter()
        .filter(|index| !used.contains(*index))
        .collect();
    assert!(dead.is_empty(), "unused indexes: {dead:?}");
}
