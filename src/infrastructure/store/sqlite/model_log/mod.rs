//! `ModelLogStore` over migration 0007: the message cache, extraction and
//! chat logs with their filter queries, rescan jobs, allowance overrides and
//! self-service tips. Every statement is constant SQL with bound
//! parameters; writes take one `BEGIN IMMEDIATE` each.

mod chat;
mod extractions;
mod jobs;
mod messages;
#[cfg(test)]
mod plans;
mod prune;
mod refresh;

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::{Connection, Row};

use super::SqliteStore;
use super::rows;
use super::schedule::store_error;
use crate::domain::model_log::{
    AllowanceOverride, ChatFilter, ChatInteraction, ExtractionFilter, ExtractionLog, LogFacets,
    LogPage, MessageUpsert, ModelLogStore, PRUNE_BATCH, PruneCounts, ReadMessage, RescanJob,
    WatchedMessage,
};
use crate::domain::scheduler::StoreError;

fn corrupt(table: &str, detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("{table} row is unreadable: {detail}"))
}

fn text(row: &SqliteRow, column: &str) -> Result<String, StoreError> {
    row.try_get(column).map_err(|error| corrupt(column, error))
}

fn optional_text(row: &SqliteRow, column: &str) -> Result<Option<String>, StoreError> {
    row.try_get(column).map_err(|error| corrupt(column, error))
}

fn read_instant(row: &SqliteRow, column: &str) -> Result<DateTime<Utc>, StoreError> {
    crate::domain::time::from_iso(&text(row, column)?).map_err(|error| corrupt(column, error))
}

fn read_optional_instant(
    row: &SqliteRow,
    column: &str,
) -> Result<Option<DateTime<Utc>>, StoreError> {
    optional_text(row, column)?
        .map(|value| crate::domain::time::from_iso(&value).map_err(|error| corrupt(column, error)))
        .transpose()
}

fn read_u64(row: &SqliteRow, column: &str) -> Result<u64, StoreError> {
    let value: i64 = row
        .try_get(column)
        .map_err(|error| corrupt(column, error))?;
    u64::try_from(value).map_err(|error| corrupt(column, error))
}

fn read_optional_u64(row: &SqliteRow, column: &str) -> Result<Option<u64>, StoreError> {
    let value: Option<i64> = row
        .try_get(column)
        .map_err(|error| corrupt(column, error))?;
    value
        .map(|value| u64::try_from(value).map_err(|error| corrupt(column, error)))
        .transpose()
}

fn read_json(row: &SqliteRow, column: &str) -> Result<Value, StoreError> {
    serde_json::from_str(&text(row, column)?).map_err(|error| corrupt(column, error))
}

fn read_list(row: &SqliteRow, column: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(&text(row, column)?).map_err(|error| corrupt(column, error))
}

fn signed(value: u64, what: &str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Constraint(format!("{what} is too large")))
}

fn optional_signed(value: Option<u64>, what: &str) -> Result<Option<i64>, StoreError> {
    value.map(|value| signed(value, what)).transpose()
}

fn json_text(value: &Value) -> String {
    // Serialising a `Value` cannot fail.
    serde_json::to_string(value).unwrap_or_default()
}

fn instant(at: &DateTime<Utc>) -> Result<String, StoreError> {
    rows::instant(at)
}

impl ModelLogStore for SqliteStore {
    async fn upsert_message(&self, message: WatchedMessage) -> Result<MessageUpsert, StoreError> {
        write_txn!(self, tx, messages::upsert(&mut tx, &message))
    }

    async fn mark_processed(&self, ids: &[String], at: DateTime<Utc>) -> Result<u64, StoreError> {
        write_txn!(self, tx, messages::mark_processed(&mut tx, ids, &at))
    }

    async fn mark_read(&self, read: &[ReadMessage], at: DateTime<Utc>) -> Result<u64, StoreError> {
        write_txn!(self, tx, messages::mark_read(&mut tx, read, &at))
    }

    async fn delete_message(&self, id: &str) -> Result<bool, StoreError> {
        write_txn!(self, tx, messages::delete(&mut tx, id))
    }

    async fn channel_messages(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
        unprocessed_only: bool,
    ) -> Result<Vec<WatchedMessage>, StoreError> {
        read_txn!(
            self,
            tx,
            messages::in_channel(&mut tx, channel_id, &since, unprocessed_only)
        )
    }

    async fn messages_by_ids(&self, ids: &[String]) -> Result<Vec<WatchedMessage>, StoreError> {
        read_txn!(self, tx, messages::by_ids(&mut tx, ids))
    }

    async fn record_extraction(&self, log: ExtractionLog) -> Result<(), StoreError> {
        log.check_shape()?;
        write_txn!(self, tx, extractions::insert(&mut tx, &log))
    }

    async fn load_extraction(&self, id: &str) -> Result<Option<ExtractionLog>, StoreError> {
        read_txn!(self, tx, extractions::load(&mut tx, id))
    }

    async fn list_extractions(
        &self,
        filter: &ExtractionFilter,
    ) -> Result<LogPage<ExtractionLog>, StoreError> {
        read_txn!(self, tx, extractions::list(&mut tx, filter))
    }

    async fn extraction_facets(&self) -> Result<LogFacets, StoreError> {
        read_txn!(self, tx, extractions::facets(&mut tx))
    }

    async fn record_chat(&self, interaction: ChatInteraction) -> Result<(), StoreError> {
        interaction.check_shape()?;
        write_txn!(self, tx, chat::insert(&mut tx, &interaction))
    }

    async fn load_chat(&self, id: &str) -> Result<Option<ChatInteraction>, StoreError> {
        read_txn!(self, tx, chat::load(&mut tx, id))
    }

    async fn list_chats(
        &self,
        filter: &ChatFilter,
    ) -> Result<LogPage<ChatInteraction>, StoreError> {
        read_txn!(self, tx, chat::list(&mut tx, filter))
    }

    async fn chat_facets(&self) -> Result<LogFacets, StoreError> {
        read_txn!(self, tx, chat::facets(&mut tx))
    }

    async fn prune_model_logs(&self, before: DateTime<Utc>) -> Result<PruneCounts, StoreError> {
        let mut total = PruneCounts::default();
        loop {
            let done: PruneCounts =
                write_txn!(self, tx, prune::batch(&mut tx, &before, PRUNE_BATCH))?;
            total.extractions += done.extractions;
            total.chats += done.chats;
            total.messages += done.messages;
            total.notices += done.notices;
            let full = u64::from(PRUNE_BATCH);
            if done.extractions < full
                && done.chats < full
                && done.messages < full
                && done.notices < full
            {
                return Ok(total);
            }
        }
    }

    async fn insert_rescan_job(&self, job: RescanJob) -> Result<(), StoreError> {
        job.check_shape()?;
        write_txn!(self, tx, jobs::insert_rescan(&mut tx, &job))
    }

    async fn update_rescan_job(&self, job: RescanJob) -> Result<bool, StoreError> {
        job.check_shape()?;
        write_txn!(self, tx, jobs::update_rescan(&mut tx, &job))
    }

    async fn load_rescan_job(&self, id: &str) -> Result<Option<RescanJob>, StoreError> {
        read_txn!(self, tx, jobs::load_rescan(&mut tx, id))
    }

    async fn recent_rescan_jobs(&self, limit: u32) -> Result<Vec<RescanJob>, StoreError> {
        read_txn!(self, tx, jobs::recent_rescans(&mut tx, limit))
    }

    async fn set_allowance_override(&self, entry: AllowanceOverride) -> Result<(), StoreError> {
        entry.check_shape()?;
        write_txn!(self, tx, jobs::set_allowance(&mut tx, &entry))
    }

    async fn clear_allowance_override(&self, member_id: &str) -> Result<bool, StoreError> {
        write_txn!(self, tx, jobs::clear_allowance(&mut tx, member_id))
    }

    async fn allowance_overrides(&self) -> Result<Vec<AllowanceOverride>, StoreError> {
        read_txn!(self, tx, jobs::allowances(&mut tx))
    }

    async fn claim_tip(
        &self,
        member_id: &str,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        write_txn!(self, tx, jobs::claim_tip(&mut tx, member_id, &week, &at))
    }

    async fn release_tip(&self, member_id: &str, week: DateTime<Utc>) -> Result<bool, StoreError> {
        write_txn!(self, tx, jobs::release_tip(&mut tx, member_id, &week))
    }
}
