//! History wire shapes (`history.json`): records exactly as hashed (the
//! canonical body plus `hash`), pages, rollback plans and checkpoints.
//! Records and row keys stay JSON values: their encoding is the domain's
//! canonical one, which the hash covers.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::domain::history::{
    ChangeRecord, ChangeRef, RecordError, RevertOutcome, RowChange as Change,
    RowConflict as Conflict, RowValue, SkippedRow,
};

/// `{seq, hash}`: a record in the chain.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ChainHead {
    pub seq: u64,
    pub hash: String,
}

impl From<&ChangeRef> for ChainHead {
    fn from(head: &ChangeRef) -> Self {
        Self {
            seq: head.seq,
            hash: head.hash.clone(),
        }
    }
}

/// `GET /api/admin/history?week&actor&run&before&limit`, newest first. With
/// `run=<id>` (alone; not with `week` or `actor`): the run's change log, each
/// record changing its row or RSVPs; the run's before → after is in those
/// `rows` (`runs` keyed by `id`, `rsvps` by `run_id`).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct HistoryPage {
    #[cfg_attr(test, ts(type = "ChangeRecord[]"))]
    pub records: Vec<Value>,
    pub head: ChainHead,
    /// Pass as `before` for the next (older) page; null on the last page.
    pub next_before: Option<u64>,
    /// Matching records (journal only; `settings_total` counts Config saves).
    pub total: u64,
    /// Config section saves in this page's time window, newest first: at or
    /// after the page's oldest record (no lower bound on the last page) and
    /// before the oldest record of the page `before` came from (no upper
    /// bound on the first page), so each save appears on exactly one page.
    /// Always empty with `run`.
    pub settings: Vec<SettingsChangeRow>,
    /// Config saves matching `week`/`actor` across all pages.
    pub settings_total: u64,
}

/// `{kind, id}`, as a record names its actor.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SettingsActor {
    #[cfg_attr(test, ts(type = "ActorKind"))]
    pub kind: &'static str,
    pub id: String,
}

/// One stored settings row's text before and after the save.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SettingRowDiff {
    pub key: String,
    pub from: String,
    pub to: String,
}

/// A saved Config section: view-only, outside the hash chain, never
/// revertible. Settings rows never hold a secret.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SettingsChangeRow {
    pub id: u64,
    pub at: String,
    pub actor: SettingsActor,
    #[cfg_attr(test, ts(type = "Surface"))]
    pub surface: &'static str,
    /// The saved section; the PWA links `/config?section=<section>`.
    pub section: String,
    /// The settings revision the save published (counted per process run).
    pub revision: u64,
    /// The boss week containing `at`, named as records name weeks.
    pub week: String,
    /// Changed rows, by key.
    pub values: Vec<SettingRowDiff>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RowChange {
    #[cfg_attr(test, ts(type = "RowKey"))]
    pub key: Value,
    /// Full domain row; null = absent.
    #[cfg_attr(test, ts(type = "Record<string, unknown> | null"))]
    pub before: Value,
    #[cfg_attr(test, ts(type = "Record<string, unknown> | null"))]
    pub after: Value,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RowConflict {
    pub seq: u64,
    #[cfg_attr(test, ts(type = "RowKey"))]
    pub key: Value,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub expected: Value,
    #[cfg_attr(test, ts(type = "unknown"))]
    pub found: Value,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SkippedKey {
    #[cfg_attr(test, ts(type = "RowKey"))]
    pub key: Value,
    pub reason: &'static str,
}

#[derive(Clone, Copy, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PlanOutcome {
    Preview,
    Applied,
    Unchanged,
    Conflicts,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RevertPlan {
    pub outcome: PlanOutcome,
    /// The requested records (a refused week or actor rollback: the conflicting ones).
    pub reverts: Vec<u64>,
    /// Empty when `outcome` is `conflicts`: a strict refusal plans nothing.
    pub rows: Vec<RowChange>,
    pub conflicts: Vec<RowConflict>,
    pub skipped: Vec<SkippedKey>,
    #[cfg_attr(test, ts(type = "ChangeRecord | null"))]
    pub record: Option<Value>,
}

/// `matches`: the chain holds the backup's head; `older_schema`: it does,
/// but the backup predates the store's schema; `mismatch`: the head is not
/// in the chain (truncated or forked history).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum BackupAnchor {
    Matches,
    OlderSchema,
    Mismatch,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct BackupRow {
    pub file: String,
    #[cfg_attr(test, ts(type = "'kanade.backup.v1'"))]
    pub format: &'static str,
    pub created_at: String,
    pub history_head: ChainHead,
    pub revision: u64,
    pub schema_version: i64,
    /// The chain still contains `history_head`.
    pub anchored: bool,
    pub anchor: BackupAnchor,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Verified {
    pub ok: bool,
    pub checked: u64,
    pub head: ChainHead,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Checkpoints {
    pub verified: Verified,
    /// `KANADE_BACKUP_DIR` is set: false means no directory, not no backups.
    pub backup_dir_configured: bool,
    /// Newest first (at most 100); re-read and re-checked on every request.
    pub backups: Vec<BackupRow>,
}

/// `ChangeRecord`: the canonical body the hash covers, plus the hash.
///
/// # Errors
/// [`RecordError`] for an instant outside the representable years.
pub fn record(record: &ChangeRecord) -> Result<Value, RecordError> {
    let mut body = record.body()?;
    if let Value::Object(map) = &mut body {
        map.insert("hash".into(), record.hash.clone().into());
    }
    Ok(body)
}

fn value(value: Option<&RowValue>) -> Result<Value, RecordError> {
    value.map_or(Ok(Value::Null), RowValue::to_json)
}

/// `RowChange`.
///
/// # Errors
/// As [`record`].
pub fn row(row: &Change) -> Result<RowChange, RecordError> {
    Ok(RowChange {
        key: row.key.to_json(),
        before: value(row.before.as_ref())?,
        after: value(row.after.as_ref())?,
    })
}

fn conflict(conflict: &Conflict) -> Result<RowConflict, RecordError> {
    Ok(RowConflict {
        seq: conflict.seq,
        key: conflict.key.to_json(),
        expected: value(conflict.expected.as_ref())?,
        found: value(conflict.found.as_ref())?,
    })
}

/// One entry per key: a week restore reports a timing skipped by several
/// records once.
fn skipped(rows: &[SkippedRow]) -> Vec<SkippedKey> {
    let mut seen = BTreeSet::new();
    rows.iter()
        .filter(|row| seen.insert(row.key.clone()))
        .map(|row| SkippedKey {
            key: row.key.to_json(),
            reason: "outside week",
        })
        .collect()
}

/// `RevertPlan` for a rollback outcome; `applied` is the committed rollback
/// record, whose rows are the answer's (the outcome's are a plan).
///
/// # Errors
/// As [`record`].
pub fn plan(
    outcome: &RevertOutcome,
    applied: Option<&ChangeRecord>,
) -> Result<RevertPlan, RecordError> {
    let rows = |rows: &[Change]| rows.iter().map(row).collect::<Result<Vec<_>, _>>();
    let conflicts = |list: &[Conflict]| list.iter().map(conflict).collect::<Result<Vec<_>, _>>();
    Ok(match outcome {
        RevertOutcome::Reverted {
            seqs,
            overridden,
            skipped: left,
            rows: changed,
            ..
        } => RevertPlan {
            outcome: if applied.is_some() {
                PlanOutcome::Applied
            } else {
                PlanOutcome::Preview
            },
            reverts: seqs.clone(),
            rows: rows(applied.map_or(changed.as_slice(), |record| record.rows.as_slice()))?,
            conflicts: conflicts(overridden)?,
            skipped: skipped(left),
            record: applied.map(record).transpose()?,
        },
        RevertOutcome::Unchanged {
            seqs,
            skipped: left,
        } => RevertPlan {
            outcome: PlanOutcome::Unchanged,
            reverts: seqs.clone(),
            rows: Vec::new(),
            conflicts: Vec::new(),
            skipped: skipped(left),
            record: None,
        },
        RevertOutcome::Conflicts {
            seqs,
            conflicts: list,
        } => RevertPlan {
            outcome: PlanOutcome::Conflicts,
            reverts: seqs.clone(),
            rows: Vec::new(),
            conflicts: conflicts(list)?,
            skipped: Vec::new(),
            record: None,
        },
    })
}

/// A replayed rollback: what its record shows (the records it undid are its
/// refs, a checkpoint head never being among a plain rollback's).
///
/// # Errors
/// As [`record`].
pub fn replayed(applied: &ChangeRecord) -> Result<RevertPlan, RecordError> {
    Ok(RevertPlan {
        outcome: PlanOutcome::Applied,
        reverts: applied.refs.iter().map(|reference| reference.seq).collect(),
        rows: applied
            .rows
            .iter()
            .map(row)
            .collect::<Result<Vec<_>, _>>()?,
        conflicts: Vec::new(),
        skipped: Vec::new(),
        record: Some(record(applied)?),
    })
}
