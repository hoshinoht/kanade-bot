//! What every store records per commit: the touched keys, their before/after
//! values, and the boss weeks involved.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::domain::history::{RowChange, RowKey, RowValue};
use crate::domain::schedule::{Change, ChangeSet};

/// Every key a change set touches.
pub(crate) fn touched_keys(changes: &ChangeSet) -> BTreeSet<RowKey> {
    changes
        .changes
        .iter()
        .map(|change| match change {
            Change::PutFixedRun(row) => RowKey::FixedRun(row.id.clone()),
            Change::DeleteFixedRun(id) => RowKey::FixedRun(id.clone()),
            Change::PutRun(row) => RowKey::Run(row.id.clone()),
            Change::PutReminder(row) => RowKey::Reminder(row.id.clone()),
            Change::DeleteReminder(id) => RowKey::Reminder(id.clone()),
            Change::PutRsvp(row) => RowKey::Rsvp {
                run_id: row.run_id.clone(),
                user_id: row.user_id.clone(),
            },
            Change::DeleteRsvp { run_id, user_id } => RowKey::Rsvp {
                run_id: run_id.clone(),
                user_id: user_id.clone(),
            },
        })
        .collect()
}

/// Rows whose value actually changed, in key order.
pub(crate) fn changed_rows(
    keys: &BTreeSet<RowKey>,
    before: impl Fn(&RowKey) -> Option<RowValue>,
    after: impl Fn(&RowKey) -> Option<RowValue>,
) -> Vec<RowChange> {
    keys.iter()
        .filter_map(|key| {
            let (old, new) = (before(key), after(key));
            (old != new).then(|| RowChange {
                key: key.clone(),
                before: old,
                after: new,
            })
        })
        .collect()
}

/// Boss weeks of the touched runs and of the runs owning touched reminders
/// and RSVPs; `run_week` looks a run up (after, else before, the commit).
pub(crate) fn touched_weeks(
    rows: &[RowChange],
    run_week: impl Fn(&str) -> Option<DateTime<Utc>>,
) -> Vec<DateTime<Utc>> {
    let mut weeks = BTreeSet::new();
    for row in rows {
        for value in [&row.before, &row.after].into_iter().flatten() {
            if let Some(week) = value.run_week() {
                weeks.insert(week);
            } else if let Some(week) = value.owning_run().and_then(&run_week) {
                weeks.insert(week);
            }
        }
    }
    weeks.into_iter().collect()
}
