//! History wire shapes (`history.json`): records exactly as hashed (the
//! canonical body plus `hash`) and rollback plans.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::domain::history::{
    ChangeRecord, RecordError, RevertOutcome, RowChange, RowConflict, RowValue, SkippedRow,
};

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
pub fn row(row: &RowChange) -> Result<Value, RecordError> {
    Ok(json!({
        "key": row.key.to_json(),
        "before": value(row.before.as_ref())?,
        "after": value(row.after.as_ref())?,
    }))
}

fn conflict(conflict: &RowConflict) -> Result<Value, RecordError> {
    Ok(json!({
        "seq": conflict.seq,
        "key": conflict.key.to_json(),
        "expected": value(conflict.expected.as_ref())?,
        "found": value(conflict.found.as_ref())?,
    }))
}

/// One entry per key: a week restore reports a timing skipped by several
/// records once.
fn skipped(rows: &[SkippedRow]) -> Vec<Value> {
    let mut seen = BTreeSet::new();
    rows.iter()
        .filter(|row| seen.insert(row.key.clone()))
        .map(|row| json!({"key": row.key.to_json(), "reason": "outside week"}))
        .collect()
}

/// `RevertPlan` for a rollback outcome; `applied` is the committed rollback
/// record, whose rows are the answer's (the outcome's are a plan).
///
/// # Errors
/// As [`record`].
pub fn plan(outcome: &RevertOutcome, applied: Option<&ChangeRecord>) -> Result<Value, RecordError> {
    let rows = |rows: &[RowChange]| rows.iter().map(row).collect::<Result<Vec<_>, _>>();
    let conflicts = |list: &[RowConflict]| list.iter().map(conflict).collect::<Result<Vec<_>, _>>();
    Ok(match outcome {
        RevertOutcome::Reverted {
            seqs,
            overridden,
            skipped: left,
            rows: changed,
            ..
        } => json!({
            "outcome": if applied.is_some() { "applied" } else { "preview" },
            "reverts": seqs,
            "rows": rows(applied.map_or(changed.as_slice(), |record| record.rows.as_slice()))?,
            "conflicts": conflicts(overridden)?,
            "skipped": skipped(left),
            "record": applied.map(record).transpose()?,
        }),
        RevertOutcome::Unchanged {
            seqs,
            skipped: left,
        } => json!({
            "outcome": "unchanged",
            "reverts": seqs,
            "rows": [],
            "conflicts": [],
            "skipped": skipped(left),
            "record": null,
        }),
        RevertOutcome::Conflicts {
            seqs,
            conflicts: list,
        } => json!({
            "outcome": "conflicts",
            "reverts": seqs,
            "rows": [],
            "conflicts": conflicts(list)?,
            "skipped": [],
            "record": null,
        }),
    })
}

/// A replayed rollback: what its record shows (the records it undid are its
/// refs, a checkpoint head never being among a plain rollback's).
///
/// # Errors
/// As [`record`].
pub fn replayed(applied: &ChangeRecord) -> Result<Value, RecordError> {
    Ok(json!({
        "outcome": "applied",
        "reverts": applied.refs.iter().map(|reference| reference.seq).collect::<Vec<_>>(),
        "rows": applied.rows.iter().map(row).collect::<Result<Vec<_>, _>>()?,
        "conflicts": [],
        "skipped": [],
        "record": record(applied)?,
    }))
}
