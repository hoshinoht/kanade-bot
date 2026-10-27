//! One plain line per staged draft operation, as the inbox shows a member
//! request's change it has no dedicated row for.

use super::{inbox::slot, week::Context, when};
use kanade_domain::{
    drafts::{DraftOp, Target},
    ids::short_id,
    schedule::{RunStatus, ScheduleSnapshot},
};

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
