//! An inbox item's one-line consequence ("Party unchanged · 3 reminders will
//! move"), read off the current schedule and the merge's preview snapshot.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::{
    reminders::{classified, is_upcoming},
    week::Context,
};
use crate::domain::schedule::{Run, RunStatus, ScheduleSnapshot};

/// `None` when approving changes nothing worth a line.
pub fn consequence(
    ctx: &Context<'_>,
    current: &ScheduleSnapshot,
    preview: &ScheduleSnapshot,
) -> Option<String> {
    let parts: Vec<String> = party(ctx, current, preview)
        .into_iter()
        .chain(reminders(ctx, current, preview))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Joins and leaves across live runs whose time, bosses, channel or party
/// change; else the size of the new runs. Cancelled runs say nothing.
fn party(
    ctx: &Context<'_>,
    current: &ScheduleSnapshot,
    preview: &ScheduleSnapshot,
) -> Option<String> {
    let mut changed = false;
    let mut joins: Vec<&String> = Vec::new();
    let mut leaves: Vec<&String> = Vec::new();
    let mut created: Vec<&Run> = Vec::new();
    for run in preview.runs.iter().filter(|run| run.status.is_live()) {
        let Some(old) = current.runs.iter().find(|old| old.id == run.id) else {
            created.push(run);
            continue;
        };
        if old.status == RunStatus::Cancelled
            || (
                old.datetime,
                &old.bosses,
                &old.channel_id,
                &old.participants,
            ) == (
                run.datetime,
                &run.bosses,
                &run.channel_id,
                &run.participants,
            )
        {
            continue;
        }
        changed = true;
        for id in run
            .participants
            .iter()
            .filter(|id| !old.participants.contains(id))
        {
            if !joins.contains(&id) {
                joins.push(id);
            }
        }
        for id in old
            .participants
            .iter()
            .filter(|id| !run.participants.contains(id))
        {
            if !leaves.contains(&id) {
                leaves.push(id);
            }
        }
    }
    if changed {
        return Some(match (joins.as_slice(), leaves.as_slice()) {
            ([], []) => "Party unchanged".into(),
            ([one], []) => format!("Adds {}", ctx.name(one)),
            ([], [one]) => format!("Removes {}", ctx.name(one)),
            ([new], [old]) => format!("{} replaces {}", ctx.name(new), ctx.name(old)),
            (joins, []) => format!("Party +{}", joins.len()),
            ([], leaves) => format!("Party −{}", leaves.len()),
            (joins, leaves) => format!("Party +{} −{}", joins.len(), leaves.len()),
        });
    }
    let size = created.first()?.participants.len();
    created
        .iter()
        .all(|run| run.participants.len() == size)
        .then(|| format!("Party of {size}"))
}

/// Upcoming reminders (the Reminders page's queued and due rows) by run and
/// kind, with their fire time.
fn upcoming<'s>(
    ctx: &Context<'_>,
    snapshot: &'s ScheduleSnapshot,
) -> BTreeMap<(&'s str, &'s str), DateTime<Utc>> {
    classified(ctx, snapshot)
        .filter(|(.., state)| is_upcoming(state))
        .map(|(reminder, ..)| {
            (
                (reminder.run_id.as_str(), reminder.kind.as_str()),
                reminder.fire_at,
            )
        })
        .collect()
}

fn reminders(
    ctx: &Context<'_>,
    current: &ScheduleSnapshot,
    preview: &ScheduleSnapshot,
) -> Option<String> {
    let before = upcoming(ctx, current);
    let after = upcoming(ctx, preview);
    let mut moved = 0;
    let mut dropped = 0;
    for (key, at) in &before {
        match after.get(key) {
            Some(to) if to != at => moved += 1,
            Some(_) => {}
            None => dropped += 1,
        }
    }
    let added = after
        .keys()
        .filter(|key| !before.contains_key(*key))
        .count();
    // Only the first count names the noun, to keep the line short:
    // "2 reminders will move, 1 will be added".
    let parts: Vec<String> = [
        (moved, "move"),
        (dropped, "be dropped"),
        (added, "be added"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .enumerate()
    .map(|(n, (count, verb))| match (n, count) {
        (0, 1) => format!("1 reminder will {verb}"),
        (0, _) => format!("{count} reminders will {verb}"),
        _ => format!("{count} will {verb}"),
    })
    .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}
