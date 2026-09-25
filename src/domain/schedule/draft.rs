//! A mutable working copy of a snapshot with v4 table semantics.
//!
//! Planners call these methods where v4 called `Repo`; [`Draft::into_changes`]
//! diffs the result against the loaded snapshot. Every insert takes its id from
//! the caller's generator exactly when v4's `new_id()` ran, including inserts
//! the `(run_id, kind)` uniqueness then ignores.

use crate::domain::attendance::{
    AttendanceDefault, AttendancePolicy, AttendanceRecord, StandingAnswer, StatusPin,
};
use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::error::ScheduleError;
use super::run::{
    FixedRun, FixedRunPatch, NewFixedRun, NewRun, Reminder, Rsvp, RsvpSource, RsvpState, Run,
    RunStatus,
};
use super::state::{Change, ChangeSet, ScheduleSnapshot};
use crate::domain::ids::IdGenerator;

type RsvpKey = (String, String);

#[derive(Clone, Debug)]
pub struct Draft {
    base: ScheduleSnapshot,
    fixed: BTreeMap<String, FixedRun>,
    runs: BTreeMap<String, Run>,
    reminders: BTreeMap<String, Reminder>,
    rsvps: BTreeMap<RsvpKey, Rsvp>,
    /// The attendance rules status recounts follow (v4 unless set).
    attendance: AttendancePolicy,
}

impl Draft {
    pub fn new(base: ScheduleSnapshot) -> Self {
        Self {
            fixed: keyed(&base.fixed_runs, |row| row.id.clone()),
            runs: keyed(&base.runs, |row| row.id.clone()),
            reminders: keyed(&base.reminders, |row| row.id.clone()),
            rsvps: keyed(&base.rsvps, rsvp_key),
            base,
            attendance: AttendancePolicy::V4_COMPAT,
        }
    }

    /// Recount statuses under `attendance` (the schedule policy's).
    #[must_use]
    pub fn with_attendance(mut self, attendance: AttendancePolicy) -> Self {
        self.attendance = attendance;
        self
    }

    pub fn attendance(&self) -> AttendancePolicy {
        self.attendance
    }

    pub fn add_fixed_run(&mut self, ids: &mut impl IdGenerator, new: NewFixedRun) -> String {
        let id = ids.new_id();
        let row = FixedRun {
            id: id.clone(),
            owner_id: new.owner_id,
            channel_id: new.channel_id,
            bosses: new.bosses,
            weekday: new.weekday,
            time: new.time,
            participants: new.participants,
            note: new.note,
            attendance_default: AttendanceDefault::OptIn,
            standing: Vec::new(),
        };
        self.fixed.insert(id.clone(), row);
        id
    }

    pub fn fixed_run(&self, id: &str) -> Option<&FixedRun> {
        self.fixed.get(id)
    }

    /// Weekly timings ordered by `(weekday, time, id)`.
    pub fn fixed_runs(&self) -> Vec<FixedRun> {
        let mut rows: Vec<FixedRun> = self.fixed.values().cloned().collect();
        rows.sort_by(|a, b| {
            (a.weekday.num_days_from_monday(), a.time, &a.id).cmp(&(
                b.weekday.num_days_from_monday(),
                b.time,
                &b.id,
            ))
        });
        rows
    }

    /// v4 `update_fixed_run`: unset fields are left alone; a missing id is a no-op.
    pub fn update_fixed_run(&mut self, id: &str, patch: FixedRunPatch) {
        let Some(row) = self.fixed.get_mut(id) else {
            return;
        };
        if let Some(value) = patch.owner_id {
            row.owner_id = value;
        }
        if let Some(value) = patch.channel_id {
            row.channel_id = Some(value);
        }
        if let Some(value) = patch.bosses {
            row.bosses = value;
        }
        if let Some(value) = patch.weekday {
            row.weekday = value;
        }
        if let Some(value) = patch.time {
            row.time = value;
        }
        if let Some(value) = patch.participants {
            row.participants = value;
            // A member who leaves the party loses their standing answer.
            let party = &row.participants;
            row.standing
                .retain(|answer| party.contains(&answer.user_id));
        }
        if let Some(value) = patch.note {
            row.note = Some(value);
        }
    }

    pub fn delete_fixed_run(&mut self, id: &str) {
        self.fixed.remove(id);
    }

    /// Set (`Some`) or clear a member's standing answer on a weekly timing;
    /// a missing timing is a no-op. Answers stay sorted by user id.
    pub fn set_standing(&mut self, fixed_id: &str, user_id: &str, answer: Option<StandingAnswer>) {
        let Some(row) = self.fixed.get_mut(fixed_id) else {
            return;
        };
        row.standing.retain(|existing| existing.user_id != user_id);
        if let Some(answer) = answer {
            row.standing.push(answer);
            row.standing.sort_by(|a, b| a.user_id.cmp(&b.user_id));
        }
    }

    pub fn set_attendance_default(&mut self, fixed_id: &str, default: AttendanceDefault) {
        if let Some(row) = self.fixed.get_mut(fixed_id) {
            row.attendance_default = default;
        }
    }

    /// # Errors
    /// [`ScheduleError::RunMoveConflict`] when the weekly already has a run that week.
    pub fn create_run(
        &mut self,
        ids: &mut impl IdGenerator,
        new: NewRun,
    ) -> Result<String, ScheduleError> {
        let id = ids.new_id();
        self.check_fixed_week(&id, new.fixed_run_id.as_deref(), new.week_start)?;
        let row = Run {
            id: id.clone(),
            fixed_run_id: new.fixed_run_id,
            channel_id: new.channel_id,
            week_start: new.week_start,
            datetime: new.datetime,
            bosses: new.bosses,
            participants: new.participants,
            status: new.status,
            source: new.source,
            attendance: Vec::new(),
            status_pin: None,
        };
        self.runs.insert(id.clone(), row);
        Ok(id)
    }

    pub fn run(&self, id: &str) -> Option<&Run> {
        self.runs.get(id)
    }

    /// # Errors
    /// [`ScheduleError::UnknownRun`] when the run is absent.
    pub fn require_run(&self, id: &str) -> Result<Run, ScheduleError> {
        self.run(id)
            .cloned()
            .ok_or_else(|| ScheduleError::UnknownRun(id.to_owned()))
    }

    pub fn run_for_fixed(&self, fixed_run_id: &str, week_start: DateTime<Utc>) -> Option<&Run> {
        self.runs.values().find(|run| {
            run.fixed_run_id.as_deref() == Some(fixed_run_id) && run.week_start == week_start
        })
    }

    /// Runs ordered by `(datetime, id)`, optionally one boss week only.
    pub fn runs(&self, week_start: Option<DateTime<Utc>>) -> Vec<Run> {
        let mut rows: Vec<Run> = self
            .runs
            .values()
            .filter(|run| week_start.is_none_or(|week| run.week_start == week))
            .cloned()
            .collect();
        rows.sort_by(|a, b| (a.datetime, &a.id).cmp(&(b.datetime, &b.id)));
        rows
    }

    /// A run revived from done loses its recorded attendance (it records a
    /// night that is no longer over).
    /// Leaving the answer-driven statuses (done, cancelled, otot) ends a
    /// hand-set status pin.
    pub fn set_run_status(&mut self, id: &str, status: RunStatus) {
        if let Some(run) = self.runs.get_mut(id) {
            if run.status == RunStatus::Done && status != RunStatus::Done {
                run.attendance.clear();
            }
            if !matches!(
                status,
                RunStatus::Planned | RunStatus::Confirmed | RunStatus::AtRisk
            ) {
                run.status_pin = None;
            }
            run.status = status;
        }
    }

    /// Set or clear a run's hand-set status pin (v5).
    pub fn set_run_pin(&mut self, id: &str, pin: Option<StatusPin>) {
        if let Some(run) = self.runs.get_mut(id) {
            run.status_pin = pin;
        }
    }

    /// # Errors
    /// [`ScheduleError::RunMoveConflict`] when the weekly already has a run that week.
    pub fn set_run_datetime(
        &mut self,
        id: &str,
        at: DateTime<Utc>,
        week_start: DateTime<Utc>,
    ) -> Result<(), ScheduleError> {
        let Some(fixed) = self.runs.get(id).map(|run| run.fixed_run_id.clone()) else {
            return Ok(());
        };
        self.check_fixed_week(id, fixed.as_deref(), week_start)?;
        if let Some(run) = self.runs.get_mut(id) {
            run.datetime = at;
            run.week_start = week_start;
        }
        Ok(())
    }

    /// # Errors
    /// [`ScheduleError::RunMoveConflict`] when that weekly already has a run in this run's week.
    pub fn set_run_fixed(
        &mut self,
        id: &str,
        fixed_run_id: Option<String>,
    ) -> Result<(), ScheduleError> {
        let Some(week) = self.runs.get(id).map(|run| run.week_start) else {
            return Ok(());
        };
        self.check_fixed_week(id, fixed_run_id.as_deref(), week)?;
        if let Some(run) = self.runs.get_mut(id) {
            run.fixed_run_id = fixed_run_id;
        }
        Ok(())
    }

    /// Members no longer on the run lose their recorded attendance, and a
    /// changed party ends a hand-set status pin (callers re-derive).
    pub fn set_run_participants(&mut self, id: &str, participants: Vec<String>) {
        if let Some(run) = self.runs.get_mut(id) {
            run.attendance
                .retain(|record| participants.contains(&record.user_id));
            if run.participants != participants {
                run.status_pin = None;
            }
            run.participants = participants;
        }
    }

    /// Replace a run's recorded attendance (kept sorted by user id).
    pub fn set_run_attendance(&mut self, id: &str, mut records: Vec<AttendanceRecord>) {
        if let Some(run) = self.runs.get_mut(id) {
            records.sort_by(|a, b| a.user_id.cmp(&b.user_id));
            run.attendance = records;
        }
    }

    pub fn set_run_bosses(&mut self, id: &str, bosses: Vec<String>) {
        if let Some(run) = self.runs.get_mut(id) {
            run.bosses = bosses;
        }
    }

    pub fn set_run_channel(&mut self, id: &str, channel_id: Option<String>) {
        if let Some(run) = self.runs.get_mut(id) {
            run.channel_id = channel_id;
        }
    }

    /// Upsert by `(run_id, user_id)`, as v4's `ON CONFLICT DO UPDATE`.
    pub fn set_rsvp(
        &mut self,
        run_id: &str,
        user_id: &str,
        state: RsvpState,
        source: RsvpSource,
        at: DateTime<Utc>,
    ) {
        let row = Rsvp {
            run_id: run_id.to_owned(),
            user_id: user_id.to_owned(),
            state,
            source,
            at,
        };
        self.rsvps.insert(rsvp_key(&row), row);
    }

    pub fn clear_rsvp(&mut self, run_id: &str, user_id: &str) {
        self.rsvps.remove(&(run_id.to_owned(), user_id.to_owned()));
    }

    /// `user_id -> state` for one run.
    pub fn rsvps(&self, run_id: &str) -> BTreeMap<String, RsvpState> {
        self.rsvps
            .values()
            .filter(|row| row.run_id == run_id)
            .map(|row| (row.user_id.clone(), row.state))
            .collect()
    }

    /// v4 `INSERT OR IGNORE`: an id is always drawn, and `None` means the
    /// `(run_id, kind)` row already existed and nothing was written.
    pub fn add_reminder(
        &mut self,
        ids: &mut impl IdGenerator,
        run_id: &str,
        kind: &str,
        fire_at: DateTime<Utc>,
        sent_at: Option<DateTime<Utc>>,
    ) -> Option<String> {
        let id = ids.new_id();
        let taken = self
            .reminders
            .values()
            .any(|row| row.run_id == run_id && row.kind == kind);
        if taken || self.reminders.contains_key(&id) {
            return None;
        }
        let row = Reminder {
            id: id.clone(),
            run_id: run_id.to_owned(),
            kind: kind.to_owned(),
            fire_at,
            sent_at,
            message_id: None,
        };
        self.reminders.insert(id.clone(), row);
        Some(id)
    }

    pub fn reminder(&self, id: &str) -> Option<&Reminder> {
        self.reminders.get(id)
    }

    /// One run's reminders ordered by `(fire_at, kind)`.
    pub fn reminders(&self, run_id: &str) -> Vec<Reminder> {
        let mut rows: Vec<Reminder> = self
            .reminders
            .values()
            .filter(|row| row.run_id == run_id)
            .cloned()
            .collect();
        rows.sort_by(|a, b| (a.fire_at, &a.kind).cmp(&(b.fire_at, &b.kind)));
        rows
    }

    /// Put a weekly timing back exactly as recorded (history revert).
    pub fn put_fixed_run(&mut self, row: FixedRun) {
        self.fixed.insert(row.id.clone(), row);
    }

    /// Put a run back exactly as recorded (history revert).
    pub fn put_run(&mut self, row: Run) {
        self.runs.insert(row.id.clone(), row);
    }

    pub fn rsvp(&self, run_id: &str, user_id: &str) -> Option<&Rsvp> {
        self.rsvps.get(&(run_id.to_owned(), user_id.to_owned()))
    }

    /// Put an RSVP back exactly as recorded (history revert).
    pub fn put_rsvp(&mut self, row: Rsvp) {
        self.rsvps.insert(rsvp_key(&row), row);
    }

    pub fn delete_reminder(&mut self, id: &str) {
        self.reminders.remove(id);
    }

    /// Drop every reminder of a run, sent ones included.
    pub fn delete_reminders(&mut self, run_id: &str) {
        self.reminders.retain(|_, row| row.run_id != run_id);
    }

    /// Drop a run's unsent reminders except `keep_kinds`.
    pub fn delete_unsent_reminders(&mut self, run_id: &str, keep_kinds: &[&str]) {
        self.reminders.retain(|_, row| {
            row.run_id != run_id || row.sent_at.is_some() || keep_kinds.contains(&row.kind.as_str())
        });
    }

    /// v4 `reschedule_unposted_reminder`: never touches a posted card. A future
    /// time reopens the row; a past one is written as already handled. Returns
    /// whether the row changed. A row whose send was retired without proof
    /// of delivery is never reopened, since that send may have landed.
    pub fn reschedule_unposted_reminder(
        &mut self,
        id: &str,
        fire_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> bool {
        let Some(row) = self.reminders.get_mut(id) else {
            return false;
        };
        if row.message_id.is_some() {
            return false;
        }
        if fire_at > now {
            if (row.fire_at == fire_at && row.sent_at.is_none())
                || self.base.unproven_retired.contains(id)
            {
                return false;
            }
            row.sent_at = None;
        } else {
            if row.fire_at == fire_at && row.sent_at.is_some() {
                return false;
            }
            row.sent_at = row.sent_at.or(Some(now));
        }
        row.fire_at = fire_at;
        true
    }

    /// Record a send; an empty message id is stored as none, as v4.
    pub fn mark_reminder_sent(&mut self, id: &str, message_id: Option<&str>, at: DateTime<Utc>) {
        if let Some(row) = self.reminders.get_mut(id) {
            row.sent_at = Some(at);
            row.message_id = message_id
                .filter(|text| !text.is_empty())
                .map(str::to_owned);
        }
    }

    /// The draft's current rows as a snapshot (store order), keeping the
    /// loaded revision and unproven-retirement set.
    pub fn to_snapshot(&self) -> ScheduleSnapshot {
        let mut snapshot = ScheduleSnapshot {
            revision: self.base.revision,
            fixed_runs: self.fixed.values().cloned().collect(),
            runs: self.runs.values().cloned().collect(),
            reminders: self.reminders.values().cloned().collect(),
            rsvps: self.rsvps.values().cloned().collect(),
            unproven_retired: self.base.unproven_retired.clone(),
        };
        snapshot.sort();
        snapshot
    }

    /// The row writes that turn the loaded snapshot into this draft.
    pub fn into_changes(self) -> ChangeSet {
        let base_fixed = keyed(&self.base.fixed_runs, |row| row.id.clone());
        let base_runs = keyed(&self.base.runs, |row| row.id.clone());
        let base_reminders = keyed(&self.base.reminders, |row| row.id.clone());
        let base_rsvps = keyed(&self.base.rsvps, rsvp_key);
        let mut changes = Vec::new();
        for id in base_reminders.keys() {
            if !self.reminders.contains_key(id) {
                changes.push(Change::DeleteReminder(id.clone()));
            }
        }
        for (run_id, user_id) in base_rsvps.keys() {
            if !self.rsvps.contains_key(&(run_id.clone(), user_id.clone())) {
                changes.push(Change::DeleteRsvp {
                    run_id: run_id.clone(),
                    user_id: user_id.clone(),
                });
            }
        }
        for id in base_fixed.keys() {
            if !self.fixed.contains_key(id) {
                changes.push(Change::DeleteFixedRun(id.clone()));
            }
        }
        // Runs are never deleted by scheduler rules; a vanished run is a bug.
        debug_assert!(base_runs.keys().all(|id| self.runs.contains_key(id)));
        puts(&base_fixed, self.fixed, Change::PutFixedRun, &mut changes);
        puts(&base_runs, self.runs, Change::PutRun, &mut changes);
        puts(
            &base_reminders,
            self.reminders,
            Change::PutReminder,
            &mut changes,
        );
        puts(&base_rsvps, self.rsvps, Change::PutRsvp, &mut changes);
        ChangeSet { changes }
    }

    fn check_fixed_week(
        &self,
        run_id: &str,
        fixed_run_id: Option<&str>,
        week_start: DateTime<Utc>,
    ) -> Result<(), ScheduleError> {
        match fixed_run_id.and_then(|fixed| self.run_for_fixed(fixed, week_start)) {
            Some(other) if other.id != run_id => Err(ScheduleError::RunMoveConflict),
            _ => Ok(()),
        }
    }
}

fn rsvp_key(row: &Rsvp) -> RsvpKey {
    (row.run_id.clone(), row.user_id.clone())
}

fn keyed<K: Ord, T: Clone>(rows: &[T], key: impl Fn(&T) -> K) -> BTreeMap<K, T> {
    rows.iter().map(|row| (key(row), row.clone())).collect()
}

fn puts<K: Ord, T: PartialEq>(
    base: &BTreeMap<K, T>,
    current: BTreeMap<K, T>,
    wrap: impl Fn(T) -> Change,
    changes: &mut Vec<Change>,
) {
    for (key, row) in current {
        if base.get(&key) != Some(&row) {
            changes.push(wrap(row));
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};

    use super::*;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 1, hour, 0, 0)
            .single()
            .expect("valid instant")
    }

    fn skipped(id: &str) -> Reminder {
        Reminder {
            id: id.into(),
            run_id: "r-1".into(),
            kind: "day_of".into(),
            fire_at: at(8),
            sent_at: Some(at(8)),
            message_id: None,
        }
    }

    #[test]
    fn unproven_retired_reminder_is_never_reopened() {
        let mut snapshot = ScheduleSnapshot {
            reminders: vec![skipped("m-unproven"), skipped("m-skipped")],
            ..ScheduleSnapshot::default()
        };
        snapshot.unproven_retired.insert("m-unproven".into());
        let mut draft = Draft::new(snapshot);
        let later = at(12);
        let now = at(9);
        assert!(!draft.reschedule_unposted_reminder("m-unproven", later, now));
        assert!(draft.reschedule_unposted_reminder("m-skipped", later, now));
        assert!(
            draft.reschedule_unposted_reminder("m-unproven", now - TimeDelta::hours(2), now),
            "a past time only moves the handled row, as v4"
        );
        let changes = draft.into_changes();
        let reopened: Vec<&Reminder> = changes
            .changes
            .iter()
            .filter_map(|change| match change {
                Change::PutReminder(row) if row.sent_at.is_none() => Some(row),
                _ => None,
            })
            .collect();
        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened[0].id, "m-skipped");
    }
}
