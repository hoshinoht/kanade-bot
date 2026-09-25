//! `kanade import v4`: a one-off testing import from a read-only copy of the
//! v4 SQLite database (user decision 2026-09-25, a narrow exception to the
//! "no raw v4 SQLite import" rule; see `docs/v5/v4-import.md`).
//!
//! Imports the weekly fixed runs (all of them, through the scheduler) and
//! the recent chat and extraction logs with the watched messages they
//! reference. Nothing else: no runs, answers, reminders, history, members
//! or settings. A dry run unless `apply`; a re-run adds nothing twice.

mod fixed;
mod logs;
mod read;
mod report;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;

pub use report::{Counts, Report};

use crate::domain::model_log::{
    DEFAULT_LOG_RETENTION, ModelLogStore, WatchedMessage, retention_cutoff,
};
use crate::domain::settings::{RuntimeSettings, load_settings};
use crate::infrastructure::files::load_catalog;
use crate::infrastructure::store::{SqliteStore, SqliteStoreConfig};
use crate::runtime::config::ImportConfig;
use read::{Head, Snapshot, V4Message};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// The v4 snapshot (a copy; opened read-only).
    pub from: PathBuf,
    /// Narrows the log window to guild-local days from this date.
    pub since: Option<NaiveDate>,
    pub apply: bool,
}

#[derive(Debug)]
pub enum ImportError {
    /// The snapshot is missing, unreadable or not a v4 database.
    Source(String),
    Catalog(String),
    /// The v5 store could not be opened, read or written.
    Store(String),
    Refused(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(message)
            | Self::Catalog(message)
            | Self::Store(message)
            | Self::Refused(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ImportError {}

/// Wall clock without chrono's `clock` feature.
pub fn system_now() -> DateTime<Utc> {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_timestamp(
        i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        since.subsec_nanos(),
    )
    .unwrap_or(DateTime::UNIX_EPOCH)
}

/// Canonical decimal Discord id.
fn snowflake(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('0')
        && text.bytes().all(|byte| byte.is_ascii_digit())
        && text.parse::<u64>().is_ok()
}

/// The retention cutoff at `now`, or local midnight of `since` if later.
fn cutoff(now: DateTime<Utc>, since: Option<NaiveDate>, zone: Tz) -> DateTime<Utc> {
    let retained = retention_cutoff(now, DEFAULT_LOG_RETENTION).unwrap_or(DateTime::<Utc>::MIN_UTC);
    let Some(since) = since else {
        return retained;
    };
    let midnight = since.and_time(NaiveTime::MIN);
    let since = zone
        .from_local_datetime(&midnight)
        .earliest()
        .map_or_else(|| midnight.and_utc(), |at| at.with_timezone(&Utc));
    since.max(retained)
}

/// Ids of log rows inside the window; the rest are counted as skipped.
fn select(heads: Vec<Head>, cutoff: DateTime<Utc>, counts: &mut Counts) -> Vec<String> {
    let mut ids = Vec::new();
    for head in heads {
        let Some(id) = head.id.filter(|id| !id.is_empty()) else {
            counts.skip("bad_row");
            continue;
        };
        match logs::instant(head.at.as_deref()) {
            None => counts.skip("bad_timestamp"),
            Some(at) if at < cutoff => counts.skip("outside_window"),
            Some(_) => ids.push(id),
        }
    }
    ids
}

fn same_file(from: &std::path::Path, db: &std::path::Path) -> bool {
    match (std::fs::canonicalize(from), std::fs::canonicalize(db)) {
        (Ok(from), Ok(db)) => from == db,
        _ => false,
    }
}

/// Read the snapshot, then report (and with `apply`, write) what it adds.
/// `now` is the import's single clock reading.
///
/// # Errors
/// The snapshot, catalog or store failed; nothing is known to be half
/// written except whole rows (each write is atomic).
pub async fn run(
    options: &Options,
    config: &ImportConfig,
    now: DateTime<Utc>,
) -> Result<Report, ImportError> {
    if same_file(&options.from, &config.store.db_path) {
        return Err(ImportError::Refused(
            "--from names the v5 store itself".into(),
        ));
    }
    let catalog = load_catalog(&config.files.catalog_file)
        .map_err(|error| ImportError::Catalog(error.to_string()))?;
    let cutoff = cutoff(now, options.since, config.timezone);
    let mut report = Report {
        applied: options.apply,
        cutoff,
        fixed_runs: Counts::default(),
        fixed_skipped: Vec::new(),
        materialised: None,
        chats: Counts::default(),
        extractions: Counts::default(),
        messages: Counts::default(),
    };

    let mut source = Snapshot::open(&options.from).await?;
    let read = read_source(&mut source, cutoff, &mut report).await;
    source.close().await;
    let (fixed_rows, chats, extractions, messages) = read?;

    let mut candidates = Vec::new();
    for (index, row) in fixed_rows.iter().enumerate() {
        match fixed::validate(row, &catalog) {
            Ok(candidate) => candidates.push(candidate),
            Err(reason) => {
                report.fixed_runs.skip(reason);
                let label = match &row.id {
                    Some(id) if reason != "bad_id" => id.clone(),
                    _ => format!("row {}", index + 1),
                };
                report.fixed_skipped.push((label, reason));
            }
        }
    }
    let by_id: BTreeMap<String, &V4Message> = messages
        .iter()
        .map(|message| (message.id.clone(), message))
        .collect();
    let chats: Vec<_> = chats
        .iter()
        .filter_map(|row| Some(logs::chat(row, logs::instant(row.at.as_deref())?)))
        .collect();
    let extractions: Vec<_> = extractions
        .iter()
        .filter_map(|row| {
            Some(logs::extraction(
                row,
                logs::instant(row.at.as_deref())?,
                &by_id,
            ))
        })
        .collect();
    let mut watched = Vec::new();
    for referenced in referenced(&chats, &extractions) {
        match by_id.get(&referenced) {
            None => report.messages.skip("not_in_snapshot"),
            Some(row) => match logs::message(row, now) {
                Some(message) => watched.push(message),
                None => report.messages.skip("bad_row"),
            },
        }
    }

    // A dry run against a store that does not exist yet must not create it.
    let store = if options.apply || config.store.db_path.exists() {
        let store_config = SqliteStoreConfig {
            db_path: config.store.db_path.clone(),
            owner_lock_dir: config.store.owner_lock_dir.clone(),
        };
        Some(Arc::new(SqliteStore::open(&store_config).await.map_err(
            |error| ImportError::Store(format!("the v5 store could not be opened: {error}")),
        )?))
    } else {
        None
    };
    let written = write(
        store.as_ref(),
        options.apply,
        now,
        config.timezone,
        candidates,
        watched,
        extractions,
        chats,
        &mut report,
    )
    .await;
    // `write` holds no clones past its return, so the unwrap succeeds.
    if let Some(Ok(store)) = store.map(Arc::try_unwrap) {
        store
            .close()
            .await
            .map_err(|error| ImportError::Store(error.to_string()))?;
    }
    written.map(|()| report)
}

type Source = (
    Vec<read::V4Fixed>,
    Vec<read::V4Chat>,
    Vec<read::V4Extraction>,
    Vec<V4Message>,
);

async fn read_source(
    source: &mut Snapshot,
    cutoff: DateTime<Utc>,
    report: &mut Report,
) -> Result<Source, ImportError> {
    let fixed_rows = source.fixed_runs().await?;
    let chat_ids = select(source.chat_heads().await?, cutoff, &mut report.chats);
    let extraction_ids = select(
        source.extraction_heads().await?,
        cutoff,
        &mut report.extractions,
    );
    let chats = source.chats(&chat_ids).await?;
    let extractions = source.extractions(&extraction_ids).await?;
    let mut ids: Vec<String> = chats
        .iter()
        .filter_map(|chat| chat.message_id.clone())
        .chain(
            extractions
                .iter()
                .flat_map(|row| logs::id_list(row.message_ids.as_deref()).unwrap_or_default()),
        )
        .collect();
    ids.sort();
    ids.dedup();
    let messages = source.messages(&ids).await?;
    Ok((fixed_rows, chats, extractions, messages))
}

fn referenced(
    chats: &[crate::domain::model_log::ChatInteraction],
    extractions: &[crate::domain::model_log::ExtractionLog],
) -> Vec<String> {
    let mut ids: Vec<String> = chats
        .iter()
        .filter_map(|chat| chat.message_id.clone())
        .chain(extractions.iter().flat_map(|log| log.message_ids.clone()))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[allow(clippy::too_many_arguments)]
async fn write(
    store: Option<&Arc<SqliteStore>>,
    apply: bool,
    now: DateTime<Utc>,
    zone: Tz,
    candidates: Vec<fixed::Candidate>,
    messages: Vec<WatchedMessage>,
    extractions: Vec<crate::domain::model_log::ExtractionLog>,
    chats: Vec<crate::domain::model_log::ChatInteraction>,
    report: &mut Report,
) -> Result<(), ImportError> {
    let stored =
        |error: crate::domain::scheduler::StoreError| ImportError::Store(error.to_string());
    let policy = match store {
        Some(store) => load_settings(store.as_ref(), &RuntimeSettings::default())
            .await
            .map_err(|error| ImportError::Store(error.to_string()))?,
        None => RuntimeSettings::default(),
    }
    .schedule_policy(zone);
    fixed::import(
        store,
        candidates,
        apply,
        now,
        &policy,
        &mut report.fixed_runs,
        &mut report.fixed_skipped,
    )
    .await?;

    let Some(store) = store else {
        report.messages.added += messages.len() as u64;
        report.extractions.added += extractions.len() as u64;
        report.chats.added += chats.len() as u64;
        return Ok(());
    };
    if apply {
        report.materialised = Some(fixed::materialise(store, now, &policy).await?);
    }

    let ids: Vec<String> = messages.iter().map(|message| message.id.clone()).collect();
    let present: Vec<String> = store
        .messages_by_ids(&ids)
        .await
        .map_err(stored)?
        .into_iter()
        .map(|message| message.id)
        .collect();
    for message in messages {
        if present.contains(&message.id) {
            report.messages.present += 1;
        } else {
            if apply {
                store.upsert_message(message).await.map_err(stored)?;
            }
            report.messages.added += 1;
        }
    }
    for log in extractions {
        if store
            .load_extraction(&log.id)
            .await
            .map_err(stored)?
            .is_some()
        {
            report.extractions.present += 1;
        } else {
            if apply {
                store.record_extraction(log).await.map_err(stored)?;
            }
            report.extractions.added += 1;
        }
    }
    for chat in chats {
        if store.load_chat(&chat.id).await.map_err(stored)?.is_some() {
            report.chats.present += 1;
        } else {
            if apply {
                store.record_chat(chat).await.map_err(stored)?;
            }
            report.chats.added += 1;
        }
    }
    Ok(())
}
