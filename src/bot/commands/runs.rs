//! Run changes for this week: `/amend`, `/status` (which absorbs v4's
//! `/cancel`, `/otot`, `/done` and `/restore`), `/swap` and `/rsvp`. Each
//! goes through the shared scheduler writer as the member, surface
//! Discord; the scheduler writes the channel notices to the outbox.

use std::sync::Arc;

use twilight_model::application::command::Command;

use super::access::Gate;
use super::build::{choices, command, picked, user};
use super::context::CommandContext;
use super::dispatch::{ChoicesFuture, CommandError, CommandFuture, SlashCommand};
use super::invocation::Invocation;
use super::lookup::{RunPicker, everything, load_run, refused, resolve, run_choices};
use super::options::Args;
use super::text::{format_bosses, local_day, local_time, status_label};
use crate::api::write::RunWrite;
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;
use crate::chat::tools::propose::when::parse_when;
use crate::domain::history::Expect;
use crate::domain::ids::short_id;
use crate::domain::members::member_name;
use crate::domain::schedule::{RsvpState, Run, RunStatus, StatusChange};
use crate::domain::scheduler::SchedulerError;

const PICK_RUN: &str = "Pick from the dropdown, or paste an id like `a1b2c3d4`";

/// `/status` states: v4 `STATUS_CHOICES`, own time labelled as members say it.
const STATES: [(&str, &str); 5] = [
    ("planned", "planned"),
    ("confirmed", "confirmed"),
    ("own time", "otot"),
    ("done", "done"),
    ("cancelled", "cancelled"),
];

/// Which run command this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Amend,
    Status,
    Swap,
    Rsvp,
}

pub struct RunCommand {
    ctx: Arc<CommandContext>,
    kind: Kind,
}

impl RunCommand {
    pub fn amend(ctx: Arc<CommandContext>) -> Self {
        Self {
            ctx,
            kind: Kind::Amend,
        }
    }

    pub fn status(ctx: Arc<CommandContext>) -> Self {
        Self {
            ctx,
            kind: Kind::Status,
        }
    }

    pub fn swap(ctx: Arc<CommandContext>) -> Self {
        Self {
            ctx,
            kind: Kind::Swap,
        }
    }

    pub fn rsvp(ctx: Arc<CommandContext>) -> Self {
        Self {
            ctx,
            kind: Kind::Rsvp,
        }
    }

    /// One run write as the invoker; a redelivered interaction is already
    /// applied and reads back as done.
    async fn write(
        &self,
        invocation: &Invocation,
        run_id: &str,
        write: RunWrite,
    ) -> Result<Run, CommandError> {
        let (ctx, _) = self.ctx.write_context().await?;
        let origin = self
            .ctx
            .origin(&invocation.invoker, invocation.interaction.id);
        match self
            .ctx
            .writer
            .run(origin, Expect::default(), run_id, write, &ctx)
            .await
        {
            Ok(()) | Err(SchedulerError::AlreadyApplied { .. }) => {}
            Err(error) => return Err(refused(error)),
        }
        everything(&self.ctx)
            .await?
            .runs
            .into_iter()
            .find(|run| run.id == run_id)
            .ok_or_else(|| CommandError::Internal(format!("run {run_id} vanished")))
    }

    async fn amend_run(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let (_, run) = load_run(
            &self.ctx,
            &invocation.invoker,
            args.text("run_id").unwrap_or_default(),
        )
        .await?;
        let zone = self.ctx.policy.zone();
        let to = parse_when(args.text("to").unwrap_or_default(), zone, self.ctx.now())
            .map_err(CommandError::User)?;
        let moved = self
            .write(invocation, &run.id, RunWrite::Move { to })
            .await?;
        Ok(InteractionReply::ephemeral(format!(
            "✅ Run `#{}` moved to {} {}.",
            short_id(&run.id),
            local_day(moved.datetime, zone),
            local_time(moved.datetime, zone)
        )))
    }

    async fn set_status(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let (_, run) = load_run(
            &self.ctx,
            &invocation.invoker,
            args.text("run_id").unwrap_or_default(),
        )
        .await?;
        let status = RunStatus::parse(args.text("state").unwrap_or_default())
            .map_err(|error| CommandError::User(error.to_string()))?;
        // A slash change is a chat decision: announced, without the portal mark.
        let change = StatusChange {
            status,
            announce: true,
            via_portal: false,
        };
        let updated = self
            .write(invocation, &run.id, RunWrite::Status(change))
            .await?;
        let label = status_label(updated.status);
        if run.status == updated.status {
            return Ok(InteractionReply::ephemeral(format!(
                "Run `#{}` is already {label}.",
                short_id(&run.id)
            )));
        }
        let zone = self.ctx.policy.zone();
        Ok(InteractionReply::ephemeral(format!(
            "{label} — run `#{}` ({}, {} {}).",
            short_id(&run.id),
            format_bosses(&updated.bosses),
            local_day(updated.datetime, zone),
            local_time(updated.datetime, zone)
        )))
    }

    async fn swap_people(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let (_, run) = load_run(
            &self.ctx,
            &invocation.invoker,
            args.text("run_id").unwrap_or_default(),
        )
        .await?;
        let remove: Vec<String> = ["out", "out2"]
            .iter()
            .filter_map(|name| args.user(name))
            .collect();
        let add: Vec<String> = ["in", "in2"]
            .iter()
            .filter_map(|name| args.user(name))
            .collect();
        if remove.is_empty() && add.is_empty() {
            return Err(CommandError::User(
                "Pick someone to swap out, in, or both.".into(),
            ));
        }
        let updated = self
            .write(invocation, &run.id, RunWrite::Participants { add, remove })
            .await?;
        let (ctx, _) = self.ctx.write_context().await?;
        let names: Vec<String> = updated
            .participants
            .iter()
            .map(|uid| member_name(&ctx.directory, uid))
            .collect();
        Ok(InteractionReply::ephemeral(format!(
            "✅ Run `#{}` this week: {}\nThe weekly timing is unchanged — use `/fixed edit` for \
             that.",
            short_id(&run.id),
            names.join(", ")
        )))
    }

    async fn answer(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let snapshot = everything(&self.ctx).await?;
        let id = resolve(
            args.text("run_id").unwrap_or_default(),
            snapshot.runs.iter().map(|run| run.id.as_str()),
            "run",
        )?;
        let user = id_text(invocation.invoker.user_id);
        let on_it = snapshot
            .runs
            .iter()
            .any(|run| run.id == id && run.participants.contains(&user));
        if !on_it {
            return Err(CommandError::User(format!(
                "You're not on run `#{}`.",
                short_id(&id)
            )));
        }
        let (answer, state) = match args.text("answer") {
            Some("no") => ("no", RsvpState::No),
            _ => ("yes", RsvpState::Yes),
        };
        let updated = self
            .write(
                invocation,
                &id,
                RunWrite::Rsvp {
                    user_id: user,
                    answer: Some(state),
                },
            )
            .await?;
        Ok(InteractionReply::ephemeral(format!(
            "Noted: **{answer}** for run `#{}` ({}).",
            short_id(&id),
            status_label(updated.status)
        )))
    }
}

impl SlashCommand for RunCommand {
    fn definition(&self) -> Command {
        let run_id = picked("run_id", PICK_RUN, true);
        match self.kind {
            Kind::Amend => command(
                "amend",
                "Move a run to a new day/time",
                false,
                vec![
                    run_id,
                    super::build::text(
                        "to",
                        "e.g. `wed 21:30`, `tomorrow 9:45pm`, `in 2 hours`",
                        true,
                    ),
                ],
            ),
            Kind::Status => command(
                "status",
                "Set a run's status: planned, confirmed, own time, done or cancelled",
                false,
                vec![
                    picked(
                        "run_id",
                        "Any of your runs, including cancelled, own-time and finished ones",
                        true,
                    ),
                    choices(
                        "state",
                        "planned · confirmed · own time · done · cancelled",
                        true,
                        &STATES,
                    ),
                ],
            ),
            Kind::Swap => command(
                "swap",
                "Swap someone in or out for this week only",
                false,
                vec![
                    run_id,
                    user("out", "Who is dropping out this week", false),
                    user("in", "Who is standing in", false),
                    user("out2", "Someone else dropping out", false),
                    user("in2", "Someone else standing in", false),
                ],
            ),
            Kind::Rsvp => command(
                "rsvp",
                "Say whether you're on a run",
                false,
                vec![
                    run_id,
                    choices("answer", "yes or no", true, &[("yes", "yes"), ("no", "no")]),
                ],
            ),
        }
    }

    fn gate(&self) -> Gate {
        Gate::BossingRole
    }

    fn defer(&self) -> Option<bool> {
        Some(true)
    }

    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async move {
            match self.kind {
                Kind::Amend => self.amend_run(invocation).await,
                Kind::Status => self.set_status(invocation).await,
                Kind::Swap => self.swap_people(invocation).await,
                Kind::Rsvp => self.answer(invocation).await,
            }
        })
    }

    fn autocomplete<'a>(&'a self, invocation: &'a Invocation) -> ChoicesFuture<'a> {
        Box::pin(async move {
            let Some(("run_id", typed)) = invocation.focused() else {
                return Vec::new();
            };
            // `/status` must reach finished, cancelled and own-time runs.
            let picker = if self.kind == Kind::Status {
                RunPicker::AnyStatus
            } else {
                RunPicker::Live
            };
            run_choices(&self.ctx, &invocation.invoker, typed, picker).await
        })
    }
}
