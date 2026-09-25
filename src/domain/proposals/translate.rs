//! A proposed change → the draft operations its approval merges, checked on
//! the current schedule with v4 `commit.py`'s rules and refusal texts.
//!
//! v4 applied a change at ✅ and refused it then; v5 refuses it up front
//! (user decision, `D-PROPOSE-REFUSES`), so a proposal only ever holds
//! operations that applied when it was made.

use chrono::{DateTime, Utc};

use super::change::{ChangeKind, Payload, ProposalSubject, ProposedChange};
use super::refusal::Refusal;
use crate::domain::drafts::{DraftOp, Target};
use crate::domain::schedule::{
    FixedEdit, FixedEditChoices, NewFixedRun, RsvpSource, Run, RunSource, RunStatus,
    SchedulePolicy, ScheduleSnapshot, StatusChange, utc_instant,
};

/// The note v4 gave a weekly timing created from chat.
pub const CREATED_FROM_CHAT: &str = "created from chat";

/// The operations to stage and the facts stored beside them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Translation {
    pub ops: Vec<DraftOp>,
    pub subject: ProposalSubject,
}

/// Translate `change` on `snapshot`.
///
/// Who approves is unknown until ✅: a new run or timing that names nobody
/// is staged with no participants (and a timing with no owner), filled in
/// with the approver by [`fill_approver`], as v4 used the confirming member.
///
/// # Errors
/// The [`Refusal`] v4 `commit` would have reported for this change.
pub fn translate(
    change: &ProposedChange,
    snapshot: &ScheduleSnapshot,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<Translation, Refusal> {
    let run = change
        .run_id
        .as_deref()
        .and_then(|id| snapshot.runs.iter().find(|run| run.id == id));
    let mut subject = ProposalSubject {
        kind: change.kind,
        run_id: change.run_id.clone(),
        fixed_run_id: None,
        channel_id: change.channel_id.clone(),
        bosses: change.bosses.clone(),
        named: change.participants.clone(),
    };
    let ops = match change.kind {
        ChangeKind::Move => move_run(change, run.ok_or(Refusal::RunGone)?, snapshot, policy)?,
        ChangeKind::Add => add(change, policy)?,
        ChangeKind::Cancel => status(run, RunStatus::Cancelled)?,
        ChangeKind::Otot => status(run, RunStatus::Otot)?,
        ChangeKind::Sub => sub(change, run.ok_or(Refusal::RunGone)?)?,
        ChangeKind::Split => split(change, run.ok_or(Refusal::RunGone)?, snapshot, policy)?,
        ChangeKind::Rsvp => rsvp(change, run.ok_or(Refusal::RunGone)?)?,
        ChangeKind::Fix => {
            let (mut ops, fixed) = fix(change, snapshot)?;
            retire_weeks(&mut ops, policy, now)?;
            subject.fixed_run_id = fixed;
            ops
        }
    };
    Ok(Translation { ops, subject })
}

fn week_of(at: &DateTime<Utc>, policy: &SchedulePolicy) -> Result<DateTime<Utc>, Refusal> {
    policy
        .week_of(at)
        .and_then(|start| utc_instant(&start))
        .map_err(|error| Refusal::Rule(error.to_string()))
}

fn move_run(
    change: &ProposedChange,
    run: &Run,
    snapshot: &ScheduleSnapshot,
    policy: &SchedulePolicy,
) -> Result<Vec<DraftOp>, Refusal> {
    let to = change.new_datetime.ok_or(Refusal::NoNewTime)?;
    let week = week_of(&to, policy)?;
    // v4 `run_move_conflict`, checked here for v4's wording.
    if let Some(fixed) = &run.fixed_run_id
        && snapshot.runs.iter().any(|other| {
            other.id != run.id
                && other.fixed_run_id.as_ref() == Some(fixed)
                && other.week_start == week
        })
    {
        return Err(Refusal::WeeklyHoldsWeek);
    }
    let target = Target::Existing(run.id.clone());
    let mut ops = Vec::new();
    // User decision: v4 `_move` revives a cancelled or otot run; the move
    // then re-derives it (and ends any pin) as every move does.
    if matches!(run.status, RunStatus::Cancelled | RunStatus::Otot) {
        ops.push(DraftOp::ReviveRun {
            run: target.clone(),
        });
    }
    ops.push(DraftOp::AmendRun { run: target, to });
    Ok(ops)
}

fn add(change: &ProposedChange, policy: &SchedulePolicy) -> Result<Vec<DraftOp>, Refusal> {
    let at = change.new_datetime.ok_or(Refusal::NoDayAndTime)?;
    if change.bosses.is_empty() {
        return Err(Refusal::NoBosses);
    }
    Ok(vec![
        DraftOp::CreateRun {
            fixed: None,
            channel_id: change.channel_id.clone(),
            week_start: week_of(&at, policy)?,
            datetime: at,
            bosses: change.bosses.clone(),
            participants: change.participants.clone(),
            status: RunStatus::Planned,
            source: RunSource::Amend,
        },
        DraftOp::EnsureReminders {
            run: Target::Created(0),
        },
    ])
}

fn status(run: Option<&Run>, to: RunStatus) -> Result<Vec<DraftOp>, Refusal> {
    let run = run.ok_or(Refusal::RunGone)?;
    Ok(vec![DraftOp::SetStatus {
        run: Target::Existing(run.id.clone()),
        change: StatusChange {
            status: to,
            announce: false,
            via_portal: false,
        },
    }])
}

/// v4 `_sub`: leavers not on the run and joiners already on it are ignored.
fn sub(change: &ProposedChange, run: &Run) -> Result<Vec<DraftOp>, Refusal> {
    let (remove, add) = match &change.payload {
        Payload::Sub { remove, add } => (remove.as_slice(), add.as_slice()),
        _ => (&[][..], &[][..]),
    };
    let mut people = run.participants.clone();
    let mut leaving = Vec::new();
    for uid in remove {
        if let Some(at) = people.iter().position(|p| p == uid) {
            people.remove(at);
            leaving.push(uid.clone());
        }
    }
    let mut joining = Vec::new();
    for uid in add {
        if !people.contains(uid) {
            people.push(uid.clone());
            joining.push(uid.clone());
        }
    }
    if people == run.participants {
        return Err(Refusal::NobodyToSwap);
    }
    if people.is_empty() {
        return Err(Refusal::RunEmptied);
    }
    Ok(vec![DraftOp::SwapParticipants {
        run: Target::Existing(run.id.clone()),
        remove: leaving,
        add: joining,
        via_portal: false,
    }])
}

/// v4 `_split`: shrink the run and create one for the bosses that left;
/// when every boss leaves it is a move.
fn split(
    change: &ProposedChange,
    run: &Run,
    snapshot: &ScheduleSnapshot,
    policy: &SchedulePolicy,
) -> Result<Vec<DraftOp>, Refusal> {
    let (named, people) = match &change.payload {
        Payload::Split {
            bosses,
            participants,
        } => (
            bosses.as_ref().unwrap_or(&change.bosses),
            participants.as_slice(),
        ),
        _ => (&change.bosses, &[][..]),
    };
    let moved: Vec<String> = named
        .iter()
        .filter(|boss| run.bosses.contains(boss))
        .cloned()
        .collect();
    let remaining: Vec<String> = run
        .bosses
        .iter()
        .filter(|boss| !moved.contains(boss))
        .cloned()
        .collect();
    if moved.is_empty() {
        return Err(Refusal::NoBossesFromRun);
    }
    if remaining.is_empty() {
        return move_run(change, run, snapshot, policy);
    }
    let people = [
        people,
        change.participants.as_slice(),
        run.participants.as_slice(),
    ]
    .into_iter()
    .find(|list| !list.is_empty())
    .unwrap_or_default()
    .to_vec();
    let at = change.new_datetime.unwrap_or(run.datetime);
    Ok(vec![
        DraftOp::SetRunBosses {
            run: Target::Existing(run.id.clone()),
            bosses: remaining,
        },
        DraftOp::CreateRun {
            fixed: None,
            channel_id: change.channel_id.clone().or(run.channel_id.clone()),
            week_start: week_of(&at, policy)?,
            datetime: at,
            bosses: moved,
            participants: people,
            status: RunStatus::Planned,
            source: RunSource::Amend,
        },
        DraftOp::EnsureReminders {
            run: Target::Created(1),
        },
    ])
}

/// v4 `_rsvp`: the chatbot's carded answers, then the status re-derived.
fn rsvp(change: &ProposedChange, run: &Run) -> Result<Vec<DraftOp>, Refusal> {
    let state = change.rsvp.ok_or(Refusal::NoAnswer)?;
    if change.participants.is_empty() {
        return Err(Refusal::NobodyNamed);
    }
    if change
        .participants
        .iter()
        .any(|uid| !run.participants.contains(uid))
    {
        return Err(Refusal::AnswerForOutsider);
    }
    let target = Target::Existing(run.id.clone());
    let mut ops: Vec<DraftOp> = change
        .participants
        .iter()
        .map(|uid| DraftOp::SetRsvp {
            run: target.clone(),
            user_id: uid.clone(),
            state,
            source: RsvpSource::Chat,
        })
        .collect();
    ops.push(DraftOp::RecountRun { run: target });
    Ok(ops)
}

/// v4 `_fix`, `_refix` and `_unfix`; returns the timing an edit or
/// removal names.
fn fix(
    change: &ProposedChange,
    snapshot: &ScheduleSnapshot,
) -> Result<(Vec<DraftOp>, Option<String>), Refusal> {
    let timing = |id: &Option<String>| -> Result<String, Refusal> {
        id.clone().ok_or(Refusal::NoTimingNamed)
    };
    let exists = |id: &str| snapshot.fixed_runs.iter().any(|row| row.id == id);
    match &change.payload {
        Payload::FixRemove { fixed_run_id } => {
            let id = timing(fixed_run_id)?;
            if !exists(&id) {
                return Err(Refusal::TimingAlreadyGone);
            }
            Ok((
                vec![DraftOp::RetireFixedRun {
                    fixed: Target::Existing(id.clone()),
                    weeks: Vec::new(),
                }],
                Some(id),
            ))
        }
        Payload::FixEdit {
            fixed_run_id,
            weekday,
            time,
            participants,
        } => {
            let id = timing(fixed_run_id)?;
            if !exists(&id) {
                return Err(Refusal::TimingGone);
            }
            let mut edit = FixedEdit::default();
            if let (Some(weekday), Some(time)) = (weekday, time) {
                edit.weekday = Some(*weekday);
                edit.time = Some(*time);
            }
            if !participants.is_empty() {
                edit.participants = Some(participants.clone());
            }
            if edit == FixedEdit::default() {
                return Err(Refusal::NothingLeftToChange);
            }
            Ok((
                vec![DraftOp::ApplyFixedEdit {
                    fixed: Target::Existing(id.clone()),
                    edit,
                    choices: FixedEditChoices::UpdateAll,
                }],
                Some(id),
            ))
        }
        other => {
            let Payload::Fix {
                weekday: Some(weekday),
                time: Some(time),
            } = other
            else {
                return Err(Refusal::NoRecurringSlot);
            };
            if change.bosses.is_empty() {
                return Err(Refusal::NoBosses);
            }
            Ok((
                vec![DraftOp::AddFixedRun(NewFixedRun {
                    owner_id: String::new(),
                    channel_id: change.channel_id.clone(),
                    bosses: change.bosses.clone(),
                    weekday: *weekday,
                    time: *time,
                    participants: change.participants.clone(),
                    note: Some(CREATED_FROM_CHAT.to_owned()),
                })],
                None,
            ))
        }
    }
}

/// Stage a removal for the boss weeks materialised at `now` (as `/fixed
/// remove` does); the merge refuses it once they have rolled over.
fn retire_weeks(
    ops: &mut [DraftOp],
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<(), Refusal> {
    let weeks = policy
        .materialised_weeks(now)
        .map_err(|error| Refusal::Rule(error.to_string()))?
        .iter()
        .map(utc_instant)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| Refusal::Rule(error.to_string()))?;
    for op in ops {
        if let DraftOp::RetireFixedRun { weeks: staged, .. } = op {
            staged.clone_from(&weeks);
        }
    }
    Ok(())
}

/// Fill the approver into what [`translate`] left open: an unnamed new
/// run's or timing's party, and a new timing's owner (v4: the confirming
/// member owns it, as `/fixed add` does).
pub fn fill_approver(ops: &mut [DraftOp], user_id: &str) {
    for op in ops {
        match op {
            DraftOp::CreateRun { participants, .. } if participants.is_empty() => {
                participants.push(user_id.to_owned());
            }
            DraftOp::AddFixedRun(new) => {
                if new.owner_id.is_empty() {
                    user_id.clone_into(&mut new.owner_id);
                }
                if new.participants.is_empty() {
                    new.participants.push(user_id.to_owned());
                }
            }
            _ => {}
        }
    }
}
