//! Typed scheduler snapshots and the row-level change sets planners emit.

use std::collections::BTreeSet;

use super::run::{FixedRun, Reminder, Rsvp, Run};

/// The scheduler rows a planner reads: every weekly timing plus the runs in
/// the loaded scope with their reminders and RSVPs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScheduleSnapshot {
    /// The store revision this snapshot was read at; a commit planned from it
    /// must present it back.
    pub revision: u64,
    pub fixed_runs: Vec<FixedRun>,
    pub runs: Vec<Run>,
    pub reminders: Vec<Reminder>,
    pub rsvps: Vec<Rsvp>,
    /// Loaded reminders whose send was retired without proof of delivery (a
    /// runtime attempt retired with no message id); they are never reopened.
    pub unproven_retired: BTreeSet<String>,
}

impl ScheduleSnapshot {
    /// Put rows in the order every store returns: weekly timings by
    /// `(weekday, time, id)`, runs by `(datetime, id)`, reminders by
    /// `(run_id, fire_at, kind)`, RSVPs by `(run_id, user_id)`.
    pub fn sort(&mut self) {
        self.fixed_runs.sort_by(|a, b| {
            (a.weekday.num_days_from_monday(), a.time, &a.id).cmp(&(
                b.weekday.num_days_from_monday(),
                b.time,
                &b.id,
            ))
        });
        self.runs
            .sort_by(|a, b| (a.datetime, &a.id).cmp(&(b.datetime, &b.id)));
        self.reminders
            .sort_by(|a, b| (&a.run_id, a.fire_at, &a.kind).cmp(&(&b.run_id, b.fire_at, &b.kind)));
        self.rsvps
            .sort_by(|a, b| (&a.run_id, &a.user_id).cmp(&(&b.run_id, &b.user_id)));
    }
}

/// One row write. `Put*` inserts or replaces by primary key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    PutFixedRun(FixedRun),
    DeleteFixedRun(String),
    PutRun(Run),
    PutReminder(Reminder),
    DeleteReminder(String),
    PutRsvp(Rsvp),
    DeleteRsvp { run_id: String, user_id: String },
}

/// The writes of one planned operation, committed atomically.
///
/// Deletes precede puts so a rebuilt `(run_id, kind)` never collides with the
/// row it replaces. Stores validate uniqueness on the resulting state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    pub changes: Vec<Change>,
}

impl ChangeSet {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}
