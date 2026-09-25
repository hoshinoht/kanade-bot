//! Finding the run or weekly timing a command names, who may change it, the
//! dropdown labels (v4 `_run_label`/`_fixed_label`) and refusal wording.

use chrono::{DateTime, Utc};
use twilight_model::application::command::CommandOptionChoice;

use super::access::Invoker;
use super::context::{CommandContext, store_failed};
use super::dispatch::{CommandError, MAX_CHOICES, choice};
use super::text::{format_bosses, hhmm, local_day, local_time, weekday_name};
use crate::bot::ids::id_text;
use crate::domain::ids::{IdError, resolve_id, short_id};
use crate::domain::schedule::{
    FixedRun, Run, RunStatus, ScheduleError, ScheduleSnapshot, utc_instant,
};
use crate::domain::scheduler::{SchedulerError, Scope};

/// The whole schedule (every weekly timing and run).
pub async fn everything(ctx: &CommandContext) -> Result<ScheduleSnapshot, CommandError> {
    ctx.store.snapshot(Scope::All).await.map_err(store_failed)
}

/// The current and next two boss weeks, as stored (v4
/// `materialised_week_starts`).
pub fn materialised_weeks(
    ctx: &CommandContext,
    now: DateTime<Utc>,
) -> Result<Vec<DateTime<Utc>>, CommandError> {
    let weeks = ctx
        .policy
        .materialised_weeks(now)
        .map_err(|error| CommandError::Internal(error.to_string()))?;
    weeks
        .iter()
        .map(|week| utc_instant(week).map_err(|error| CommandError::Internal(error.to_string())))
        .collect()
}

/// v4 `_resolve`: an id or unique prefix, or guidance naming `noun`.
///
/// # Errors
/// A [`CommandError::User`] with v4's wording.
pub fn resolve<'a>(
    raw: &str,
    candidates: impl IntoIterator<Item = &'a str>,
    noun: &str,
) -> Result<String, CommandError> {
    match resolve_id(raw, candidates) {
        Ok(id) => Ok(id.to_owned()),
        Err(IdError::Ambiguous { candidates, .. }) => {
            let listed: Vec<String> = candidates
                .iter()
                .take(8)
                .map(|id| format!("`#{}`", short_id(id)))
                .collect();
            Err(CommandError::User(format!(
                "`{raw}` matches several {noun}s: {} - be more specific",
                listed.join(", ")
            )))
        }
        Err(error) => Err(CommandError::User(format!(
            "{error} - pick a {noun} from the dropdown or check `/schedule`"
        ))),
    }
}

/// The owner of the weekly timing a run came from.
pub fn owner_of<'a>(snapshot: &'a ScheduleSnapshot, run: &Run) -> Option<&'a str> {
    let fixed = run.fixed_run_id.as_deref()?;
    snapshot
        .fixed_runs
        .iter()
        .find(|row| row.id == fixed)
        .map(|row| row.owner_id.as_str())
}

/// v4 `can_modify_run`: participants, the timing's owner, or an admin.
pub fn can_modify_run(snapshot: &ScheduleSnapshot, run: &Run, user: &str, admin: bool) -> bool {
    admin || run.participants.iter().any(|id| id == user) || owner_of(snapshot, run) == Some(user)
}

/// v4 `can_modify_fixed`: its owner, its participants, or an admin.
pub fn can_modify_fixed(fixed: &FixedRun, user: &str, admin: bool) -> bool {
    admin || fixed.owner_id == user || fixed.participants.iter().any(|id| id == user)
}

/// v4 `list_fixed_runs(involving=)`: owner or participant.
pub fn involves_fixed(fixed: &FixedRun, user: &str) -> bool {
    can_modify_fixed(fixed, user, false)
}

/// v4 `list_runs(involving=)`: on the run, or on/owner of its timing.
pub fn involves_run(snapshot: &ScheduleSnapshot, run: &Run, user: &str) -> bool {
    run.participants.iter().any(|id| id == user)
        || run.fixed_run_id.as_deref().is_some_and(|fixed| {
            snapshot
                .fixed_runs
                .iter()
                .any(|row| row.id == fixed && involves_fixed(row, user))
        })
}

/// v4 `_load_run`: any run by id or prefix that the invoker may change.
///
/// # Errors
/// v4's refusal texts, or a store failure.
pub async fn load_run(
    ctx: &CommandContext,
    invoker: &Invoker,
    raw: &str,
) -> Result<(ScheduleSnapshot, Run), CommandError> {
    let snapshot = everything(ctx).await?;
    let id = resolve(raw, snapshot.runs.iter().map(|run| run.id.as_str()), "run")?;
    let run = snapshot
        .runs
        .iter()
        .find(|run| run.id == id)
        .cloned()
        .ok_or_else(|| CommandError::User(format!("No run `#{}`.", short_id(raw))))?;
    if !can_modify_run(
        &snapshot,
        &run,
        &id_text(invoker.user_id),
        ctx.is_admin(invoker),
    ) {
        return Err(CommandError::User(format!(
            "You're not on run `#{}`, so you can't change it.",
            short_id(&run.id)
        )));
    }
    Ok((snapshot, run))
}

/// v4 `_channel_name`.
pub fn channel_name(ctx: &CommandContext, channel_id: Option<&str>) -> String {
    match channel_id.filter(|id| !id.is_empty()) {
        None => "no channel".to_owned(),
        Some(id) => ctx
            .channels
            .name(id)
            .map_or_else(|| "#unknown".to_owned(), |name| format!("#{name}")),
    }
}

fn cut(label: String) -> String {
    label.chars().take(100).collect()
}

/// v4 `_run_label`.
pub fn run_label(ctx: &CommandContext, run: &Run) -> String {
    let zone = ctx.policy.zone();
    let when = if run.status == RunStatus::Otot {
        "own time".to_owned()
    } else {
        format!(
            "{} {}",
            local_day(run.datetime, zone),
            local_time(run.datetime, zone)
        )
    };
    cut(format!(
        "{} · {when} · {} · {}",
        format_bosses(&run.bosses),
        channel_name(ctx, run.channel_id.as_deref()),
        short_id(&run.id)
    ))
}

/// v4 `_fixed_label`.
pub fn fixed_label(ctx: &CommandContext, fixed: &FixedRun) -> String {
    cut(format!(
        "{} · {} {} · {} · {}",
        format_bosses(&fixed.bosses),
        weekday_name(fixed.weekday),
        hhmm(fixed.time),
        channel_name(ctx, fixed.channel_id.as_deref()),
        short_id(&fixed.id)
    ))
}

/// v4 `_matches`: empty text, a label substring, or a short-id prefix.
pub fn matches(text: &str, label: &str, id: &str) -> bool {
    let text = text.trim().trim_start_matches('#').to_lowercase();
    text.is_empty() || label.to_lowercase().contains(&text) || short_id(id).starts_with(&text)
}

/// Which runs a run picker lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunPicker {
    /// v4 `run_autocomplete`: live runs of the materialised weeks.
    Live,
    /// v4 `any_run_autocomplete`: every status, labelled with it.
    AnyStatus,
    /// v4 `/debug` picker: every run in the store.
    Everything,
}

/// Run choices for `invoker`: their own runs unless they are an admin
/// (debug lists every run). Must not fail, so errors list nothing.
pub async fn run_choices(
    ctx: &CommandContext,
    invoker: &Invoker,
    typed: &str,
    picker: RunPicker,
) -> Vec<CommandOptionChoice> {
    let Ok(snapshot) = everything(ctx).await else {
        return Vec::new();
    };
    let weeks = match picker {
        RunPicker::Everything => Vec::new(),
        _ => materialised_weeks(ctx, ctx.now()).unwrap_or_default(),
    };
    let user = id_text(invoker.user_id);
    let everyone = picker == RunPicker::Everything || ctx.is_admin(invoker);
    let mut runs: Vec<&Run> = snapshot
        .runs
        .iter()
        .filter(|run| picker == RunPicker::Everything || weeks.contains(&run.week_start))
        .filter(|run| picker != RunPicker::Live || run.status.is_live())
        .filter(|run| everyone || involves_run(&snapshot, run, &user))
        .collect();
    runs.sort_by_key(|run| run.datetime);
    runs.into_iter()
        .filter_map(|run| {
            let mut label = run_label(ctx, run);
            if picker == RunPicker::AnyStatus {
                label = cut(format!("{label} · {}", run.status.as_str()));
            }
            matches(typed, &label, &run.id).then(|| choice(&label, run.id.clone()))
        })
        .take(MAX_CHOICES)
        .collect()
}

/// v4 `fixed_autocomplete`: the invoker's timings (every one for admins),
/// by weekday and time.
pub async fn fixed_choices(
    ctx: &CommandContext,
    invoker: &Invoker,
    typed: &str,
) -> Vec<CommandOptionChoice> {
    let Ok(snapshot) = ctx.store.snapshot(Scope::Weeks(Vec::new())).await else {
        return Vec::new();
    };
    let user = id_text(invoker.user_id);
    let admin = ctx.is_admin(invoker);
    snapshot
        .fixed_runs
        .iter()
        .filter(|fixed| admin || involves_fixed(fixed, &user))
        .filter_map(|fixed| {
            let label = fixed_label(ctx, fixed);
            matches(typed, &label, &fixed.id).then(|| choice(&label, fixed.id.clone()))
        })
        .take(MAX_CHOICES)
        .collect()
}

/// A scheduler refusal in the words members see; store trouble stays hidden.
pub fn refused(error: SchedulerError) -> CommandError {
    match error {
        SchedulerError::Schedule(error) => CommandError::User(schedule_text(&error)),
        SchedulerError::Forbidden(reason) => CommandError::User(reason),
        other => CommandError::Internal(other.to_string()),
    }
}

fn schedule_text(error: &ScheduleError) -> String {
    match error {
        // `/amend`'s own wording (v4 `commands.amend`), not the service's.
        ScheduleError::MoveConflict {
            week_day,
            existing_short_id,
            existing_day,
            existing_time,
        } => format!(
            "That weekly already has a run in the week of {week_day} \
             (`#{existing_short_id}` on {existing_day} {existing_time}). \
             Edit that existing run instead, or keep this move within its current boss week."
        ),
        other => other.to_string(),
    }
}
