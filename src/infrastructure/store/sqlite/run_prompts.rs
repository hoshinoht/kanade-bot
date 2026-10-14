//! `RunPromptStore` over `run_prompts` (0035): constant SQL, reads on a
//! reader connection, one `BEGIN IMMEDIATE` per write.

use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteRow};

use super::SqliteStore;
use super::rows::instant;
use super::schedule::store_error;
use crate::domain::completion::{
    PromptClose, PromptFuture, PromptOutcome, RunPrompt, RunPromptStore,
};
use crate::domain::scheduler::StoreError;

const COLUMNS: &str = "run_id, ask, ends_at, due_at, cutoff_at, channel_id, message_id, \
     outcome, decided_by, decided_at, message_settled";

fn corrupt(column: &str, detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("run_prompts.{column} is unreadable: {detail}"))
}

fn decode(row: &SqliteRow) -> Result<RunPrompt, StoreError> {
    let text = |column: &str| -> Result<String, StoreError> {
        row.try_get(column).map_err(|error| corrupt(column, error))
    };
    let optional = |column: &str| -> Result<Option<String>, StoreError> {
        row.try_get(column).map_err(|error| corrupt(column, error))
    };
    let at = |column: &str| {
        crate::domain::time::from_iso(&text(column)?).map_err(|error| corrupt(column, error))
    };
    let ask: i64 = row.try_get("ask").map_err(|error| corrupt("ask", error))?;
    Ok(RunPrompt {
        run_id: text("run_id")?,
        ask: u32::try_from(ask).map_err(|error| corrupt("ask", error))?,
        ends_at: at("ends_at")?,
        due_at: at("due_at")?,
        cutoff_at: at("cutoff_at")?,
        channel_id: optional("channel_id")?,
        message_id: optional("message_id")?,
        outcome: optional("outcome")?
            .map(|text| PromptOutcome::parse(&text).ok_or_else(|| corrupt("outcome", text)))
            .transpose()?,
        decided_by: optional("decided_by")?,
        decided_at: optional("decided_at")?
            .map(|text| crate::domain::time::from_iso(&text))
            .transpose()
            .map_err(|error| corrupt("decided_at", error))?,
        message_settled: row
            .try_get::<i64, _>("message_settled")
            .map_err(|error| corrupt("message_settled", error))?
            != 0,
    })
}

async fn insert(conn: &mut SqliteConnection, prompt: &RunPrompt) -> Result<(), StoreError> {
    let taken = sqlx::query(
        "SELECT 1 FROM run_prompts WHERE run_id = ?1 AND (ask = ?2 OR outcome IS NULL)",
    )
    .bind(&prompt.run_id)
    .bind(i64::from(prompt.ask))
    .fetch_optional(&mut *conn)
    .await
    .map_err(store_error)?;
    if taken.is_some() {
        return Err(StoreError::Constraint(
            "the run already has this ask or an open one".into(),
        ));
    }
    sqlx::query(&format!(
        "INSERT INTO run_prompts ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
    ))
    .bind(&prompt.run_id)
    .bind(i64::from(prompt.ask))
    .bind(instant(&prompt.ends_at)?)
    .bind(instant(&prompt.due_at)?)
    .bind(instant(&prompt.cutoff_at)?)
    .bind(&prompt.channel_id)
    .bind(&prompt.message_id)
    .bind(prompt.outcome.map(PromptOutcome::as_str))
    .bind(&prompt.decided_by)
    .bind(prompt.decided_at.as_ref().map(instant).transpose()?)
    .bind(i64::from(prompt.message_settled))
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

async fn close(
    conn: &mut SqliteConnection,
    run_id: &str,
    ask: u32,
    close: &PromptClose,
) -> Result<bool, StoreError> {
    let changed = sqlx::query(
        "UPDATE run_prompts SET outcome = ?3, decided_by = ?4, decided_at = ?5 \
         WHERE run_id = ?1 AND ask = ?2 AND outcome IS NULL",
    )
    .bind(run_id)
    .bind(i64::from(ask))
    .bind(close.outcome.as_str())
    .bind(&close.decided_by)
    .bind(instant(&close.at)?)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?
    .rows_affected();
    if changed != 1 {
        return Ok(false);
    }
    if let Some(next) = &close.next {
        insert(conn, next).await?;
    }
    Ok(true)
}

impl RunPromptStore for SqliteStore {
    fn create_run_prompt(&self, prompt: RunPrompt) -> PromptFuture<'_, ()> {
        Box::pin(async move {
            prompt.check_new()?;
            write_txn!(self, tx, insert(&mut tx, &prompt))
        })
    }

    fn run_prompt(&self, run_id: String, ask: u32) -> PromptFuture<'_, Option<RunPrompt>> {
        Box::pin(async move {
            read_txn!(self, tx, async {
                sqlx::query(&format!(
                    "SELECT {COLUMNS} FROM run_prompts WHERE run_id = ?1 AND ask = ?2"
                ))
                .bind(&run_id)
                .bind(i64::from(ask))
                .fetch_optional(&mut *tx)
                .await
                .map_err(store_error)?
                .as_ref()
                .map(decode)
                .transpose()
            })
        })
    }

    fn latest_run_prompt(&self, run_id: String) -> PromptFuture<'_, Option<RunPrompt>> {
        Box::pin(async move {
            read_txn!(self, tx, async {
                sqlx::query(&format!(
                    "SELECT {COLUMNS} FROM run_prompts WHERE run_id = ?1 \
                     ORDER BY ask DESC LIMIT 1"
                ))
                .bind(&run_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(store_error)?
                .as_ref()
                .map(decode)
                .transpose()
            })
        })
    }

    fn open_run_prompts(&self) -> PromptFuture<'_, Vec<RunPrompt>> {
        Box::pin(async move {
            read_txn!(self, tx, async {
                sqlx::query(&format!(
                    "SELECT {COLUMNS} FROM run_prompts WHERE outcome IS NULL \
                     ORDER BY due_at, run_id"
                ))
                .fetch_all(&mut *tx)
                .await
                .map_err(store_error)?
                .iter()
                .map(decode)
                .collect()
            })
        })
    }

    fn close_run_prompt(
        &self,
        run_id: String,
        ask: u32,
        close: PromptClose,
    ) -> PromptFuture<'_, bool> {
        Box::pin(async move {
            close.check(&run_id)?;
            write_txn!(self, tx, self::close(&mut tx, &run_id, ask, &close))
        })
    }

    fn set_run_prompt_message(
        &self,
        run_id: String,
        ask: u32,
        channel_id: String,
        message_id: String,
    ) -> PromptFuture<'_, ()> {
        Box::pin(async move {
            write_txn!(self, tx, async {
                sqlx::query(
                    "UPDATE run_prompts SET channel_id = ?3, message_id = ?4 \
                     WHERE run_id = ?1 AND ask = ?2",
                )
                .bind(&run_id)
                .bind(i64::from(ask))
                .bind(&channel_id)
                .bind(&message_id)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
                Ok(())
            })
        })
    }

    fn unsettled_run_prompts(&self) -> PromptFuture<'_, Vec<RunPrompt>> {
        Box::pin(async move {
            read_txn!(self, tx, async {
                sqlx::query(&format!(
                    "SELECT {COLUMNS} FROM run_prompts WHERE outcome IS NOT NULL \
                     AND message_id IS NOT NULL AND message_settled = 0 \
                     ORDER BY decided_at, run_id, ask"
                ))
                .fetch_all(&mut *tx)
                .await
                .map_err(store_error)?
                .iter()
                .map(decode)
                .collect()
            })
        })
    }

    fn settle_run_prompt_message(&self, run_id: String, ask: u32) -> PromptFuture<'_, ()> {
        Box::pin(async move {
            write_txn!(self, tx, async {
                sqlx::query(
                    "UPDATE run_prompts SET message_settled = 1 WHERE run_id = ?1 AND ask = ?2",
                )
                .bind(&run_id)
                .bind(i64::from(ask))
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
                Ok(())
            })
        })
    }
}
