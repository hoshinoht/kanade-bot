//! A run completion prompt's presses (user decisions 2026-10-09/10), routed
//! as the presser's internal `/status` call. The dispatcher lets anyone in
//! (`Gate::Anyone` for this path); only one of the run's party or staff (the
//! admin role, the guild owner or Administrator, as `Gate::Staff`) may
//! answer, anyone else is refused ephemerally.
//!
//! One decision per ask: the press first claims the ask (`close_run_prompt`
//! has a single winner; a loser is told it was already answered and writes
//! nothing). Done and Didn't happen then settle the run through the shared
//! writer's guarded `settle_run`, which re-checks on the committed state
//! that the run is still live, unmoved and the presser's; a run that ended
//! or moved first is left as it is. If the claim landed but the settle was
//! lost, the tick finishes it. Not yet opens the next ask half an hour later
//! while that is before the cutoff, else leaves the run to the cutoff.

use super::components::INACTIVE;
use super::context::store_failed;
use super::dispatch::CommandError;
use super::invocation::Invocation;
use super::lookup::{everything, refused};
use super::options::Args;
use super::runs::RunCommand;
use super::text::{local_time, status_label};
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;
use crate::domain::completion::{
    ASK_AGAIN_AFTER, PromptClose, PromptOutcome, RunPrompt, pressed_request_id, run_minutes,
};
use crate::domain::history::{Actor, Origin, Surface};
use crate::domain::ids::short_id;
use crate::domain::schedule::{RunStatus, SettleRun};
use crate::domain::scheduler::SchedulerError;

/// Internal path of a prompt button press (never registered).
pub const PROMPT_PRESS: &str = "prompt-press";
/// The refusal for someone neither on the run nor staff.
pub const NOT_ON_RUN: &str = "Only the run's party or an admin can answer this.";
/// The answer to a press on an ask that is already closed.
pub const ALREADY_ANSWERED: &str = "That prompt was already answered.";
/// The answer when the run moved or ended before the press landed.
pub const RUN_CHANGED: &str = "That run changed before your answer landed; nothing was recorded.";

impl RunCommand {
    /// A Done / Didn't happen / Not yet press on a posted ask.
    pub(super) async fn prompt_press(
        &self,
        invocation: &Invocation,
    ) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let inactive = || CommandError::User(INACTIVE.into());
        let run_id = args.text("run").unwrap_or_default().to_owned();
        let ask: u32 = args
            .text("ask")
            .and_then(|ask| ask.parse().ok())
            .ok_or_else(inactive)?;
        let outcome = args
            .text("answer")
            .and_then(PromptOutcome::parse)
            .filter(|outcome| outcome.pressed())
            .ok_or_else(inactive)?;
        let ctx = &self.ctx;
        let prompt = ctx
            .run_prompts
            .run_prompt(run_id.clone(), ask)
            .await
            .map_err(store_failed)?
            .ok_or_else(inactive)?;
        let run = everything(ctx)
            .await?
            .runs
            .into_iter()
            .find(|run| run.id == run_id)
            .ok_or_else(inactive)?;
        let me = id_text(invocation.invoker.user_id);
        let staff = ctx
            .access
            .policy
            .is_staff(&invocation.invoker, invocation.owner_id);
        if !run.participants.contains(&me) && !staff {
            return Err(CommandError::User(NOT_ON_RUN.into()));
        }
        if !prompt.is_open() {
            return Err(CommandError::User(ALREADY_ANSWERED.into()));
        }
        let short = short_id(&run.id);
        if !run.status.is_live() {
            return Err(CommandError::User(format!(
                "Run `#{short}` is already {}.",
                status_label(run.status)
            )));
        }
        let lengths = match &ctx.config {
            Some(desk) => desk.settings().await.run_lengths,
            None => Default::default(),
        };
        let minutes = run_minutes(&lengths, Some(&ctx.catalog), &run.bosses);
        let moved = crate::domain::completion::plan(&run, minutes, &ctx.policy)
            .map_or(true, |plan| plan.ends_at != prompt.ends_at);
        if moved {
            return Err(CommandError::User(RUN_CHANGED.into()));
        }
        let now = ctx.now();
        let zone = ctx.policy.zone();
        let again = now + ASK_AGAIN_AFTER;
        let next = (outcome == PromptOutcome::NotYet && again < prompt.cutoff_at).then(|| {
            RunPrompt::open(
                run.id.clone(),
                ask + 1,
                prompt.ends_at,
                again,
                prompt.cutoff_at,
            )
        });
        let has_next = next.is_some();
        let close = PromptClose {
            outcome,
            decided_by: Some(me.clone()),
            at: now,
            next,
        };
        if !ctx
            .run_prompts
            .close_run_prompt(run.id.clone(), ask, close)
            .await
            .map_err(store_failed)?
        {
            return Err(CommandError::User(ALREADY_ANSWERED.into()));
        }
        if outcome == PromptOutcome::NotYet {
            return Ok(InteractionReply::ephemeral(if has_next {
                format!(
                    "⏳ I'll ask again about run `#{short}` at {}.",
                    local_time(again, zone)
                )
            } else {
                format!(
                    "Okay. If nobody marks it, run `#{short}` will be marked done at {}.",
                    local_time(prompt.cutoff_at, zone)
                )
            }));
        }
        let status = if outcome == PromptOutcome::Done {
            RunStatus::Done
        } else {
            RunStatus::Cancelled
        };
        let (write_ctx, _) = ctx.write_context().await?;
        let origin = Origin::new(Actor::member(me.clone()), Surface::Discord)
            .with_request_id(pressed_request_id(&run.id, ask, outcome));
        let settle = SettleRun {
            run_id: run.id.clone(),
            status,
            datetime: run.datetime,
            user: me,
            staff,
        };
        match ctx.writer.settle_run(origin, settle, &write_ctx).await {
            Ok(true) | Err(SchedulerError::AlreadyApplied { .. }) => {}
            Ok(false) => return Err(CommandError::User(RUN_CHANGED.into())),
            Err(error) => return Err(refused(error)),
        }
        Ok(InteractionReply::ephemeral(match outcome {
            PromptOutcome::Done => format!("🏁 Marked run `#{short}` done."),
            _ => format!("Marked run `#{short}` as didn't happen."),
        }))
    }
}
