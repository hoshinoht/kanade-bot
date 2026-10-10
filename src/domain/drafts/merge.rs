//! Three-way merge of a draft into the current schedule.
//!
//! B is the schedule the draft was based on, T the current one, D the draft
//! replayed on B and R the draft replayed on T. The draft's changes (B→D)
//! and upstream's (B→T) are compared per merge field; R is the merge result
//! and must hold exactly what the three-way merge expects, field by field.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::conflict::{Entity, Field, FieldValue, MergeConflict, Removal};
use super::op::DraftOp;
use super::replay::{Replay, replay_partial};
use crate::domain::completion::RunEnds;
use crate::domain::history::{ChangeRecord, ChangeRef, HistoryGap, rewind};
use crate::domain::members::Directory;
use crate::domain::schedule::{ChangeSet, Notice, SchedulePolicy, ScheduleSnapshot, utc_instant};
use crate::domain::time::to_iso;

type Fields = BTreeMap<(Entity, Field), FieldValue>;

/// What merging a draft now would do. Nothing is written.
#[derive(Clone, Debug)]
pub struct MergeAnalysis {
    pub conflicts: Vec<MergeConflict>,
    /// The row writes turning the current schedule into the merge result,
    /// with preview ids for created rows; `None` when the replay failed.
    pub result_changes: Option<ChangeSet>,
    /// Notices the merge would post, listed only.
    pub notices_preview: Vec<Notice>,
    /// The schedule after the merge; `None` when the replay failed.
    pub preview: Option<ScheduleSnapshot>,
    /// Boss weeks whose runs the merge changes.
    pub weeks: Vec<DateTime<Utc>>,
}

impl MergeAnalysis {
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty() && self.result_changes.is_some()
    }
}

/// [`analyze_merge`] with the base rewound from `current` through the
/// records after the draft's `base` head, up to a `head` read after
/// `current` was loaded (see [`rewind`]).
///
/// # Errors
/// [`HistoryGap`] when `upstream` is not exactly that chain.
#[allow(clippy::too_many_arguments)]
pub fn analyze_merge_since(
    current: &ScheduleSnapshot,
    base: &ChangeRef,
    head: &ChangeRef,
    upstream: &[ChangeRecord],
    ops: &[DraftOp],
    policy: &SchedulePolicy,
    ends: Option<&Arc<RunEnds>>,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
) -> Result<MergeAnalysis, HistoryGap> {
    let base = rewind(current, base, head, upstream)?;
    Ok(analyze_merge(
        &base, current, ops, policy, ends, directory, now,
    ))
}

/// Analyse merging `ops`, drafted on `base`, into `current`; `ends` as for
/// [`super::replay`].
pub fn analyze_merge(
    base: &ScheduleSnapshot,
    current: &ScheduleSnapshot,
    ops: &[DraftOp],
    policy: &SchedulePolicy,
    ends: Option<&Arc<RunEnds>>,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
) -> MergeAnalysis {
    analyze_merge_applying(
        base,
        current,
        ops,
        policy,
        ends,
        directory,
        now,
        &BTreeSet::new(),
    )
}

/// [`analyze_merge`] where the status of each run in `status_at_apply` is
/// whatever the operations make of the current schedule: it never
/// conflicts with an upstream status change (a proposal's cancel/otot
/// target or its recount, v4 parity). Every other field is checked as usual.
#[allow(clippy::too_many_arguments)]
pub fn analyze_merge_applying(
    base: &ScheduleSnapshot,
    current: &ScheduleSnapshot,
    ops: &[DraftOp],
    policy: &SchedulePolicy,
    ends: Option<&Arc<RunEnds>>,
    directory: &(dyn Directory + Sync),
    now: DateTime<Utc>,
    status_at_apply: &BTreeSet<String>,
) -> MergeAnalysis {
    let (drafted, drafted_rejected) = replay_partial(base, ops, policy, ends, directory, now);
    let (merged, merged_rejected) = replay_partial(current, ops, policy, ends, directory, now);
    let mut analysis = MergeAnalysis {
        conflicts: Vec::new(),
        result_changes: None,
        notices_preview: Vec::new(),
        preview: None,
        weeks: Vec::new(),
    };
    let drafted_names = created_names(&drafted);
    let merged_names = created_names(&merged);
    for (rejected, on_base) in [(&drafted_rejected, true), (&merged_rejected, false)] {
        if let Some(rejected) = rejected {
            analysis.conflicts.push(MergeConflict::OpRejected {
                ord: rejected.ord,
                error: rejected.error.clone(),
                on_base,
            });
        }
    }
    // Per-run choices are compared even when the replay stopped: a stale
    // listing is usually why the edit was refused.
    stale_choices(
        &drafted,
        &merged,
        &drafted_names,
        &merged_names,
        &mut analysis,
    );
    stale_weeks(ops, policy, now, &mut analysis);
    if drafted_rejected.is_some() || merged_rejected.is_some() {
        return analysis;
    }
    let preview = merged.draft.to_snapshot();
    analysis.weeks = changed_weeks(current, &preview);
    analysis.result_changes = Some(merged.draft.clone().into_changes());
    analysis.notices_preview = merged.notices.clone();
    analysis.preview = Some(preview);
    let identity = BTreeMap::new();
    let b = fields(base, &identity);
    let d = fields(&drafted.draft.to_snapshot(), &drafted_names);
    let t = fields(current, &identity);
    let r = fields(analysis.preview.as_ref().unwrap_or(current), &merged_names);
    let applied: BTreeSet<(Entity, Field)> = status_at_apply
        .iter()
        .map(|run| (Entity::Run(run.clone()), Field::Status))
        .collect();
    three_way(&b, &d, &t, &r, &applied, &mut analysis.conflicts);
    analysis
}

/// Canonical names for the rows a replay created (see [`Entity`]).
fn created_names(replay: &Replay) -> BTreeMap<String, String> {
    let snapshot = replay.draft.to_snapshot();
    let mut drawn: BTreeMap<usize, Vec<(u64, String)>> = BTreeMap::new();
    let ids = snapshot
        .fixed_runs
        .iter()
        .map(|row| &row.id)
        .chain(snapshot.runs.iter().map(|row| &row.id));
    for id in ids {
        if let Some((ord, draw)) = replay.ids.owner(id) {
            drawn.entry(ord).or_default().push((draw, id.clone()));
        }
    }
    let mut names = BTreeMap::new();
    for (ord, mut ids) in drawn {
        ids.sort();
        for (n, (_, id)) in ids.into_iter().enumerate() {
            names.insert(id, format!("created:{ord}.{n}"));
        }
    }
    names
}

fn rename(names: &BTreeMap<String, String>, id: &str) -> String {
    names.get(id).cloned().unwrap_or_else(|| id.to_owned())
}

fn stale_choices(
    drafted: &Replay,
    merged: &Replay,
    drafted_names: &BTreeMap<String, String>,
    merged_names: &BTreeMap<String, String>,
    analysis: &mut MergeAnalysis,
) {
    let canonical = |ids: &[String], names: &BTreeMap<String, String>| {
        let set: BTreeSet<String> = ids.iter().map(|id| rename(names, id)).collect();
        set.into_iter().collect::<Vec<_>>()
    };
    for (ord, (on_base, on_current)) in drafted.amended.iter().zip(&merged.amended).enumerate() {
        if let (Some(on_base), Some(on_current)) = (on_base, on_current) {
            let base = canonical(on_base, drafted_names);
            let current = canonical(on_current, merged_names);
            if base != current {
                analysis
                    .conflicts
                    .push(MergeConflict::ChoicesStale { ord, base, current });
            }
        }
    }
}

/// A retirement's staged boss weeks must be the ones materialised now: a
/// reset since staging leaves a newly materialised week's run live.
fn stale_weeks(
    ops: &[DraftOp],
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
    analysis: &mut MergeAnalysis,
) {
    // Unrepresentable weeks match nothing, so every retirement is stale.
    let current: Vec<DateTime<Utc>> = policy
        .materialised_weeks(now)
        .ok()
        .and_then(|weeks| weeks.iter().map(utc_instant).collect::<Result<_, _>>().ok())
        .unwrap_or_default();
    let current_set: BTreeSet<&DateTime<Utc>> = current.iter().collect();
    for (ord, op) in ops.iter().enumerate() {
        if let DraftOp::RetireFixedRun { weeks, .. } = op {
            let staged: BTreeSet<&DateTime<Utc>> = weeks.iter().collect();
            if staged != current_set {
                analysis.conflicts.push(MergeConflict::StaleWeeks {
                    ord,
                    staged: staged.into_iter().copied().collect(),
                    current: current.clone(),
                });
            }
        }
    }
}

fn text(value: impl Into<String>) -> FieldValue {
    FieldValue::Text(Some(value.into()))
}

fn set(values: &[String]) -> FieldValue {
    let set: BTreeSet<&String> = values.iter().collect();
    FieldValue::Set(set.into_iter().cloned().collect())
}

fn instant(at: DateTime<Utc>) -> FieldValue {
    // Out-of-range instants cannot come from a store; compare them raw.
    text(to_iso(&at).unwrap_or_else(|_| format!("{at:?}")))
}

/// The merge fields of every entity in `snapshot`, created rows renamed.
fn fields(snapshot: &ScheduleSnapshot, names: &BTreeMap<String, String>) -> Fields {
    let mut out = Fields::new();
    for row in &snapshot.fixed_runs {
        let entity = Entity::Fixed(rename(names, &row.id));
        let values = [
            (
                Field::DayTime,
                text(format!("{} {}", row.weekday, row.time)),
            ),
            (
                Field::Bosses,
                FieldValue::Text(Some(row.bosses.join("\u{1f}"))),
            ),
            (Field::Participants, set(&row.participants)),
            (Field::Channel, FieldValue::Text(row.channel_id.clone())),
            (Field::Note, FieldValue::Text(row.note.clone())),
            // The stored owner and its pin; the effective owner would also
            // move with the party, which is the participants field's change.
            (
                Field::Owner,
                text(format!(
                    "{}{}",
                    row.owner_id,
                    if row.owner_pinned { "\u{1f}pinned" } else { "" }
                )),
            ),
        ];
        for (field, value) in values {
            out.insert((entity.clone(), field), value);
        }
    }
    for row in &snapshot.runs {
        let entity = Entity::Run(rename(names, &row.id));
        let values = [
            (Field::Slot, instant(row.datetime)),
            (
                Field::Bosses,
                FieldValue::Text(Some(row.bosses.join("\u{1f}"))),
            ),
            (Field::Participants, set(&row.participants)),
            (Field::Channel, FieldValue::Text(row.channel_id.clone())),
            (Field::Status, text(row.status.as_str())),
            (
                Field::FixedRun,
                FieldValue::Text(row.fixed_run_id.as_deref().map(|id| rename(names, id))),
            ),
        ];
        for (field, value) in values {
            out.insert((entity.clone(), field), value);
        }
    }
    for row in &snapshot.rsvps {
        let entity = Entity::Rsvp {
            run_id: rename(names, &row.run_id),
            user_id: row.user_id.clone(),
        };
        let answer = format!("{}/{}", row.state.as_str(), row.source.as_str());
        out.insert((entity, Field::Answer), text(answer));
    }
    out
}

fn exists(fields: &Fields, entity: &Entity) -> bool {
    fields
        .range((entity.clone(), Field::Slot)..)
        .next()
        .is_some_and(|((found, _), _)| found == entity)
}

fn is_status(fields: &Fields, run: &Entity, statuses: &[&str]) -> Option<String> {
    match fields.get(&(run.clone(), Field::Status)) {
        Some(FieldValue::Text(Some(status))) if statuses.contains(&status.as_str()) => {
            Some(status.clone())
        }
        _ => None,
    }
}

/// How upstream took `entity` away, if it did.
fn removal(b: &Fields, t: &Fields, entity: &Entity) -> Option<Removal> {
    if exists(b, entity) && !exists(t, entity) {
        return Some(match entity {
            Entity::Fixed(_) => Removal::Retired,
            _ => Removal::Deleted,
        });
    }
    match entity {
        Entity::Run(_) => {
            let terminal = ["cancelled", "done"];
            if is_status(b, entity, &terminal).is_some() {
                return None;
            }
            match is_status(t, entity, &terminal)?.as_str() {
                "done" => Some(Removal::Done),
                _ => Some(Removal::Cancelled),
            }
        }
        Entity::Rsvp { run_id, .. } => removal(b, t, &Entity::Run(run_id.clone())),
        Entity::Fixed(_) => None,
    }
}

/// The timing a drafted run names when upstream retired it.
fn retired_timing(b: &Fields, d: &Fields, t: &Fields, entity: &Entity) -> Option<Entity> {
    let key = (entity.clone(), Field::FixedRun);
    if !matches!(entity, Entity::Run(_)) || b.get(&key) == d.get(&key) {
        return None;
    }
    let Some(FieldValue::Text(Some(id))) = d.get(&key) else {
        return None;
    };
    let fixed = Entity::Fixed(id.clone());
    (exists(b, &fixed) && !exists(t, &fixed)).then_some(fixed)
}

/// A drafted answer whose member was on the run in the draft but is not on
/// the merged run (upstream took them off).
fn left_run(d: &Fields, r: &Fields, entity: &Entity) -> bool {
    let Entity::Rsvp { run_id, user_id } = entity else {
        return false;
    };
    if !d.contains_key(&(entity.clone(), Field::Answer)) {
        return false;
    }
    let on = |fields: &Fields| match fields.get(&(Entity::Run(run_id.clone()), Field::Participants))
    {
        Some(FieldValue::Set(members)) => Some(members.contains(user_id)),
        _ => None,
    };
    on(d) == Some(true) && on(r) == Some(false)
}

fn merge_sets(
    b: Option<&FieldValue>,
    d: Option<&FieldValue>,
    t: Option<&FieldValue>,
) -> Option<FieldValue> {
    let (Some(FieldValue::Set(b)), Some(FieldValue::Set(d)), Some(FieldValue::Set(t))) = (b, d, t)
    else {
        return None;
    };
    let b: BTreeSet<&String> = b.iter().collect();
    let d: BTreeSet<&String> = d.iter().collect();
    let t: BTreeSet<&String> = t.iter().collect();
    let merged: BTreeSet<&String> = b
        .iter()
        .filter(|member| d.contains(*member) && t.contains(*member))
        .copied()
        .chain(d.difference(&b).copied())
        .chain(t.difference(&b).copied())
        .collect();
    Some(FieldValue::Set(merged.into_iter().cloned().collect()))
}

/// `applied` keys take the merge result's value as it stands (see
/// [`analyze_merge_applying`]); an upstream removal of their entity still
/// conflicts.
fn three_way(
    b: &Fields,
    d: &Fields,
    t: &Fields,
    r: &Fields,
    applied: &BTreeSet<(Entity, Field)>,
    conflicts: &mut Vec<MergeConflict>,
) {
    let keys: BTreeSet<&(Entity, Field)> = b.keys().chain(d.keys()).chain(t.keys()).collect();
    let drafted: Vec<&(Entity, Field)> = keys
        .into_iter()
        .filter(|key| b.get(*key) != d.get(*key))
        .collect();
    let mut expected: BTreeMap<(Entity, Field), Option<FieldValue>> = BTreeMap::new();
    let mut skip_entities: BTreeSet<Entity> = BTreeSet::new();
    let mut skip_keys: BTreeSet<(Entity, Field)> = BTreeSet::new();
    let mut retired: BTreeSet<Entity> = BTreeSet::new();

    let touched: BTreeSet<&Entity> = drafted.iter().map(|(entity, _)| entity).collect();
    for entity in touched {
        let converged = drafted
            .iter()
            .filter(|(e, _)| e == entity)
            .all(|key| d.get(*key) == t.get(*key));
        if converged {
            continue;
        }
        if let Some(removal) = removal(b, t, entity) {
            conflicts.push(MergeConflict::UpstreamRemoved {
                entity: entity.clone(),
                removal,
            });
            skip_entities.insert(entity.clone());
        } else if let Some(fixed) = retired_timing(b, d, t, entity) {
            // A run the draft attaches to a timing upstream retired.
            if retired.insert(fixed.clone()) {
                conflicts.push(MergeConflict::UpstreamRemoved {
                    entity: fixed,
                    removal: Removal::Retired,
                });
            }
            skip_entities.insert(entity.clone());
        } else if left_run(d, r, entity) {
            conflicts.push(MergeConflict::UpstreamRemoved {
                entity: entity.clone(),
                removal: Removal::LeftRun,
            });
            skip_entities.insert(entity.clone());
        }
    }
    for key in drafted {
        if skip_entities.contains(&key.0) || applied.contains(key) {
            continue;
        }
        let (bv, dv, tv) = (b.get(key), d.get(key), t.get(key));
        let value = if tv == bv || tv == dv {
            dv.cloned()
        } else if let Some(merged) = (key.1 == Field::Participants)
            .then(|| merge_sets(bv, dv, tv))
            .flatten()
        {
            Some(merged)
        } else {
            conflicts.push(MergeConflict::BothChanged {
                entity: key.0.clone(),
                field: key.1,
                base: bv.cloned(),
                draft: dv.cloned(),
                upstream: tv.cloned(),
            });
            skip_keys.insert(key.clone());
            continue;
        };
        expected.insert(key.clone(), value);
    }

    let all: BTreeSet<&(Entity, Field)> = t.keys().chain(r.keys()).chain(expected.keys()).collect();
    for key in all {
        if skip_entities.contains(&key.0) || skip_keys.contains(key) || applied.contains(key) {
            continue;
        }
        let want = match expected.get(key) {
            Some(value) => value.as_ref(),
            None => t.get(key),
        };
        if r.get(key) != want {
            conflicts.push(MergeConflict::Divergent {
                entity: key.0.clone(),
                field: key.1,
            });
        }
    }
}

/// Whether two replays of the same operations hold the same merge fields:
/// the preview (preview ids) against the merge commit's real-id replay.
/// Created rows compare by their canonical `created:<ord>.<n>` names, so
/// only the schedule content is compared, never the ids themselves.
pub fn replay_equivalent(first: &Replay, second: &Replay) -> Vec<MergeConflict> {
    let a_names = created_names(first);
    let b_names = created_names(second);
    let a = fields(&first.draft.to_snapshot(), &a_names);
    let b = fields(&second.draft.to_snapshot(), &b_names);
    let keys: BTreeSet<&(Entity, Field)> = a.keys().chain(b.keys()).collect();
    keys.into_iter()
        .filter(|key| a.get(*key) != b.get(*key))
        .map(|(entity, field)| MergeConflict::Divergent {
            entity: entity.clone(),
            field: *field,
        })
        .collect()
}

/// Boss weeks of runs that differ between two snapshots.
fn changed_weeks(before: &ScheduleSnapshot, after: &ScheduleSnapshot) -> Vec<DateTime<Utc>> {
    let old: BTreeMap<&str, _> = before
        .runs
        .iter()
        .map(|run| (run.id.as_str(), run))
        .collect();
    let new: BTreeMap<&str, _> = after
        .runs
        .iter()
        .map(|run| (run.id.as_str(), run))
        .collect();
    let mut weeks = BTreeSet::new();
    for (id, run) in &new {
        if old.get(id) != Some(run) {
            weeks.insert(run.week_start);
        }
    }
    for (id, run) in &old {
        if new.get(id) != Some(run) {
            weeks.insert(run.week_start);
        }
    }
    let rsvp_runs = |snapshot: &ScheduleSnapshot| -> BTreeSet<(String, String, String)> {
        snapshot
            .rsvps
            .iter()
            .map(|row| {
                (
                    row.run_id.clone(),
                    row.user_id.clone(),
                    format!("{}/{}", row.state.as_str(), row.source.as_str()),
                )
            })
            .collect()
    };
    for (run_id, _, _) in rsvp_runs(before).symmetric_difference(&rsvp_runs(after)) {
        if let Some(run) = new.get(run_id.as_str()).or(old.get(run_id.as_str())) {
            weeks.insert(run.week_start);
        }
    }
    weeks.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(value: &str) -> FieldValue {
        FieldValue::Text(Some(value.to_owned()))
    }

    /// An applied-at-merge status takes the merge result over an upstream
    /// change; the same change on any other key still conflicts.
    #[test]
    fn an_applied_status_never_conflicts_but_other_fields_still_do() {
        let run = Entity::Run("r-1".into());
        let status = (run.clone(), Field::Status);
        let slot = (run.clone(), Field::Slot);
        let b = Fields::from([
            (status.clone(), text_of("planned")),
            (slot.clone(), text_of("mon")),
        ]);
        let mut d = b.clone();
        d.insert(status.clone(), text_of("cancelled"));
        d.insert(slot.clone(), text_of("tue"));
        let mut t = b.clone();
        t.insert(status.clone(), text_of("at_risk"));
        t.insert(slot.clone(), text_of("wed"));
        let mut r = t.clone();
        r.insert(status.clone(), text_of("cancelled"));
        let mut conflicts = Vec::new();
        three_way(
            &b,
            &d,
            &t,
            &r,
            &BTreeSet::from([status.clone()]),
            &mut conflicts,
        );
        assert!(
            matches!(
                conflicts.as_slice(),
                [MergeConflict::BothChanged {
                    field: Field::Slot,
                    ..
                }]
            ),
            "{conflicts:?}"
        );
        let mut conflicts = Vec::new();
        three_way(&b, &d, &t, &r, &BTreeSet::new(), &mut conflicts);
        assert!(conflicts.iter().any(|conflict| matches!(
            conflict,
            MergeConflict::BothChanged {
                field: Field::Status,
                ..
            }
        )));
    }

    /// Replay normally refuses a run on a missing timing first; the field
    /// check still names the retired timing if a run reaches it another way.
    #[test]
    fn a_drafted_run_on_a_timing_upstream_retired_is_upstream_removed() {
        let fixed = Entity::Fixed("f-1".into());
        let run = Entity::Run("created:0.0".into());
        let b = Fields::from([((fixed.clone(), Field::Note), FieldValue::Text(None))]);
        let mut d = b.clone();
        d.insert((run.clone(), Field::FixedRun), text_of("f-1"));
        d.insert((run.clone(), Field::Status), text_of("planned"));
        let t = Fields::new();
        let mut r = d.clone();
        r.remove(&(fixed.clone(), Field::Note));
        let mut conflicts = Vec::new();
        three_way(&b, &d, &t, &r, &BTreeSet::new(), &mut conflicts);
        assert_eq!(
            conflicts,
            [MergeConflict::UpstreamRemoved {
                entity: fixed,
                removal: Removal::Retired,
            }]
        );
    }
}
