//! Planning reverts like `git revert`: the inverse of recorded changes,
//! applied as a new change. History itself is never rewritten.
//!
//! Runs, weekly timings and RSVPs go back to their recorded `before` values.
//! Reminders are never restored from history: only runs whose slot or status
//! the revert changed have their reminders reconciled, as
//! `ensure_reminders(rebuild = false)` would, keeping every reminder already
//! sent or skipped, every reminder an unresolved delivery attempt holds, and
//! every unsent reminder whose kind and time still match the run's slot.
//! Nothing already sent is sent again, and due unsent reminders survive.
//!
//! Runs cannot be deleted, so reverting a run's creation cancels it; the run
//! keeps its weekly timing's slot for that boss week.

use std::collections::{BTreeMap, BTreeSet};

use std::future::Future;

use chrono::{DateTime, Utc};

use super::origin::Actor;
use super::record::{ChangeRecord, RowChange, RowKey, RowValue};
use crate::domain::ids::IdGenerator;
use crate::domain::notify::{DeliveryJournal, DeliveryTarget};
use crate::domain::schedule::{
    Change, ChangeSet, Draft, Notice, NoticeChange, ReminderPolicy, RunStatus, ScheduleError,
    reminder_specs,
};
use crate::domain::scheduler::StoreError;

/// Where a rollback reads the reminders unresolved delivery attempts hold;
/// read again on every commit attempt.
pub trait HeldReminders {
    fn held_reminders(&self) -> impl Future<Output = Result<BTreeSet<String>, StoreError>> + Send;
}

/// A fixed set, e.g. for tests or a caller that already read the journal.
impl HeldReminders for BTreeSet<String> {
    fn held_reminders(&self) -> impl Future<Output = Result<BTreeSet<String>, StoreError>> + Send {
        let held = self.clone();
        async move { Ok(held) }
    }
}

/// The reminders a delivery journal's unresolved attempts hold.
pub struct JournalHeld<'a, J>(pub &'a J);

impl<J: DeliveryJournal + Sync> HeldReminders for JournalHeld<'_, J> {
    async fn held_reminders(&self) -> Result<BTreeSet<String>, StoreError> {
        let view = self
            .0
            .load_view()
            .await
            .map_err(|error| StoreError::Backend(error.to_string()))?;
        Ok(view
            .targets()
            .iter()
            .filter_map(|target| match target {
                DeliveryTarget::Reminder(id) => Some(id.clone()),
                DeliveryTarget::Digest(_)
                | DeliveryTarget::Card(_)
                | DeliveryTarget::Decline { .. }
                | DeliveryTarget::DebugCard { .. } => None,
            })
            .collect())
    }
}

/// Whether rows changed since the recorded change block the revert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevertMode {
    /// Refuse with a conflict report when any row differs from its recorded
    /// `after` value.
    Strict,
    /// Revert anyway: current values go back to the recorded `before`.
    Force,
}

/// Which rows of the selected records a revert touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevertScope {
    /// Every row of every record.
    Whole,
    /// Only runs that were in this boss week before or after the change, and
    /// their RSVPs; weekly timings and other weeks' runs stay as they are.
    Week(DateTime<Utc>),
}

/// A row that no longer holds the value the change left it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowConflict {
    /// The record whose revert found the conflict.
    pub seq: u64,
    pub key: RowKey,
    /// What the change left.
    pub expected: Option<RowValue>,
    /// What the row holds at that point of the revert.
    pub found: Option<RowValue>,
}

/// A recorded row a week-scoped revert left alone (outside the week).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedRow {
    pub seq: u64,
    pub key: RowKey,
}

/// What a revert did, or why it did nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RevertOutcome {
    Reverted {
        /// Records reverted, newest first.
        seqs: Vec<u64>,
        /// Runs whose creation was reverted, now cancelled.
        cancelled_runs: Vec<String>,
        /// Conflicts overridden by [`RevertMode::Force`].
        overridden: Vec<RowConflict>,
        /// Rows not reverted because they lie outside the week.
        skipped: Vec<SkippedRow>,
        /// One summary notice per affected channel.
        notices: Vec<Notice>,
        /// Every row the rollback changes (reminders included), in key order:
        /// the record's rows once applied. Set by the service, empty from
        /// [`apply_revert`].
        rows: Vec<RowChange>,
        /// The rollback record's seq once committed; `None` for a preview.
        seq: Option<u64>,
    },
    /// The selected records were already undone (or only skipped rows
    /// remained): nothing changed, nothing is recorded or announced.
    Unchanged {
        seqs: Vec<u64>,
        skipped: Vec<SkippedRow>,
    },
    /// [`RevertMode::Strict`] found conflicts; nothing was changed. `seqs`
    /// are every selected record, newest first, not only the conflicting.
    Conflicts {
        seqs: Vec<u64>,
        conflicts: Vec<RowConflict>,
    },
}

fn current(draft: &Draft, key: &RowKey) -> Option<RowValue> {
    match key {
        RowKey::FixedRun(id) => draft.fixed_run(id).cloned().map(RowValue::FixedRun),
        RowKey::Run(id) => draft.run(id).cloned().map(RowValue::Run),
        RowKey::Reminder(id) => draft.reminder(id).cloned().map(RowValue::Reminder),
        RowKey::Rsvp { run_id, user_id } => {
            draft.rsvp(run_id, user_id).cloned().map(RowValue::Rsvp)
        }
    }
}

fn restore(draft: &mut Draft, key: &RowKey, before: Option<RowValue>, cancelled: &mut Vec<String>) {
    match (key, before) {
        (_, Some(RowValue::FixedRun(row))) => draft.put_fixed_run(row),
        (_, Some(RowValue::Run(row))) => draft.put_run(row),
        (_, Some(RowValue::Rsvp(row))) => draft.put_rsvp(row),
        (_, Some(RowValue::Reminder(_))) | (RowKey::Reminder(_), None) => {}
        (RowKey::FixedRun(id), None) => draft.delete_fixed_run(id),
        (RowKey::Run(id), None) => {
            if draft
                .run(id)
                .is_some_and(|run| run.status != RunStatus::Cancelled)
            {
                draft.set_run_status(id, RunStatus::Cancelled);
                cancelled.push(id.clone());
            }
        }
        (RowKey::Rsvp { run_id, user_id }, None) => draft.clear_rsvp(run_id, user_id),
    }
}

/// Whether a row belongs to `week`: a run in it before or after the change,
/// or a row of such a run. Weekly timings never do.
fn in_week(record: &ChangeRecord, key: &RowKey, draft: &Draft, week: DateTime<Utc>) -> bool {
    let run_in_week = |run_id: &str| {
        draft.run(run_id).is_some_and(|run| run.week_start == week)
            || record.rows.iter().any(|row| {
                row.key == RowKey::Run(run_id.to_owned())
                    && [&row.before, &row.after]
                        .into_iter()
                        .flatten()
                        .any(|value| value.run_week() == Some(week))
            })
    };
    match key {
        RowKey::FixedRun(_) => false,
        RowKey::Run(id) => {
            record.rows.iter().any(|row| {
                &row.key == key
                    && [&row.before, &row.after]
                        .into_iter()
                        .flatten()
                        .any(|value| value.run_week() == Some(week))
            }) || draft.run(id).is_some_and(|run| run.week_start == week)
        }
        RowKey::Reminder(id) => draft
            .reminder(id)
            .is_some_and(|row| run_in_week(&row.run_id)),
        RowKey::Rsvp { run_id, .. } => run_in_week(run_id),
    }
}

/// Keep sent/skipped, journal-held, and still-matching unsent reminders;
/// replace the rest with the policy's reminders for the run's slot.
fn reconcile_reminders(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    run_id: &str,
    policy: &ReminderPolicy,
    held: &BTreeSet<String>,
    now: DateTime<Utc>,
) -> Result<(), ScheduleError> {
    let Some(run) = draft.run(run_id).cloned() else {
        return Ok(());
    };
    let specs = reminder_specs(run.datetime.fixed_offset(), run.status, policy)?;
    let wanted: BTreeMap<&str, DateTime<Utc>> = specs
        .iter()
        .map(|spec| (spec.kind.as_str(), spec.fire_at.with_timezone(&Utc)))
        .collect();
    for row in draft.reminders(run_id) {
        let matches = wanted.get(row.kind.as_str()) == Some(&row.fire_at);
        if row.sent_at.is_none() && !held.contains(&row.id) && !matches {
            draft.delete_reminder(&row.id);
        }
    }
    for spec in &specs {
        let fire_at = spec.fire_at.with_timezone(&Utc);
        let sent_at = (fire_at <= now).then_some(now);
        draft.add_reminder(ids, run_id, &spec.kind, fire_at, sent_at);
    }
    Ok(())
}

/// Revert `records` (newest first) on `draft` within `scope`. `held` names
/// reminders an unresolved delivery attempt holds. On
/// [`RevertOutcome::Conflicts`] the draft is left untouched.
///
/// # Errors
/// [`ScheduleError`] from reminder planning.
#[allow(clippy::too_many_arguments)]
pub fn apply_revert(
    draft: &mut Draft,
    ids: &mut impl IdGenerator,
    records: &[ChangeRecord],
    scope: RevertScope,
    mode: RevertMode,
    policy: &ReminderPolicy,
    held: &BTreeSet<String>,
    now: DateTime<Utc>,
) -> Result<RevertOutcome, ScheduleError> {
    let mut trial = draft.clone();
    let mut conflicts = Vec::new();
    let mut cancelled = Vec::new();
    let mut skipped = Vec::new();
    let mut runs = BTreeSet::new();
    let mut affected = BTreeSet::new();
    let mut timings = BTreeSet::new();
    for record in records {
        for row in &record.rows {
            if matches!(row.key, RowKey::Reminder(_)) {
                continue;
            }
            if let RevertScope::Week(week) = scope
                && !in_week(record, &row.key, &trial, week)
            {
                skipped.push(SkippedRow {
                    seq: record.seq,
                    key: row.key.clone(),
                });
                continue;
            }
            match &row.key {
                RowKey::Run(id) => {
                    runs.insert(id.clone());
                    affected.insert(id.clone());
                }
                RowKey::Rsvp { run_id, .. } => {
                    affected.insert(run_id.clone());
                }
                RowKey::FixedRun(id) => {
                    timings.insert(id.clone());
                }
                RowKey::Reminder(_) => {}
            }
            let found = current(&trial, &row.key);
            if found != row.after {
                conflicts.push(RowConflict {
                    seq: record.seq,
                    key: row.key.clone(),
                    expected: row.after.clone(),
                    found,
                });
            }
            restore(&mut trial, &row.key, row.before.clone(), &mut cancelled);
        }
    }
    if mode == RevertMode::Strict && !conflicts.is_empty() {
        return Ok(RevertOutcome::Conflicts {
            seqs: records.iter().map(|record| record.seq).collect(),
            conflicts,
        });
    }
    for run_id in &runs {
        let moved = match (draft.run(run_id), trial.run(run_id)) {
            (Some(was), Some(now)) => was.datetime != now.datetime || was.status != now.status,
            (None, Some(_)) => true,
            _ => false,
        };
        if moved {
            reconcile_reminders(&mut trial, ids, run_id, policy, held, now)?;
        }
    }
    let seqs: Vec<u64> = records.iter().map(|record| record.seq).collect();
    let notices = summary_notices(&trial, draft, &seqs, &cancelled, &affected, &timings);
    *draft = trial;
    Ok(RevertOutcome::Reverted {
        seqs,
        cancelled_runs: cancelled,
        overridden: conflicts,
        skipped,
        notices,
        rows: Vec::new(),
        seq: None,
    })
}

/// The rows `changes` alter going from `before` to `after`, in key order, as
/// the store records them.
pub fn changed_rows(before: &Draft, after: &Draft, changes: &ChangeSet) -> Vec<RowChange> {
    let keys: BTreeSet<RowKey> = changes
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
        .collect();
    keys.into_iter()
        .filter_map(|key| {
            let (old, new) = (current(before, &key), current(after, &key));
            (old != new).then_some(RowChange {
                key,
                before: old,
                after: new,
            })
        })
        .collect()
}

/// One rollback notice per home channel of the affected runs (listing
/// them), and one for runs or weekly timings without a home channel, which
/// the notice planner sends to the post channel.
fn summary_notices(
    after: &Draft,
    before: &Draft,
    seqs: &[u64],
    cancelled: &[String],
    runs: &BTreeSet<String>,
    timings: &BTreeSet<String>,
) -> Vec<Notice> {
    let mut by_channel: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
    for run_id in runs {
        let channel = after
            .run(run_id)
            .or_else(|| before.run(run_id))
            .and_then(|run| run.channel_id.clone());
        by_channel.entry(channel).or_default().push(run_id.clone());
    }
    for fixed_id in timings {
        let channel = after
            .fixed_run(fixed_id)
            .or_else(|| before.fixed_run(fixed_id))
            .and_then(|row| row.channel_id.clone());
        by_channel.entry(channel).or_default();
    }
    if by_channel.is_empty() {
        by_channel.insert(None, Vec::new());
    }
    by_channel
        .into_iter()
        .map(|(channel_id, run_ids)| Notice {
            change: NoticeChange::Rollback {
                reverted: seqs.to_vec(),
                cancelled_runs: cancelled
                    .iter()
                    .filter(|id| run_ids.contains(id))
                    .cloned()
                    .collect(),
                run_ids,
                checkpoint: None,
            },
            channel_id,
            listed: Vec::new(),
            via_portal: true,
        })
        .collect()
}

fn newest_first(mut records: Vec<ChangeRecord>) -> Vec<ChangeRecord> {
    records.sort_by_key(|record| std::cmp::Reverse(record.seq));
    records
}

/// Every change after store revision `revision` touching `week`, newest
/// first: reverting them with [`RevertScope::Week`] restores the week to that
/// point.
pub fn changes_for_week(
    records: &[ChangeRecord],
    week: DateTime<Utc>,
    revision: u64,
) -> Vec<ChangeRecord> {
    newest_first(
        records
            .iter()
            .filter(|record| {
                record.seq > 0 && record.revision > revision && record.touches_week(week)
            })
            .cloned()
            .collect(),
    )
}

/// Every change by `actor` at or after `since`, newest first.
pub fn changes_by_actor(
    records: &[ChangeRecord],
    actor: &Actor,
    since: DateTime<Utc>,
) -> Vec<ChangeRecord> {
    newest_first(
        records
            .iter()
            .filter(|record| record.seq > 0 && &record.origin.actor == actor && record.at >= since)
            .cloned()
            .collect(),
    )
}
