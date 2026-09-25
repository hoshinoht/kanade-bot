//! History wire shapes (`history.json`): records exactly as hashed (the
//! canonical body plus `hash`), rollback plans and blame lines.

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::domain::{
    history::{
        Blame, ChangeRecord, RecordError, RevertOutcome, RowChange, RowConflict, RowValue,
        SkippedRow,
    },
    schedule::ScheduleSnapshot,
    time::to_iso,
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

/// `RevertPlan` for a rollback outcome. `selected` names the records a
/// strict refusal was about (the outcome itself lists only conflicts);
/// `applied` is the committed rollback record.
///
/// # Errors
/// As [`record`].
pub fn plan(
    outcome: &RevertOutcome,
    selected: &[u64],
    applied: Option<&ChangeRecord>,
) -> Result<Value, RecordError> {
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
            "rows": rows(changed)?,
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
        RevertOutcome::Conflicts(list) => {
            let mut reverts: Vec<u64> = if selected.is_empty() {
                list.iter().map(|conflict| conflict.seq).collect()
            } else {
                selected.to_vec()
            };
            reverts.sort_unstable_by(|a, b| b.cmp(a));
            reverts.dedup();
            json!({
                "outcome": "conflicts",
                "reverts": reverts,
                "rows": [],
                "conflicts": conflicts(list)?,
                "skipped": [],
                "record": null,
            })
        }
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

/// `BlameEntry[]` for a run: each field some record set, with its current
/// value from `snapshot` (fields that predate the history are left out).
///
/// # Errors
/// As [`record`].
pub fn blame(
    blame: &Blame,
    run_id: &str,
    snapshot: &ScheduleSnapshot,
) -> Result<Vec<Value>, RecordError> {
    let run = snapshot.runs.iter().find(|run| run.id == run_id);
    let run_json = run
        .map(|run| RowValue::Run(run.clone()).to_json())
        .transpose()?
        .unwrap_or(Value::Null);
    let pick = |keys: &[&str]| -> Value {
        let mut out = Map::new();
        for key in keys {
            out.insert(
                (*key).to_owned(),
                run_json.get(*key).cloned().unwrap_or(Value::Null),
            );
        }
        Value::Object(out)
    };
    let field_value = |field: &str| -> Result<Value, RecordError> {
        if let Some(user) = field.strip_prefix("rsvp:") {
            let rsvp = snapshot
                .rsvps
                .iter()
                .find(|rsvp| rsvp.run_id == run_id && rsvp.user_id == user);
            return value(rsvp.map(|rsvp| RowValue::Rsvp(rsvp.clone())).as_ref());
        }
        if let Some(user) = field.strip_prefix("attended:") {
            return Ok(run_json
                .get("attendance")
                .and_then(Value::as_array)
                .and_then(|entries| {
                    entries
                        .iter()
                        .find(|entry| entry["user_id"] == user)
                        .cloned()
                })
                .unwrap_or(Value::Null));
        }
        Ok(match field {
            "slot" => pick(&["datetime", "week_start", "source", "fixed_run_id"]),
            "channel" => run_json.get("channel_id").cloned().unwrap_or(Value::Null),
            other => run_json.get(other).cloned().unwrap_or(Value::Null),
        })
    };
    let mut entries = Vec::new();
    for line in &blame.lines {
        let Some(last) = &line.last else {
            continue;
        };
        entries.push(json!({
            "field": line.field,
            "value": field_value(&line.field)?,
            "seq": last.seq,
            "at": to_iso(&last.at).map_err(|error| RecordError(error.to_string()))?,
            "actor": {"kind": last.actor.kind(), "id": last.actor.id()},
            "surface": last.surface.as_str(),
        }));
    }
    Ok(entries)
}
