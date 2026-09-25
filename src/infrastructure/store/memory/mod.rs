use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use chrono::{DateTime, Timelike, Utc};

use crate::domain::attendance::PastRun;
use crate::domain::drafts::{
    DraftChange, DraftCreated, DraftEvent, DraftEventKind, DraftStale, DraftStore, DraftWrite,
    LoadedDraft, MergeCommit, NewDraft, RequestLimit, StoredDraft,
};
use crate::domain::history::{
    Actor, ChangeFilter, ChangeHistory, ChangeMeta, ChangePage, ChangeQuery, ChangeRecord,
    ChangeRef, CheckedChange, HistoryVerification, RowKey, RowValue, verify_chain,
};
use crate::domain::history::{
    BlameIndex, BlameTarget, Checkpoint, CheckpointCreated, CheckpointKind, Checkpoints, FieldNow,
    LastChange, NewCheckpoint, PreconditionError, Versioned, changed_fields, check_checkpoint,
    check_field, target_key, validate_fields,
};
use crate::domain::schedule::{
    Change, ChangeSet, FixedRun, Reminder, Rsvp, Run, RunStatus, ScheduleSnapshot,
};
use crate::domain::scheduler::{
    AttendanceHistory, Committed, RecordedRequest, ScheduleStore, Scope, StoreError,
};

use super::history::{changed_rows, touched_keys, touched_weeks};
use super::order::sort_snapshot;

mod journal;
mod members;
mod model_log;
mod proposal_cards;
mod proposals;
mod settings;
mod web_sessions;

use journal::JournalTables;

#[derive(Clone, Debug, Default)]
struct DraftTables {
    drafts: BTreeMap<String, StoredDraft>,
    /// Draft ids in creation order.
    order: Vec<String>,
    ops: BTreeMap<String, Vec<crate::domain::drafts::StagedOp>>,
    events: BTreeMap<String, Vec<DraftEvent>>,
    requests: BTreeMap<(String, String, String), (String, String)>,
    proposals: BTreeMap<String, crate::domain::drafts::ProposalInfo>,
    /// Proposal cards by proposal id, with their creation order.
    cards: BTreeMap<String, (u64, crate::domain::proposals::StoredCard)>,
}

#[derive(Clone, Debug, Default)]
struct Tables {
    revision: u64,
    fixed: BTreeMap<String, FixedRun>,
    runs: BTreeMap<String, Run>,
    reminders: BTreeMap<String, Reminder>,
    rsvps: BTreeMap<(String, String), Rsvp>,
    journal: JournalTables,
    history: History,
    drafts: DraftTables,
}

/// The change records (from genesis), the request digests stored beside
/// them, the blame index (last `seq` per target field) and checkpoints.
#[derive(Clone, Debug)]
struct History {
    records: Vec<ChangeRecord>,
    digests: BTreeMap<u64, String>,
    fields: BTreeMap<(BlameTarget, String), u64>,
    checkpoints: Vec<Checkpoint>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            // The epoch is always representable, so genesis always seals.
            records: ChangeRecord::genesis(0).into_iter().collect(),
            digests: BTreeMap::new(),
            fields: BTreeMap::new(),
            checkpoints: Vec::new(),
        }
    }
}

impl History {
    fn request(&self, actor: &Actor, request_id: &str) -> Option<RecordedRequest> {
        self.records
            .iter()
            .find(|record| {
                &record.origin.actor == actor
                    && record.origin.request_id.as_deref() == Some(request_id)
            })
            .map(|record| RecordedRequest {
                committed: Committed {
                    seq: record.seq,
                    revision: record.revision,
                    replayed: true,
                },
                digest: self.digests.get(&record.seq).cloned(),
            })
    }
}

impl Tables {
    fn value(&self, key: &RowKey) -> Option<RowValue> {
        match key {
            RowKey::FixedRun(id) => self.fixed.get(id).cloned().map(RowValue::FixedRun),
            RowKey::Run(id) => self.runs.get(id).cloned().map(RowValue::Run),
            RowKey::Reminder(id) => self.reminders.get(id).cloned().map(RowValue::Reminder),
            RowKey::Rsvp { run_id, user_id } => self
                .rsvps
                .get(&(run_id.clone(), user_id.clone()))
                .cloned()
                .map(RowValue::Rsvp),
        }
    }
}

/// An in-process [`ScheduleStore`] and delivery journal with the same
/// invariants a durable store keeps.
#[derive(Debug, Default)]
pub struct MemoryScheduleStore {
    tables: Mutex<Tables>,
    logs: Mutex<model_log::LogTables>,
    sessions: Mutex<BTreeMap<String, crate::infrastructure::store::web_sessions::WebSession>>,
    members: Mutex<BTreeMap<String, crate::domain::members::MemberProfile>>,
    config: Mutex<BTreeMap<String, String>>,
}

impl MemoryScheduleStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn tables(&self) -> std::sync::MutexGuard<'_, Tables> {
        // A panic mid-commit never leaves partial state: commits swap whole tables.
        self.tables
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl AttendanceHistory for MemoryScheduleStore {
    async fn member_history(
        &self,
        member: &str,
        per_timing: usize,
    ) -> Result<Vec<PastRun>, StoreError> {
        let tables = self.tables();
        let mut runs: Vec<PastRun> = tables
            .runs
            .values()
            .filter(|run| run.status == RunStatus::Done)
            .filter_map(|run| {
                let record = run
                    .attendance
                    .iter()
                    .find(|record| record.user_id == member)?;
                Some(PastRun {
                    run_id: run.id.clone(),
                    fixed_run_id: run.fixed_run_id.clone(),
                    at: run.datetime,
                    explicit: tables
                        .rsvps
                        .get(&(run.id.clone(), member.to_owned()))
                        .map(|rsvp| rsvp.state),
                    attended: record.attended,
                })
            })
            .collect();
        runs.sort_by(|a, b| (b.at, &b.run_id).cmp(&(a.at, &a.run_id)));
        let mut taken: BTreeMap<Option<String>, usize> = BTreeMap::new();
        runs.retain(|run| {
            let count = taken.entry(run.fixed_run_id.clone()).or_default();
            *count += 1;
            *count <= per_timing
        });
        Ok(runs)
    }
}

impl ScheduleStore for MemoryScheduleStore {
    async fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> Result<Option<RecordedRequest>, StoreError> {
        Ok(self.tables().history.request(actor, request_id))
    }

    async fn load(&self, scope: &Scope) -> Result<ScheduleSnapshot, StoreError> {
        Ok(snapshot(&self.tables(), scope))
    }

    async fn commit(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
    ) -> Result<Option<Committed>, StoreError> {
        let mut tables = self.tables();
        if let Some(request_id) = &meta.origin.request_id
            && let Some(earlier) = tables.history.request(&meta.origin.actor, request_id)
        {
            if earlier.digest != meta.request_digest {
                return Err(StoreError::IdempotencyMismatch {
                    seq: earlier.committed.seq,
                });
            }
            return Ok(Some(earlier.committed));
        }
        check_expect(&tables, &meta)?;
        if tables.revision != expected_revision {
            return Err(StoreError::Conflict {
                expected: expected_revision,
                found: tables.revision,
            });
        }
        if changes.is_empty() {
            return Ok(None);
        }
        let keys = touched_keys(&changes);
        let mut next = tables.clone();
        for change in changes.changes {
            apply(&mut next, change)?;
        }
        validate(&next)?;
        next.revision += 1;
        let rows = changed_rows(&keys, |key| tables.value(key), |key| next.value(key));
        let weeks = touched_weeks(&rows, |run_id| {
            next.runs
                .get(run_id)
                .or_else(|| tables.runs.get(run_id))
                .map(|run| run.week_start)
        });
        let request_digest = meta.request_digest.clone();
        let record = ChangeRecord::seal(
            next.history
                .records
                .last()
                .map(|last| (last.seq, last.hash.as_str())),
            uuid::Uuid::new_v4().to_string(),
            next.revision,
            meta.clone(),
            weeks,
            rows,
        )
        .map_err(|error| StoreError::Constraint(error.to_string()))?;
        meta.expect
            .check_overrides_changed(&changed_fields(&record))
            .map_err(StoreError::Precondition)?;
        let committed = Committed {
            seq: record.seq,
            revision: record.revision,
            replayed: false,
        };
        if let Some(digest) = request_digest {
            next.history.digests.insert(record.seq, digest);
        }
        for key in changed_fields(&record) {
            next.history.fields.insert(key, record.seq);
        }
        next.history.records.push(record);
        *tables = next;
        Ok(Some(committed))
    }
}

/// A `merged` event's detail: the record seq, then the approval's note.
fn merged_detail(seq: u64, note: Option<&str>) -> String {
    match note {
        Some(note) => format!("{seq} {note}"),
        None => seq.to_string(),
    }
}

/// The commit's preconditions against the blame index, under the same lock
/// as the write.
fn check_expect(tables: &Tables, meta: &ChangeMeta) -> Result<(), StoreError> {
    let expect = &meta.expect;
    validate_fields(&expect.fields).map_err(StoreError::Precondition)?;
    expect
        .check_override_actor(&meta.origin.actor)
        .map_err(StoreError::Precondition)?;
    let record = |seq: u64| {
        tables
            .history
            .records
            .iter()
            .find(|record| record.seq == seq)
    };
    let mut stale = Vec::new();
    for precondition in &expect.fields {
        let last = tables
            .history
            .fields
            .get(&(precondition.target.clone(), precondition.field.clone()))
            .and_then(|seq| record(*seq))
            .map(|record| LastChange {
                change: record.reference(),
                actor: record.origin.actor.clone(),
                at: record.at,
            });
        let now = FieldNow {
            last,
            target_exists: tables.value(&precondition.target_key()).is_some(),
            ever_recorded: tables
                .history
                .fields
                .keys()
                .any(|(target, _)| *target == precondition.target),
            value: tables.value(&precondition.value_key()),
        };
        stale.extend(check_field(precondition, now).map_err(StoreError::Precondition)?);
    }
    if !stale.is_empty() {
        return Err(StoreError::StaleEdit(stale));
    }
    for reference in &expect.overrides {
        if record(reference.seq).is_none_or(|record| record.hash != reference.hash) {
            return Err(StoreError::Precondition(
                PreconditionError::UnknownOverride { seq: reference.seq },
            ));
        }
    }
    Ok(())
}

/// Stored instants keep microseconds, as the ISO text a durable store writes.
fn micros(at: DateTime<Utc>) -> DateTime<Utc> {
    at.with_nanosecond(at.nanosecond() / 1_000 * 1_000)
        .unwrap_or(at)
}

fn apply(tables: &mut Tables, change: Change) -> Result<(), StoreError> {
    match change {
        Change::PutFixedRun(row) => {
            if row.time.nanosecond() != 0 {
                return Err(StoreError::Constraint(format!(
                    "weekly time {} has sub-second precision",
                    row.time
                )));
            }
            tables.fixed.insert(row.id.clone(), row);
        }
        Change::DeleteFixedRun(id) => {
            tables.fixed.remove(&id);
        }
        Change::PutRun(mut row) => {
            row.week_start = micros(row.week_start);
            row.datetime = micros(row.datetime);
            tables.runs.insert(row.id.clone(), row);
        }
        Change::PutReminder(mut row) => {
            row.fire_at = micros(row.fire_at);
            row.sent_at = row.sent_at.map(micros);
            tables.reminders.insert(row.id.clone(), row);
        }
        Change::DeleteReminder(id) => {
            tables.reminders.remove(&id);
        }
        Change::PutRsvp(mut row) => {
            row.at = micros(row.at);
            tables
                .rsvps
                .insert((row.run_id.clone(), row.user_id.clone()), row);
        }
        Change::DeleteRsvp { run_id, user_id } => {
            tables.rsvps.remove(&(run_id, user_id));
        }
    }
    Ok(())
}

fn validate(tables: &Tables) -> Result<(), StoreError> {
    let orphan = tables
        .reminders
        .values()
        .map(|row| &row.run_id)
        .chain(tables.rsvps.keys().map(|(run_id, _)| run_id))
        .find(|run_id| !tables.runs.contains_key(*run_id));
    if let Some(run_id) = orphan {
        return Err(StoreError::Constraint(format!(
            "run {run_id} does not exist"
        )));
    }
    let mut kinds = BTreeSet::new();
    for row in tables.reminders.values() {
        if !kinds.insert((&row.run_id, &row.kind)) {
            return Err(StoreError::Constraint(format!(
                "reminder ({}, {}) already exists",
                row.run_id, row.kind
            )));
        }
    }
    let mut weeks = BTreeSet::new();
    for row in tables.runs.values() {
        if let Some(fixed) = &row.fixed_run_id
            && !weeks.insert((fixed, row.week_start))
        {
            return Err(StoreError::Constraint(format!(
                "weekly {fixed} already has a run in week {}",
                row.week_start
            )));
        }
    }
    Ok(())
}

fn snapshot(tables: &Tables, scope: &Scope) -> ScheduleSnapshot {
    let run_ids: BTreeSet<&String> = match scope {
        Scope::All => tables.runs.keys().collect(),
        Scope::Weeks(weeks) => tables
            .runs
            .values()
            .filter(|run| weeks.iter().any(|week| micros(*week) == run.week_start))
            .map(|run| &run.id)
            .collect(),
        Scope::Run(id) => tables
            .runs
            .get_key_value(id)
            .map(|(id, _)| id)
            .into_iter()
            .collect(),
        Scope::Reminder(id) => tables
            .reminders
            .get(id)
            .map(|row| &row.run_id)
            .into_iter()
            .collect(),
    };
    let mut snapshot = ScheduleSnapshot {
        unproven_retired: tables
            .reminders
            .values()
            .filter(|row| run_ids.contains(&row.run_id))
            .filter(|row| tables.journal.retired_unproven(&row.id))
            .map(|row| row.id.clone())
            .collect(),
        revision: tables.revision,
        fixed_runs: tables.fixed.values().cloned().collect(),
        runs: run_ids
            .iter()
            .filter_map(|id| tables.runs.get(*id).cloned())
            .collect(),
        reminders: tables
            .reminders
            .values()
            .filter(|row| run_ids.contains(&row.run_id))
            .cloned()
            .collect(),
        rsvps: tables
            .rsvps
            .values()
            .filter(|row| run_ids.contains(&row.run_id))
            .cloned()
            .collect(),
    };
    sort_snapshot(&mut snapshot);
    snapshot
}

impl MemoryScheduleStore {
    /// Test support: edit a stored record in place, as an attacker with
    /// direct access to the history could.
    pub fn tamper_change(&self, seq: u64, edit: impl FnOnce(&mut ChangeRecord)) {
        let mut tables = self.tables();
        if let Some(record) = usize::try_from(seq)
            .ok()
            .and_then(|index| tables.history.records.get_mut(index))
        {
            edit(record);
        }
    }
}

impl ChangeHistory for MemoryScheduleStore {
    async fn load_checked(&self, seq: u64) -> Result<CheckedChange, StoreError> {
        let tables = self.tables();
        let Some(record) = usize::try_from(seq)
            .ok()
            .and_then(|index| tables.history.records.get(index))
        else {
            return Ok(CheckedChange::Missing);
        };
        Ok(match record.computed_hash() {
            Ok(hash) if hash == record.hash => CheckedChange::Intact(Box::new(record.clone())),
            Ok(_) => CheckedChange::Tampered("content does not match its hash".into()),
            Err(error) => CheckedChange::Tampered(error.to_string()),
        })
    }

    async fn load_change(&self, seq: u64) -> Result<Option<ChangeRecord>, StoreError> {
        let tables = self.tables();
        Ok(usize::try_from(seq)
            .ok()
            .and_then(|index| tables.history.records.get(index))
            .cloned())
    }

    async fn list_changes(&self, query: &ChangeQuery) -> Result<ChangePage, StoreError> {
        let tables = self.tables();
        let wanted = query.page_size() + 1;
        let matches = |record: &&ChangeRecord| query.selects(record);
        let records: Vec<ChangeRecord> = if query.newest_first {
            tables
                .history
                .records
                .iter()
                .rev()
                .filter(matches)
                .take(wanted)
                .cloned()
                .collect()
        } else {
            tables
                .history
                .records
                .iter()
                .filter(matches)
                .take(wanted)
                .cloned()
                .collect()
        };
        Ok(ChangePage::from_matches(records, query))
    }

    async fn count_changes(&self, filter: &ChangeFilter) -> Result<u64, StoreError> {
        let query = ChangeQuery::new(filter.clone());
        let tables = self.tables();
        let count = tables
            .history
            .records
            .iter()
            .filter(|record| record.seq > 0 && query.selects(record))
            .count();
        Ok(u64::try_from(count).unwrap_or(u64::MAX))
    }

    async fn verify_history(&self) -> Result<HistoryVerification, StoreError> {
        let tables = self.tables();
        Ok(verify_chain(tables.history.records.iter().map(|record| {
            let body = record
                .canonical()
                .map_err(|error| (record.seq, error.to_string()))?;
            Ok((record.clone(), body))
        })))
    }

    async fn history_head(&self) -> Result<ChangeRef, StoreError> {
        let tables = self.tables();
        tables
            .history
            .records
            .last()
            .map(ChangeRecord::reference)
            .ok_or_else(|| StoreError::Backend("the history has no genesis record".into()))
    }
}

impl BlameIndex for MemoryScheduleStore {
    async fn last_changes(
        &self,
        target: &BlameTarget,
    ) -> Result<BTreeMap<String, u64>, StoreError> {
        let tables = self.tables();
        Ok(tables
            .history
            .fields
            .iter()
            .filter(|((indexed, _), _)| indexed == target)
            .map(|((_, field), seq)| (field.clone(), *seq))
            .collect())
    }

    async fn read_versioned(&self, targets: &[BlameTarget]) -> Result<Vec<Versioned>, StoreError> {
        let tables = self.tables();
        Ok(targets
            .iter()
            .map(|target| Versioned {
                target: target.clone(),
                row: tables.value(&target_key(target)),
                answers: match target {
                    BlameTarget::Run(run_id) => tables
                        .rsvps
                        .values()
                        .filter(|row| &row.run_id == run_id)
                        .cloned()
                        .map(RowValue::Rsvp)
                        .collect(),
                    BlameTarget::FixedRun(_) => Vec::new(),
                },
                versions: tables
                    .history
                    .fields
                    .iter()
                    .filter(|((indexed, _), _)| indexed == target)
                    .map(|((_, field), seq)| (field.clone(), *seq))
                    .collect(),
            })
            .collect())
    }
}

impl Checkpoints for MemoryScheduleStore {
    async fn create_checkpoint(&self, new: NewCheckpoint) -> Result<CheckpointCreated, StoreError> {
        check_checkpoint(&new)?;
        let mut tables = self.tables();
        let history = &mut tables.history;
        if new.kind == CheckpointKind::Auto
            && let Some(existing) = history.checkpoints.iter().find(|row| {
                row.kind == CheckpointKind::Auto && (row.week == new.week || row.name == new.name)
            })
        {
            return Ok(CheckpointCreated::Existing(existing.clone()));
        }
        if history.checkpoints.iter().any(|row| row.name == new.name) {
            return Err(StoreError::Constraint(format!(
                "checkpoint name {:?} is taken",
                new.name
            )));
        }
        let head = history
            .records
            .last()
            .ok_or_else(|| StoreError::Backend("the history has no genesis record".into()))?;
        let created = Checkpoint {
            name: new.name,
            kind: new.kind,
            head: head.reference(),
            revision: head.revision,
            week: new.week,
            created_at: new.created_at,
            created_by: new.created_by,
        };
        history.checkpoints.push(created.clone());
        Ok(CheckpointCreated::Created(created))
    }

    async fn load_checkpoint(&self, name: &str) -> Result<Option<Checkpoint>, StoreError> {
        let tables = self.tables();
        Ok(tables
            .history
            .checkpoints
            .iter()
            .find(|row| row.name == name)
            .cloned())
    }

    async fn list_checkpoints(
        &self,
        week: Option<DateTime<Utc>>,
    ) -> Result<Vec<Checkpoint>, StoreError> {
        let tables = self.tables();
        Ok(tables
            .history
            .checkpoints
            .iter()
            .filter(|row| week.is_none_or(|week| row.week == week))
            .cloned()
            .collect())
    }
}

fn draft_stale_of(draft: &StoredDraft) -> DraftStale {
    DraftStale::Moved {
        status: draft.status,
        version: draft.version,
        merged_seq: draft.merged_seq,
    }
}

fn draft_event(
    tables: &mut Tables,
    draft_id: &str,
    version: u64,
    kind: DraftEventKind,
    actor: &Actor,
    at: chrono::DateTime<chrono::Utc>,
    detail: Option<String>,
) {
    tables
        .drafts
        .events
        .entry(draft_id.to_owned())
        .or_default()
        .push(DraftEvent {
            draft_id: draft_id.to_owned(),
            version,
            kind,
            actor: actor.clone(),
            at,
            detail,
        });
}

impl DraftStore for MemoryScheduleStore {
    async fn snapshot_with_head(&self) -> Result<(ScheduleSnapshot, ChangeRef), StoreError> {
        let tables = self.tables();
        let head = tables
            .history
            .records
            .last()
            .map(ChangeRecord::reference)
            .ok_or_else(|| StoreError::Backend("the history has no genesis record".into()))?;
        Ok((snapshot(&tables, &Scope::All), head))
    }

    async fn records_after(&self, base: &ChangeRef) -> Result<Vec<ChangeRecord>, StoreError> {
        let tables = self.tables();
        let records: Vec<ChangeRecord> = tables
            .history
            .records
            .iter()
            .filter(|record| record.seq > base.seq)
            .cloned()
            .collect();
        Ok(records)
    }

    async fn create_draft(&self, new: NewDraft) -> Result<DraftCreated, StoreError> {
        if new.kind == crate::domain::drafts::DraftKind::Proposal {
            return Err(StoreError::Constraint(
                "proposals are created with create_proposal".into(),
            ));
        }
        let mut tables = self.tables();
        if let Some(request) = &new.request {
            let key = (
                new.author.kind().to_owned(),
                new.author.id().to_owned(),
                request.request_id.clone(),
            );
            if let Some((digest, draft_id)) = tables.drafts.requests.get(&key) {
                let draft = tables
                    .drafts
                    .drafts
                    .get(draft_id)
                    .ok_or_else(|| {
                        StoreError::Backend(format!(
                            "draft_requests points at missing draft {draft_id}"
                        ))
                    })?
                    .clone();
                return if digest == &request.digest {
                    Ok(DraftCreated::Replayed(draft))
                } else {
                    Ok(DraftCreated::Mismatch {
                        draft_id: draft_id.clone(),
                    })
                };
            }
        }
        if tables.drafts.drafts.contains_key(&new.id) {
            return Err(StoreError::Constraint(format!("draft {} exists", new.id)));
        }
        if let Some(submit) = &new.submit {
            let mine = || {
                tables.drafts.drafts.values().filter(|draft| {
                    draft.kind == crate::domain::drafts::DraftKind::Request
                        && draft.author == new.author
                })
            };
            let pending = mine()
                .filter(|draft| draft.status == crate::domain::drafts::DraftStatus::Submitted)
                .count() as u64;
            if pending >= submit.limits.max_pending {
                return Ok(DraftCreated::Limited(RequestLimit::Pending {
                    count: pending,
                    max: submit.limits.max_pending,
                }));
            }
            // Stored instants keep microseconds, as SQLite's text does.
            let since = micros(new.at - submit.limits.window);
            let recent = mine().filter(|draft| draft.created_at > since).count() as u64;
            if recent >= submit.limits.max_per_window {
                return Ok(DraftCreated::Limited(RequestLimit::Rate {
                    count: recent,
                    max: submit.limits.max_per_window,
                }));
            }
        }
        let expires_week = new.submit.as_ref().and_then(|submit| submit.expires_week);
        let stored = StoredDraft {
            id: new.id.clone(),
            kind: new.kind,
            title: new.title.clone(),
            author: new.author.clone(),
            base: new.base.clone(),
            base_revision: new.base_revision,
            version: 1,
            status: if new.submit.is_some() {
                crate::domain::drafts::DraftStatus::Submitted
            } else {
                crate::domain::drafts::DraftStatus::Open
            },
            request_type: new.request_type.clone(),
            subject: new.subject.clone(),
            merged_seq: None,
            closed_by: None,
            close_reason: None,
            created_at: new.at,
            updated_at: new.at,
            scope: match expires_week {
                None => crate::domain::drafts::DraftScope::Weekly,
                Some(week) => crate::domain::drafts::DraftScope::Week(week),
            },
        };
        tables.drafts.drafts.insert(new.id.clone(), stored.clone());
        tables.drafts.order.push(new.id.clone());
        if let Some(request) = &new.request {
            tables.drafts.requests.insert(
                (
                    new.author.kind().to_owned(),
                    new.author.id().to_owned(),
                    request.request_id.clone(),
                ),
                (request.digest.clone(), new.id.clone()),
            );
        }
        let mut next = tables.clone();
        draft_event(
            &mut next,
            &new.id,
            1,
            DraftEventKind::Created,
            &new.author,
            new.at,
            None,
        );
        if let Some(submit) = &new.submit {
            let mut ops = submit.ops.clone();
            for (position, staged) in ops.iter_mut().enumerate() {
                staged.ord = position;
            }
            next.drafts.ops.insert(new.id.clone(), ops);
            draft_event(
                &mut next,
                &new.id,
                1,
                DraftEventKind::Submitted,
                &new.author,
                new.at,
                None,
            );
        }
        *tables = next;
        Ok(DraftCreated::Created(stored))
    }

    async fn load_draft(&self, id: &str) -> Result<Option<LoadedDraft>, StoreError> {
        let tables = self.tables();
        Ok(tables
            .drafts
            .drafts
            .get(id)
            .cloned()
            .map(|draft| LoadedDraft {
                draft,
                ops: tables.drafts.ops.get(id).cloned().unwrap_or_default(),
            }))
    }

    async fn recorded_draft_request(
        &self,
        author: &Actor,
        request_id: &str,
    ) -> Result<Option<(String, StoredDraft)>, StoreError> {
        let tables = self.tables();
        let key = (
            author.kind().to_owned(),
            author.id().to_owned(),
            request_id.to_owned(),
        );
        Ok(tables.drafts.requests.get(&key).and_then(|(digest, id)| {
            tables
                .drafts
                .drafts
                .get(id)
                .map(|draft| (digest.clone(), draft.clone()))
        }))
    }

    async fn list_drafts(
        &self,
        status: Option<crate::domain::drafts::DraftStatus>,
    ) -> Result<Vec<StoredDraft>, StoreError> {
        let tables = self.tables();
        let drafts: Vec<StoredDraft> = tables
            .drafts
            .order
            .iter()
            .filter_map(|id| tables.drafts.drafts.get(id))
            .filter(|draft| status.is_none_or(|status| draft.status == status))
            .cloned()
            .collect();
        Ok(drafts)
    }

    async fn draft_events(&self, id: &str) -> Result<Vec<DraftEvent>, StoreError> {
        let tables = self.tables();
        Ok(tables.drafts.events.get(id).cloned().unwrap_or_default())
    }

    async fn update_draft(
        &self,
        update: crate::domain::drafts::DraftUpdate,
    ) -> Result<DraftWrite, StoreError> {
        let mut tables = self.tables();
        let Some(draft) = tables.drafts.drafts.get(&update.draft_id).cloned() else {
            return Ok(DraftWrite::Stale(DraftStale::Missing));
        };
        if !draft.status.is_live() || draft.version != update.expected_version {
            return Ok(DraftWrite::Stale(draft_stale_of(&draft)));
        }
        let mut next = tables.clone();
        let mut draft = draft;
        match &update.change {
            DraftChange::ReplaceOps {
                ops,
                event,
                ord,
                expires_week,
            } => {
                let mut ops = ops.clone();
                for (position, staged) in ops.iter_mut().enumerate() {
                    staged.ord = position;
                }
                next.drafts.ops.insert(draft.id.clone(), ops);
                draft.version += 1;
                draft.updated_at = update.at;
                draft.scope = match expires_week {
                    None => crate::domain::drafts::DraftScope::Weekly,
                    Some(week) => crate::domain::drafts::DraftScope::Week(*week),
                };
                draft_event(
                    &mut next,
                    &draft.id,
                    draft.version,
                    *event,
                    &update.actor,
                    update.at,
                    Some(ord.to_string()),
                );
            }
            DraftChange::Rebase {
                base,
                base_revision,
                expires_week,
            } => {
                draft.base = base.clone();
                draft.base_revision = *base_revision;
                draft.version += 1;
                draft.updated_at = update.at;
                draft.scope = match expires_week {
                    None => crate::domain::drafts::DraftScope::Weekly,
                    Some(week) => crate::domain::drafts::DraftScope::Week(*week),
                };
                draft_event(
                    &mut next,
                    &draft.id,
                    draft.version,
                    DraftEventKind::Rebased,
                    &update.actor,
                    update.at,
                    Some(format!("{} {}", base.seq, base.hash)),
                );
            }
            DraftChange::Close { status, reason } => {
                let kind = match status {
                    crate::domain::drafts::DraftStatus::Discarded => DraftEventKind::Discarded,
                    crate::domain::drafts::DraftStatus::Rejected => DraftEventKind::Rejected,
                    crate::domain::drafts::DraftStatus::Withdrawn => DraftEventKind::Withdrawn,
                    crate::domain::drafts::DraftStatus::Expired => DraftEventKind::Expired,
                    _ => {
                        return Err(StoreError::Constraint(format!(
                            "cannot close a draft as {status}"
                        )));
                    }
                };
                draft.status = *status;
                draft.closed_by = Some(update.actor.clone());
                draft.close_reason = reason.clone();
                draft.updated_at = update.at;
                draft_event(
                    &mut next,
                    &draft.id,
                    draft.version,
                    kind,
                    &update.actor,
                    update.at,
                    reason.clone(),
                );
            }
        }
        next.drafts.drafts.insert(draft.id.clone(), draft.clone());
        *tables = next;
        Ok(DraftWrite::Written(draft))
    }

    async fn commit_merge(
        &self,
        expected_revision: u64,
        changes: crate::domain::schedule::ChangeSet,
        meta: ChangeMeta,
        draft_id: &str,
        expected_version: u64,
        note: Option<String>,
    ) -> Result<MergeCommit, StoreError> {
        if changes.is_empty() {
            return Err(StoreError::Constraint("a merge must change rows".into()));
        }
        let mut tables = self.tables();
        if let Some(request_id) = &meta.origin.request_id
            && let Some(earlier) = tables.history.request(&meta.origin.actor, request_id)
        {
            if earlier.digest != meta.request_digest {
                return Err(StoreError::IdempotencyMismatch {
                    seq: earlier.committed.seq,
                });
            }
            return Ok(MergeCommit::Committed(earlier.committed));
        }
        let Some(draft) = tables.drafts.drafts.get(draft_id).cloned() else {
            return Ok(MergeCommit::Stale(DraftStale::Missing));
        };
        if !draft.status.is_live() || draft.version != expected_version {
            return Ok(MergeCommit::Stale(draft_stale_of(&draft)));
        }
        if tables.revision != expected_revision {
            return Err(StoreError::Conflict {
                expected: expected_revision,
                found: tables.revision,
            });
        }
        let keys = touched_keys(&changes);
        let mut next = tables.clone();
        for change in changes.changes {
            apply(&mut next, change)?;
        }
        validate(&next)?;
        next.revision += 1;
        let rows = changed_rows(&keys, |key| tables.value(key), |key| next.value(key));
        let weeks = touched_weeks(&rows, |run_id| {
            next.runs
                .get(run_id)
                .or_else(|| tables.runs.get(run_id))
                .map(|run| run.week_start)
        });
        let request_digest = meta.request_digest.clone();
        let record = ChangeRecord::seal(
            next.history
                .records
                .last()
                .map(|last| (last.seq, last.hash.as_str())),
            uuid::Uuid::new_v4().to_string(),
            next.revision,
            meta.clone(),
            weeks,
            rows,
        )
        .map_err(|error| StoreError::Constraint(error.to_string()))?;
        let committed = Committed {
            seq: record.seq,
            revision: record.revision,
            replayed: false,
        };
        if let Some(digest) = request_digest {
            next.history.digests.insert(record.seq, digest);
        }
        for key in changed_fields(&record) {
            next.history.fields.insert(key, record.seq);
        }
        next.history.records.push(record);
        let mut draft = draft;
        draft.status = crate::domain::drafts::DraftStatus::Merged;
        draft.version += 1;
        draft.merged_seq = Some(committed.seq);
        draft.closed_by = Some(meta.origin.actor.clone());
        draft.updated_at = meta.at;
        next.drafts.drafts.insert(draft.id.clone(), draft.clone());
        draft_event(
            &mut next,
            draft_id,
            draft.version,
            DraftEventKind::Merged,
            &meta.origin.actor,
            meta.at,
            Some(merged_detail(committed.seq, note.as_deref())),
        );
        *tables = next;
        Ok(MergeCommit::Committed(committed))
    }

    async fn expire_drafts(
        &self,
        week: chrono::DateTime<chrono::Utc>,
        at: chrono::DateTime<chrono::Utc>,
        actor: &Actor,
    ) -> Result<Vec<String>, StoreError> {
        let mut tables = self.tables();
        let mut due: Vec<String> = tables
            .drafts
            .drafts
            .values()
            .filter(|draft| {
                draft.status.is_live()
                    && draft
                        .scope
                        .expires_week()
                        .is_some_and(|expires| expires < week)
            })
            .map(|draft| draft.id.clone())
            .collect();
        due.sort();
        let mut next = tables.clone();
        for id in &due {
            let Some(mut draft) = next.drafts.drafts.get(id).cloned() else {
                continue;
            };
            draft.status = crate::domain::drafts::DraftStatus::Expired;
            draft.closed_by = Some(actor.clone());
            draft.updated_at = at;
            next.drafts.drafts.insert(id.clone(), draft.clone());
            draft_event(
                &mut next,
                id,
                draft.version,
                DraftEventKind::Expired,
                actor,
                at,
                None,
            );
        }
        *tables = next;
        Ok(due)
    }
}
