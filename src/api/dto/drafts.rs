//! `drafts.json`: the signed-in admin's drafts, their staged operations and
//! the merge preview (History diff rows plus every conflict with the
//! History record behind it). Nothing here is live until a merge.

use serde::Serialize;

use super::{
    consequence::consequence,
    history::{self as diff, RowChange},
    inbox::slot,
    inbox_past::{Decider, decider},
    iso_date, iso_instant,
    week::Context,
    when,
};
use crate::domain::{
    drafts::{
        DraftOp, DraftScope, DraftStatus, Entity, FieldValue, LoadedDraft, MergeConflict, Removal,
        StagedOp, Target,
    },
    history::{ChangeRecord, RecordError, RowKey, changed_rows},
    ids::short_id,
    schedule::{Draft, RunStatus, ScheduleSnapshot},
    scheduler::DraftPreview,
};

/// The stored title of a draft created without one: the list then shows a
/// title derived from its operations.
pub const UNTITLED: &str = "Untitled draft";

/// `GET /api/admin/drafts?state=open|closed`: the admin's own drafts, most
/// recently changed first, and the current history head.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DraftList {
    pub head: u64,
    pub drafts: Vec<DraftRow>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DraftRow {
    pub id: String,
    pub short_id: String,
    /// The admin's title, or one derived from the operations (`titled`
    /// false).
    pub title: String,
    pub titled: bool,
    #[cfg_attr(test, ts(type = "'open' | 'merged' | 'discarded' | 'expired'"))]
    pub status: &'static str,
    /// The precondition every write sends back.
    pub version: u64,
    /// The history record the draft is based on.
    pub base: u64,
    /// Based on the current head ("Up to date"); else "Base moved".
    pub up_to_date: bool,
    #[cfg_attr(test, ts(type = "'week' | 'weekly'"))]
    pub scope: &'static str,
    /// The boss week's first day (guild date) of a week-scoped draft.
    pub week: Option<String>,
    pub staged: u32,
    pub author: Decider,
    pub created_at: String,
    pub updated_at: String,
    pub merged_seq: Option<u64>,
    pub closed_by: Option<Decider>,
    pub close_reason: Option<String>,
}

/// One staged operation, in order. `run_id`/`fixed_id` name an existing
/// row; `created_by` the earlier operation that creates the row it acts on.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct StagedOpRow {
    pub ord: u32,
    /// The operation's codec kind (`amend_run`, `set_status`, ...).
    pub kind: &'static str,
    pub summary: String,
    pub run_id: Option<String>,
    pub fixed_id: Option<String>,
    pub created_by: Option<u32>,
    pub author: Decider,
    pub added_at: String,
}

/// `GET /api/admin/drafts/{id}`.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DraftDetail {
    #[serde(flatten)]
    pub draft: DraftRow,
    pub ops: Vec<StagedOpRow>,
}

/// The History record behind a conflict: the latest change after the
/// draft's base that touched the conflicting row.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ConflictRecord {
    pub seq: u64,
    pub actor: Decider,
    pub at: String,
}

/// One conflict the merge analysis reports, with the operations involved
/// and the answers a rebase accepts for it (`keep` keeps the operations,
/// reviving a cancelled or finished run; `take` drops them).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DraftConflict {
    /// What a rebase names it by.
    pub key: String,
    #[cfg_attr(
        test,
        ts(
            type = "'both_changed' | 'upstream_removed' | 'op_rejected' | 'choices_stale' | 'stale_weeks' | 'divergent'"
        )
    )]
    pub kind: &'static str,
    pub run_id: Option<String>,
    pub fixed_id: Option<String>,
    pub field: Option<&'static str>,
    /// Both changed: the value drafted on, the draft's and today's.
    pub base: Option<String>,
    pub yours: Option<String>,
    pub now: Option<String>,
    #[cfg_attr(
        test,
        ts(type = "'deleted' | 'retired' | 'cancelled' | 'done' | 'left_run' | null")
    )]
    pub removal: Option<&'static str>,
    pub message: String,
    pub ops: Vec<u32>,
    #[cfg_attr(test, ts(type = "Array<'keep' | 'take'>"))]
    pub settle: Vec<&'static str>,
    pub record: Option<ConflictRecord>,
}

/// `GET /api/admin/drafts/{id}/preview`: the rows a merge would write
/// against the current schedule (History's diff rows), and the conflicts.
/// Merge is offered only when `mergeable` (on the head and conflict-free).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DraftPreviewView {
    pub id: String,
    pub version: u64,
    pub base: u64,
    pub head: u64,
    pub up_to_date: bool,
    pub mergeable: bool,
    pub rows: Vec<RowChange>,
    pub conflicts: Vec<DraftConflict>,
    /// One line on what merging does; `None` with conflicts or nothing to say.
    pub consequence: Option<String>,
}

/// Every draft write's answer: the draft as it now stands (re-read it for
/// the rest) and, for a merge, its History record.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DraftWritten {
    pub id: String,
    pub version: u64,
    #[cfg_attr(test, ts(type = "'open' | 'merged' | 'discarded' | 'expired'"))]
    pub status: &'static str,
    pub seq: Option<u64>,
    pub message: String,
}

fn status(status: DraftStatus) -> &'static str {
    match status {
        DraftStatus::Merged => "merged",
        DraftStatus::Expired => "expired",
        DraftStatus::Open | DraftStatus::Submitted => "open",
        DraftStatus::Discarded | DraftStatus::Rejected | DraftStatus::Withdrawn => "discarded",
    }
}

fn names(ctx: &Context<'_>, ids: &[String], sign: char) -> Vec<String> {
    ids.iter()
        .map(|id| format!("{sign}{}", ctx.name(id)))
        .collect()
}

fn run_label(ctx: &Context<'_>, current: &ScheduleSnapshot, target: &Target) -> String {
    match target {
        Target::Existing(id) => current.runs.iter().find(|run| &run.id == id).map_or_else(
            || format!("run #{}", short_id(id)),
            |run| format!("{} {}", run.bosses.join(" "), when(run.datetime, ctx.zone)),
        ),
        Target::Created(ord) => format!("the run change {} adds", ord + 1),
    }
}

fn fixed_label(current: &ScheduleSnapshot, target: &Target) -> String {
    match target {
        Target::Existing(id) => current
            .fixed_runs
            .iter()
            .find(|row| &row.id == id)
            .map_or_else(
                || format!("weekly #{}", short_id(id)),
                |row| {
                    format!(
                        "weekly {} {}",
                        row.bosses.join(" "),
                        slot(row.weekday, row.time)
                    )
                },
            ),
        Target::Created(ord) => format!("the weekly timing change {} adds", ord + 1),
    }
}

/// One plain line per operation, read against the current schedule.
pub fn summary(ctx: &Context<'_>, current: &ScheduleSnapshot, op: &DraftOp) -> String {
    let run = |target: &Target| run_label(ctx, current, target);
    let fixed = |target: &Target| fixed_label(current, target);
    let party = |add: &[String], remove: &[String]| {
        let mut changes = names(ctx, add, '+');
        changes.extend(names(ctx, remove, '−'));
        changes.join(", ")
    };
    match op {
        DraftOp::AmendRun { run: target, to } => {
            format!("Move {} to {}", run(target), when(*to, ctx.zone))
        }
        DraftOp::SetStatus {
            run: target,
            change,
        } => match change.status {
            RunStatus::Cancelled => format!("Cancel {}", run(target)),
            status => format!("Set {} to {}", run(target), status.as_str()),
        },
        DraftOp::SwapParticipants {
            run: target,
            add,
            remove,
            ..
        } => format!("Party of {}: {}", run(target), party(add, remove)),
        DraftOp::ResetToFixed { run: target } => {
            format!("Reset {} to its weekly timing", run(target))
        }
        DraftOp::CreateRun {
            datetime, bosses, ..
        } => format!("Add run {} {}", bosses.join(" "), when(*datetime, ctx.zone)),
        DraftOp::FixedParticipants {
            fixed: target,
            add,
            remove,
        } => format!("Party of {}: {}", fixed(target), party(add, remove)),
        DraftOp::ApplyFixedEdit {
            fixed: target,
            edit,
            ..
        } => {
            let mut parts = Vec::new();
            if let (Some(day), Some(time)) = (edit.weekday, edit.time) {
                parts.push(format!("to {}", slot(day, time)));
            }
            if let Some(party) = &edit.participants {
                let named: Vec<String> = party.iter().map(|id| ctx.name(id)).collect();
                parts.push(format!("party {}", named.join(", ")));
            }
            if let Some(channel) = &edit.channel_id {
                parts.push(format!("channel {}", ctx.channel_name(channel)));
            }
            format!("Change {} {}", fixed(target), parts.join(", "))
                .trim_end()
                .to_owned()
        }
        DraftOp::RetireFixedRun { fixed: target, .. } => format!("Retire {}", fixed(target)),
        DraftOp::AddFixedRun(new) => format!(
            "Add weekly {} {}",
            new.bosses.join(" "),
            slot(new.weekday, new.time)
        ),
        DraftOp::SetRsvp {
            run: target,
            user_id,
            state,
            ..
        } => format!(
            "{} answers {} for {}",
            ctx.name(user_id),
            state.as_str(),
            run(target)
        ),
        other => other.kind().to_owned(),
    }
}

/// The title a draft shows: the admin's, else derived from its operations.
fn title(ctx: &Context<'_>, current: &ScheduleSnapshot, loaded: &LoadedDraft) -> (String, bool) {
    if loaded.draft.title != UNTITLED {
        return (loaded.draft.title.clone(), true);
    }
    let derived = match loaded.ops.as_slice() {
        [] => UNTITLED.to_owned(),
        [only] => summary(ctx, current, &only.op),
        [first, rest @ ..] => format!(
            "{} and {} more",
            summary(ctx, current, &first.op),
            rest.len()
        ),
    };
    (derived, false)
}

pub fn row(
    ctx: &Context<'_>,
    current: &ScheduleSnapshot,
    loaded: &LoadedDraft,
    head: u64,
) -> DraftRow {
    let draft = &loaded.draft;
    let (title, titled) = title(ctx, current, loaded);
    let (scope, week) = match draft.scope {
        DraftScope::Week(start) => ("week", Some(iso_date(ctx.local_date(start)))),
        DraftScope::Weekly => ("weekly", None),
    };
    DraftRow {
        id: draft.id.clone(),
        short_id: short_id(&draft.id),
        title,
        titled,
        status: status(draft.status),
        version: draft.version,
        base: draft.base.seq,
        up_to_date: draft.base.seq == head,
        scope,
        week,
        staged: u32::try_from(loaded.ops.len()).unwrap_or(u32::MAX),
        author: decider(ctx, &draft.author),
        created_at: iso_instant(draft.created_at),
        updated_at: iso_instant(draft.updated_at),
        merged_seq: draft.merged_seq,
        closed_by: draft
            .closed_by
            .as_ref()
            .filter(|_| !draft.status.is_live())
            .map(|actor| decider(ctx, actor)),
        close_reason: draft.close_reason.clone(),
    }
}

fn ord(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn staged_row(ctx: &Context<'_>, current: &ScheduleSnapshot, staged: &StagedOp) -> StagedOpRow {
    let mut probe = staged.op.clone();
    let (mut run_id, mut fixed_id, mut created_by) = (None, None, None);
    let runs_on_run = !matches!(
        staged.op,
        DraftOp::ApplyFixedEdit { .. }
            | DraftOp::FixedParticipants { .. }
            | DraftOp::RetireFixedRun { .. }
            | DraftOp::CreateRun { .. }
    );
    probe.each_target_mut(|target| match target {
        Target::Existing(id) if runs_on_run => run_id = Some(id.clone()),
        Target::Existing(id) => fixed_id = Some(id.clone()),
        Target::Created(creator) => created_by = Some(ord(*creator)),
    });
    StagedOpRow {
        ord: ord(staged.ord),
        kind: staged.op.kind(),
        summary: summary(ctx, current, &staged.op),
        run_id,
        fixed_id,
        created_by,
        author: decider(ctx, &staged.author),
        added_at: iso_instant(staged.added_at),
    }
}

pub fn detail(
    ctx: &Context<'_>,
    current: &ScheduleSnapshot,
    loaded: &LoadedDraft,
    head: u64,
) -> DraftDetail {
    DraftDetail {
        draft: row(ctx, current, loaded, head),
        ops: loaded
            .ops
            .iter()
            .map(|staged| staged_row(ctx, current, staged))
            .collect(),
    }
}

fn value(value: Option<&FieldValue>) -> String {
    match value {
        None | Some(FieldValue::Text(None)) => "—".into(),
        Some(FieldValue::Text(Some(text))) => text.clone(),
        Some(FieldValue::Set(items)) if items.is_empty() => "—".into(),
        Some(FieldValue::Set(items)) => items.join(", "),
    }
}

fn removal(removal: Removal) -> &'static str {
    match removal {
        Removal::Deleted => "deleted",
        Removal::Retired => "retired",
        Removal::Cancelled => "cancelled",
        Removal::Done => "done",
        Removal::LeftRun => "left_run",
    }
}

fn key_of(entity: &Entity) -> RowKey {
    match entity {
        Entity::Fixed(id) => RowKey::FixedRun(id.clone()),
        Entity::Run(id) => RowKey::Run(id.clone()),
        Entity::Rsvp { run_id, user_id } => RowKey::Rsvp {
            run_id: run_id.clone(),
            user_id: user_id.clone(),
        },
    }
}

fn ids_of(entity: &Entity) -> (Option<String>, Option<String>) {
    match entity {
        Entity::Fixed(id) => (None, Some(id.clone())),
        Entity::Run(id) | Entity::Rsvp { run_id: id, .. } => (Some(id.clone()), None),
    }
}

/// The latest record after the base that touched `entity`'s row.
fn record_of(
    ctx: &Context<'_>,
    upstream: &[ChangeRecord],
    entity: &Entity,
) -> Option<ConflictRecord> {
    let key = key_of(entity);
    upstream
        .iter()
        .rev()
        .find(|record| record.rows.iter().any(|row| row.key == key))
        .map(|record| ConflictRecord {
            seq: record.seq,
            actor: decider(ctx, &record.origin.actor),
            at: iso_instant(record.at),
        })
}

fn conflict(
    ctx: &Context<'_>,
    upstream: &[ChangeRecord],
    conflict: &MergeConflict,
    involved: &[usize],
) -> DraftConflict {
    let mut out = DraftConflict {
        key: conflict.key(),
        kind: "",
        run_id: None,
        fixed_id: None,
        field: None,
        base: None,
        yours: None,
        now: None,
        removal: None,
        message: String::new(),
        ops: involved.iter().copied().map(ord).collect(),
        settle: vec!["take"],
        record: None,
    };
    let mut about = |entity: &Entity| {
        (out.run_id, out.fixed_id) = ids_of(entity);
        out.record = record_of(ctx, upstream, entity);
    };
    match conflict {
        MergeConflict::BothChanged {
            entity,
            field,
            base,
            draft,
            upstream: now,
        } => {
            about(entity);
            out.kind = "both_changed";
            out.field = Some(field.as_str());
            out.base = Some(value(base.as_ref()));
            out.yours = Some(value(draft.as_ref()));
            out.now = Some(value(now.as_ref()));
            out.message = format!("{entity} {}: changed here and in History", field.as_str());
            out.settle = vec!["keep", "take"];
        }
        MergeConflict::UpstreamRemoved {
            entity,
            removal: how,
        } => {
            about(entity);
            out.kind = "upstream_removed";
            out.removal = Some(removal(*how));
            out.message = format!("{entity} was {} since", removal(*how).replace('_', " "));
            if matches!(entity, Entity::Run(_)) && matches!(how, Removal::Cancelled | Removal::Done)
            {
                out.settle = vec!["keep", "take"];
            }
        }
        MergeConflict::OpRejected { ord: at, error, .. } => {
            out.kind = "op_rejected";
            out.message = format!("Change {} no longer applies: {error}", at + 1);
        }
        MergeConflict::ChoicesStale { ord: at, .. } => {
            out.kind = "choices_stale";
            out.message = format!(
                "Change {}'s per-run choices were made for other runs",
                at + 1
            );
        }
        MergeConflict::StaleWeeks { ord: at, .. } => {
            out.kind = "stale_weeks";
            out.message = format!("Change {} was staged for earlier boss weeks", at + 1);
        }
        MergeConflict::Divergent { entity, field } => {
            about(entity);
            out.kind = "divergent";
            out.field = Some(field.as_str());
            out.message = format!("{entity} {} would not merge as drafted", field.as_str());
            out.settle = vec!["keep", "take"];
        }
    }
    out
}

/// # Errors
/// [`RecordError`] for a row instant outside the representable years.
pub fn preview(ctx: &Context<'_>, preview: &DraftPreview) -> Result<DraftPreviewView, RecordError> {
    let draft = &preview.draft.draft;
    let analysis = &preview.analysis;
    let rows = match (&analysis.preview, &analysis.result_changes) {
        (Some(after), Some(changes)) => changed_rows(
            &Draft::new(preview.current.clone()),
            &Draft::new(after.clone()),
            changes,
        )
        .iter()
        .map(row_change)
        .collect::<Result<Vec<_>, _>>()?,
        _ => Vec::new(),
    };
    let conflicts: Vec<DraftConflict> = analysis
        .conflicts
        .iter()
        .zip(&preview.involved)
        .map(|(found, involved)| conflict(ctx, &preview.upstream, found, involved))
        .collect();
    let up_to_date = draft.base == preview.head;
    let clean = conflicts.is_empty() && analysis.result_changes.is_some();
    Ok(DraftPreviewView {
        id: draft.id.clone(),
        version: draft.version,
        base: draft.base.seq,
        head: preview.head.seq,
        up_to_date,
        mergeable: up_to_date && clean && !rows.is_empty() && draft.status.is_live(),
        consequence: clean
            .then(|| consequence(ctx, &preview.current, analysis.preview.as_ref()?))
            .flatten(),
        rows,
        conflicts,
    })
}

fn row_change(change: &crate::domain::history::RowChange) -> Result<RowChange, RecordError> {
    diff::row(change)
}

/// A write's answer for `draft` as the write left it.
pub fn written(
    draft: &crate::domain::drafts::StoredDraft,
    seq: Option<u64>,
    message: String,
) -> DraftWritten {
    DraftWritten {
        id: draft.id.clone(),
        version: draft.version,
        status: status(draft.status),
        seq,
        message,
    }
}
