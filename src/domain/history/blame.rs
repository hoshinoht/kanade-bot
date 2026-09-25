//! Blame: for each field of a run or weekly timing, the last recorded change
//! that set it.
//!
//! Stores keep a field index (`target`, `field` → record `seq`) written in
//! the same transaction as each record, from [`changed_fields`], so a blame
//! is one index lookup plus one record load per field. Values that no record
//! has set (rows from before the history began, or imported) report no
//! attribution: "unknown (before history)".

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use chrono::{DateTime, Utc};

use super::origin::{Actor, Surface};
use super::port::{ChangeHistory, CheckedChange};
use super::precondition::{EDIT_OVERRIDE, Versioned};
use super::record::{ChangeRecord, ChangeRef, RowKey, RowValue};
use crate::domain::attendance::AttendanceRecord;
use crate::domain::schedule::{FixedRun, ROLLBACK_RESTORED, Run};
use crate::domain::scheduler::{ScheduleStore, Scope, StoreError};

/// What a blame is about.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BlameTarget {
    Run(String),
    FixedRun(String),
}

impl BlameTarget {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Run(_) => "run",
            Self::FixedRun(_) => "fixed_run",
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Run(id) | Self::FixedRun(id) => id,
        }
    }
}

/// A run's blamed fields, in display order; RSVPs follow as `rsvp:<user>`.
/// `slot` covers the instant, boss week, source and weekly-timing link.
pub const RUN_FIELDS: &[&str] = &["slot", "bosses", "participants", "channel", "status"];
/// A weekly timing's blamed fields.
pub const FIXED_RUN_FIELDS: &[&str] = &[
    "day",
    "time",
    "bosses",
    "participants",
    "channel",
    "note",
    "owner",
];

pub fn rsvp_field(user_id: &str) -> String {
    format!("rsvp:{user_id}")
}

/// A weekly timing's attendance default (v5; blamed once it was set).
pub const ATTENDANCE_DEFAULT_FIELD: &str = "attendance_default";

/// A member's standing answer on a weekly timing (v5).
pub fn standing_field(user_id: &str) -> String {
    format!("standing:{user_id}")
}

/// The v5 attendance fields a weekly-timing row change set.
fn attendance_fields(before: Option<&FixedRun>, after: Option<&FixedRun>) -> Vec<String> {
    let Some(after) = after else {
        return Vec::new();
    };
    let default_before = before.map(|row| row.attendance_default).unwrap_or_default();
    let mut fields = Vec::new();
    if default_before != after.attendance_default {
        fields.push(ATTENDANCE_DEFAULT_FIELD.to_owned());
    }
    let standing = |row: Option<&FixedRun>| -> BTreeSet<(String, String, DateTime<Utc>)> {
        row.map(|row| {
            row.standing
                .iter()
                .map(|answer| (answer.user_id.clone(), answer.set_by.clone(), answer.at))
                .collect()
        })
        .unwrap_or_default()
    };
    let (was, now) = (standing(before), standing(Some(after)));
    let users: BTreeSet<&String> = was
        .symmetric_difference(&now)
        .map(|(user, ..)| user)
        .collect();
    fields.extend(users.into_iter().map(|user| standing_field(user)));
    fields
}

/// A run's hand-set status pin (v5; blamed while held or once set).
pub const STATUS_PIN_FIELD: &str = "status_pin";

/// A member's recorded attendance on a run (v5).
pub fn attended_field(user_id: &str) -> String {
    format!("attended:{user_id}")
}

/// The attendance entries a run-row change set (added, changed or removed).
fn attended_fields(before: Option<&Run>, after: Option<&Run>) -> Vec<String> {
    let Some(after) = after else {
        return Vec::new();
    };
    let records = |row: Option<&Run>| -> BTreeSet<AttendanceRecord> {
        row.map(|row| row.attendance.iter().cloned().collect())
            .unwrap_or_default()
    };
    let (was, now) = (records(before), records(Some(after)));
    let users: BTreeSet<&String> = was
        .symmetric_difference(&now)
        .map(|record| &record.user_id)
        .collect();
    users.into_iter().map(|user| attended_field(user)).collect()
}

fn run_fields(before: Option<&Run>, after: Option<&Run>) -> Vec<&'static str> {
    let Some(after) = after else {
        return Vec::new();
    };
    let Some(before) = before else {
        return RUN_FIELDS.to_vec();
    };
    let slot = (
        before.week_start,
        before.datetime,
        before.source,
        &before.fixed_run_id,
    ) != (
        after.week_start,
        after.datetime,
        after.source,
        &after.fixed_run_id,
    );
    [
        ("slot", slot),
        ("bosses", before.bosses != after.bosses),
        ("participants", before.participants != after.participants),
        ("channel", before.channel_id != after.channel_id),
        ("status", before.status != after.status),
    ]
    .into_iter()
    .filter_map(|(field, changed)| changed.then_some(field))
    .collect()
}

fn fixed_fields(before: Option<&FixedRun>, after: Option<&FixedRun>) -> Vec<&'static str> {
    let Some(after) = after else {
        return Vec::new();
    };
    let Some(before) = before else {
        return FIXED_RUN_FIELDS.to_vec();
    };
    [
        ("day", before.weekday != after.weekday),
        ("time", before.time != after.time),
        ("bosses", before.bosses != after.bosses),
        ("participants", before.participants != after.participants),
        ("channel", before.channel_id != after.channel_id),
        ("note", before.note != after.note),
        ("owner", before.owner_id != after.owner_id),
    ]
    .into_iter()
    .filter_map(|(field, changed)| changed.then_some(field))
    .collect()
}

/// Every `(target, field)` a record set. Reminder rows are never blamed.
pub fn changed_fields(record: &ChangeRecord) -> BTreeSet<(BlameTarget, String)> {
    let mut fields = BTreeSet::new();
    for row in &record.rows {
        match &row.key {
            RowKey::Run(id) => {
                let run = |value: &Option<RowValue>| match value {
                    Some(RowValue::Run(run)) => Some(run.clone()),
                    _ => None,
                };
                let (before, after) = (run(&row.before), run(&row.after));
                for field in run_fields(before.as_ref(), after.as_ref()) {
                    fields.insert((BlameTarget::Run(id.clone()), field.to_owned()));
                }
                for field in attended_fields(before.as_ref(), after.as_ref()) {
                    fields.insert((BlameTarget::Run(id.clone()), field));
                }
                if let Some(after) = &after
                    && before.as_ref().and_then(|run| run.status_pin) != after.status_pin
                {
                    fields.insert((BlameTarget::Run(id.clone()), STATUS_PIN_FIELD.to_owned()));
                }
            }
            RowKey::FixedRun(id) => {
                let fixed = |value: &Option<RowValue>| match value {
                    Some(RowValue::FixedRun(row)) => Some(row.clone()),
                    _ => None,
                };
                let (before, after) = (fixed(&row.before), fixed(&row.after));
                for field in fixed_fields(before.as_ref(), after.as_ref()) {
                    fields.insert((BlameTarget::FixedRun(id.clone()), field.to_owned()));
                }
                for field in attendance_fields(before.as_ref(), after.as_ref()) {
                    fields.insert((BlameTarget::FixedRun(id.clone()), field));
                }
            }
            RowKey::Rsvp { run_id, user_id } => {
                fields.insert((BlameTarget::Run(run_id.clone()), rsvp_field(user_id)));
            }
            RowKey::Reminder(_) => {}
        }
    }
    fields
}

/// How the attributed change came about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Via {
    /// Made directly.
    Direct,
    /// A rollback: the records it undid and, for a checkpoint restore, the
    /// checkpoint head it went back to (never itself undone).
    Rollback {
        undid: Vec<ChangeRef>,
        restored_to: Option<ChangeRef>,
    },
    /// Another change carrying references (e.g. a cherry-pick).
    Referenced(Vec<ChangeRef>),
    /// An administrator applied this change over newer changes
    /// (`notice.edit.override`): an edit's "apply mine anyway"
    /// (`picked: None`) or a forced cherry-pick (`picked`: the change it
    /// re-applied). `overridden` are the changes it knowingly overwrote
    /// (empty when the overwritten fields had no recorded change).
    Override {
        picked: Option<ChangeRef>,
        overridden: Vec<ChangeRef>,
    },
}

/// The last change that set a field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribution {
    pub seq: u64,
    pub actor: Actor,
    pub surface: Surface,
    pub at: DateTime<Utc>,
    pub via: Via,
}

impl Attribution {
    fn of(record: &ChangeRecord) -> Self {
        let via = if record.origin.surface == Surface::Rollback {
            let mut undid = record.refs.clone();
            // A checkpoint restore appends the checkpoint head last.
            let restored_to = if record.notices.iter().any(|kind| kind == ROLLBACK_RESTORED) {
                undid.pop()
            } else {
                None
            };
            Via::Rollback { undid, restored_to }
        } else if record.notices.iter().any(|kind| kind == EDIT_OVERRIDE) {
            // A cherry-pick names the picked change first.
            let mut overridden = record.refs.clone();
            let picked = (record.origin.surface == Surface::CherryPick && !overridden.is_empty())
                .then(|| overridden.remove(0));
            Via::Override { picked, overridden }
        } else if record.refs.is_empty() {
            Via::Direct
        } else {
            Via::Referenced(record.refs.clone())
        };
        Self {
            seq: record.seq,
            actor: record.origin.actor.clone(),
            surface: record.origin.surface,
            at: record.at,
            via,
        }
    }
}

/// One blamed field; `last` is `None` when no record set it (the value
/// predates the history).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    pub field: String,
    pub last: Option<Attribution>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blame {
    pub target: BlameTarget,
    pub lines: Vec<BlameLine>,
}

/// The field index a store keeps beside its history.
pub trait BlameIndex {
    /// For each field of `target` any record set, the last record's `seq`.
    fn last_changes(
        &self,
        target: &BlameTarget,
    ) -> impl Future<Output = Result<BTreeMap<String, u64>, StoreError>> + Send;

    /// Each target's row (a run's RSVP rows too) and its field versions,
    /// all read in ONE read transaction: the versions an edit screen must
    /// declare as its preconditions.
    fn read_versioned(
        &self,
        targets: &[BlameTarget],
    ) -> impl Future<Output = Result<Vec<Versioned>, StoreError>> + Send;
}

/// Blame a run (with its RSVPs) or weekly timing; `None` when it does not
/// exist.
///
/// # Errors
/// The store failed, or an attributed record no longer matches its hash.
pub async fn blame<S: ScheduleStore + ChangeHistory + BlameIndex>(
    store: &S,
    target: &BlameTarget,
) -> Result<Option<Blame>, StoreError> {
    let mut fields: Vec<String> = match target {
        BlameTarget::Run(id) => {
            let snapshot = store.load(&Scope::Run(id.clone())).await?;
            if snapshot.runs.is_empty() {
                return Ok(None);
            }
            RUN_FIELDS.iter().map(|field| (*field).to_owned()).collect()
        }
        BlameTarget::FixedRun(id) => {
            let snapshot = store.load(&Scope::Weeks(Vec::new())).await?;
            let Some(row) = snapshot.fixed_runs.iter().find(|row| &row.id == id) else {
                return Ok(None);
            };
            let mut fields: Vec<String> = FIXED_RUN_FIELDS
                .iter()
                .map(|field| (*field).to_owned())
                .collect();
            // v5 attendance fields: the default once set, and every standing
            // answer held now or recorded before.
            let last = store.last_changes(target).await?;
            if last.contains_key(ATTENDANCE_DEFAULT_FIELD) {
                fields.push(ATTENDANCE_DEFAULT_FIELD.to_owned());
            }
            let mut standing: BTreeSet<String> = row
                .standing
                .iter()
                .map(|answer| standing_field(&answer.user_id))
                .collect();
            standing.extend(
                last.keys()
                    .filter(|field| field.starts_with("standing:"))
                    .cloned(),
            );
            fields.extend(standing);
            fields
        }
    };
    let last = store.last_changes(target).await?;
    if let BlameTarget::Run(id) = target {
        let snapshot = store.load(&Scope::Run(id.clone())).await?;
        let mut users: BTreeSet<String> = snapshot
            .rsvps
            .iter()
            .map(|row| rsvp_field(&row.user_id))
            .collect();
        users.extend(
            last.keys()
                .filter(|field| field.starts_with("rsvp:"))
                .cloned(),
        );
        fields.extend(users);
        // v5: the status pin while held or once set.
        if snapshot.runs.iter().any(|run| run.status_pin.is_some())
            || last.contains_key(STATUS_PIN_FIELD)
        {
            fields.push(STATUS_PIN_FIELD.to_owned());
        }
        // v5: every attendance entry held now or recorded before.
        let mut attended: BTreeSet<String> = snapshot
            .runs
            .iter()
            .flat_map(|run| &run.attendance)
            .map(|record| attended_field(&record.user_id))
            .collect();
        attended.extend(
            last.keys()
                .filter(|field| field.starts_with("attended:"))
                .cloned(),
        );
        fields.extend(attended);
    }
    let mut lines = Vec::new();
    // Each attributed record is loaded (and hash-checked) once; every line
    // checks that its record really set that field.
    let mut loaded: BTreeMap<u64, (Attribution, BTreeSet<(BlameTarget, String)>)> = BTreeMap::new();
    for field in fields {
        let attribution = match last.get(&field) {
            None => None,
            Some(seq) => {
                if !loaded.contains_key(seq) {
                    let record = match store.load_checked(*seq).await? {
                        CheckedChange::Intact(record) => record,
                        CheckedChange::Missing => {
                            return Err(StoreError::Backend(format!(
                                "blame index names missing change {seq}"
                            )));
                        }
                        CheckedChange::Tampered(reason) => {
                            return Err(StoreError::Backend(format!("change {seq}: {reason}")));
                        }
                    };
                    loaded.insert(*seq, (Attribution::of(&record), changed_fields(&record)));
                }
                let Some((attribution, set)) = loaded.get(seq) else {
                    continue;
                };
                if !set.contains(&(target.clone(), field.clone())) {
                    return Err(StoreError::Backend(format!(
                        "blame index names change {seq}, which did not set {field}"
                    )));
                }
                Some(attribution.clone())
            }
        };
        lines.push(BlameLine {
            field,
            last: attribution,
        });
    }
    Ok(Some(Blame {
        target: target.clone(),
        lines,
    }))
}
