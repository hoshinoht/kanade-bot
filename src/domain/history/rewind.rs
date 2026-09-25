//! Rewinding a snapshot to an earlier point of the history.

use std::collections::BTreeMap;

use super::record::{ChangeRecord, ChangeRef, RowKey, RowValue};
use crate::domain::schedule::ScheduleSnapshot;

/// Why a record list cannot rewind a snapshot to its base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HistoryGap {
    /// Record `expected` is missing: another record (`found`) or nothing
    /// came in its place before the head.
    Missing { expected: u64, found: Option<u64> },
    /// Record `seq` does not link to the one before it (or to the base).
    BrokenLink { seq: u64 },
    /// Record `seq`'s stored hash is not the hash of its content.
    Tampered { seq: u64 },
    /// The head is not in the chain from the base.
    HeadMismatch { head: ChangeRef },
}

impl std::fmt::Display for HistoryGap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { expected, .. } => write!(f, "change #{expected} is missing"),
            Self::BrokenLink { seq } => write!(f, "change #{seq} does not follow its predecessor"),
            Self::Tampered { seq } => write!(f, "change #{seq} does not match its hash"),
            Self::HeadMismatch { head } => write!(f, "change #{} is not on this history", head.seq),
        }
    }
}

impl std::error::Error for HistoryGap {}

/// Check that `records` are exactly the chain after `base` up to at least
/// `head`: consecutive seqs from `base.seq + 1`, each linking to the one
/// before it (the first to `base.hash`), each hash matching its content, and
/// `head` among them (or equal to `base`). Records after `head` are allowed.
///
/// # Errors
/// The first [`HistoryGap`].
pub fn check_records_after(
    base: &ChangeRef,
    head: &ChangeRef,
    records: &[ChangeRecord],
) -> Result<(), HistoryGap> {
    let mut ordered: Vec<&ChangeRecord> = records.iter().collect();
    ordered.sort_by_key(|record| record.seq);
    let (mut prev_seq, mut prev_hash) = (base.seq, base.hash.as_str());
    let mut reached = head == base;
    for record in ordered {
        let expected = prev_seq + 1;
        if record.seq != expected {
            return Err(HistoryGap::Missing {
                expected,
                found: Some(record.seq),
            });
        }
        if record.prev_hash != prev_hash {
            return Err(HistoryGap::BrokenLink { seq: record.seq });
        }
        if record.computed_hash().ok().as_deref() != Some(record.hash.as_str()) {
            return Err(HistoryGap::Tampered { seq: record.seq });
        }
        if record.seq == head.seq {
            if record.hash != head.hash {
                return Err(HistoryGap::HeadMismatch { head: head.clone() });
            }
            reached = true;
        }
        (prev_seq, prev_hash) = (record.seq, record.hash.as_str());
    }
    if reached {
        Ok(())
    } else if head.seq > prev_seq {
        Err(HistoryGap::Missing {
            expected: prev_seq + 1,
            found: None,
        })
    } else {
        Err(HistoryGap::HeadMismatch { head: head.clone() })
    }
}

/// The schedule as it was at `base`, from `current` and the records after
/// `base` (in any order): every row they touched goes back to the `before`
/// value of the earliest record that touched it; other rows are kept.
///
/// `current` must be a whole-schedule snapshot and `head` a history head
/// read after `current` was loaded, so every record `current` reflects is
/// included. Records committed after the load are harmless: a row they alone
/// touched goes back to its value at the load, which `current` already holds.
///
/// Journal writes (a reminder stamped sent) are not records, so reminder
/// rows can differ from the true past; the revision is kept from `current`.
///
/// # Errors
/// [`HistoryGap`] (see [`check_records_after`]); nothing is rewound on a gap.
pub fn rewind(
    current: &ScheduleSnapshot,
    base: &ChangeRef,
    head: &ChangeRef,
    records: &[ChangeRecord],
) -> Result<ScheduleSnapshot, HistoryGap> {
    check_records_after(base, head, records)?;
    let mut ordered: Vec<&ChangeRecord> = records.iter().collect();
    ordered.sort_by_key(|record| record.seq);
    let mut base: BTreeMap<RowKey, Option<RowValue>> = BTreeMap::new();
    for record in ordered {
        for row in &record.rows {
            base.entry(row.key.clone())
                .or_insert_with(|| row.before.clone());
        }
    }
    let mut fixed: BTreeMap<String, _> = current
        .fixed_runs
        .iter()
        .map(|row| (row.id.clone(), row.clone()))
        .collect();
    let mut runs: BTreeMap<String, _> = current
        .runs
        .iter()
        .map(|row| (row.id.clone(), row.clone()))
        .collect();
    let mut reminders: BTreeMap<String, _> = current
        .reminders
        .iter()
        .map(|row| (row.id.clone(), row.clone()))
        .collect();
    let mut rsvps: BTreeMap<(String, String), _> = current
        .rsvps
        .iter()
        .map(|row| ((row.run_id.clone(), row.user_id.clone()), row.clone()))
        .collect();
    for (key, value) in base {
        match (key, value) {
            (_, Some(RowValue::FixedRun(row))) => {
                fixed.insert(row.id.clone(), row);
            }
            (_, Some(RowValue::Run(row))) => {
                runs.insert(row.id.clone(), row);
            }
            (_, Some(RowValue::Reminder(row))) => {
                reminders.insert(row.id.clone(), row);
            }
            (_, Some(RowValue::Rsvp(row))) => {
                rsvps.insert((row.run_id.clone(), row.user_id.clone()), row);
            }
            (RowKey::FixedRun(id), None) => {
                fixed.remove(&id);
            }
            (RowKey::Run(id), None) => {
                runs.remove(&id);
            }
            (RowKey::Reminder(id), None) => {
                reminders.remove(&id);
            }
            (RowKey::Rsvp { run_id, user_id }, None) => {
                rsvps.remove(&(run_id, user_id));
            }
        }
    }
    let mut snapshot = ScheduleSnapshot {
        revision: current.revision,
        fixed_runs: fixed.into_values().collect(),
        runs: runs.into_values().collect(),
        reminders: reminders.into_values().collect(),
        rsvps: rsvps.into_values().collect(),
        unproven_retired: current.unproven_retired.clone(),
    };
    snapshot.sort();
    Ok(snapshot)
}
