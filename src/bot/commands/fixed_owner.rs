//! `/fixed owner`: hand a weekly timing to another party member (`to:`), or
//! ask to own it; and the Accept/Decline presses on the posted request
//! (user decision 2026-10-10). Rules and writes are `api::ownership`; the
//! request's message is posted by the delivery tick.

use super::access::Invoker;
use super::dispatch::CommandError;
use super::fixed::FixedCommand;
use super::invocation::Invocation;
use super::lookup::resolve;
use super::options::Args;
use crate::api::ownership::{self, OwnershipError};
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;
use crate::domain::ids::short_id;
use crate::domain::ownership::OwnershipRefusal;
use crate::domain::scheduler::SchedulerError;

/// Internal path of a request button press (never registered).
pub const OWNER_PRESS: &str = "owner-press";

fn answer(error: OwnershipError) -> CommandError {
    match error {
        OwnershipError::Store(error) => CommandError::Internal(error.to_string()),
        OwnershipError::Scheduler(error @ SchedulerError::Schedule(_))
        | OwnershipError::Scheduler(error @ SchedulerError::Forbidden(_)) => {
            super::lookup::refused(error)
        }
        OwnershipError::Scheduler(error) => CommandError::Internal(error.to_string()),
        other => CommandError::User(other.to_string()),
    }
}

impl FixedCommand {
    /// `/fixed owner id:<timing> [to:<member>]`.
    pub(super) async fn owner(
        &self,
        invocation: &Invocation,
    ) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let timings = self.timings().await?;
        let id = resolve(
            args.text("id").unwrap_or_default(),
            timings.iter().map(|fixed| fixed.id.as_str()),
            "fixed run",
        )?;
        let me = id_text(invocation.invoker.user_id);
        let staff = self.ctx.is_admin(&invocation.invoker);
        let now = self.ctx.now();
        let store = &*self.ctx.store;
        if let Some(to) = args.user("to") {
            let (ctx, _) = self.ctx.write_context().await?;
            let origin = self
                .ctx
                .origin(&invocation.invoker, invocation.interaction.id);
            let desk = ownership::OwnerDesk {
                store,
                writer: &*self.ctx.writer,
                ctx: &ctx,
            };
            let change = desk
                .hand_off(origin, &id, &me, staff, &to, now)
                .await
                .map_err(answer)?;
            return Ok(InteractionReply::ephemeral(format!(
                "👑 <@{to}> now owns weekly timing `#{}`.",
                short_id(&change.fixed.id)
            )));
        }
        let request_id = format!("discord-{}", invocation.interaction.id.get());
        let (fixed, _) = ownership::ask(store, &id, &me, request_id, now)
            .await
            .map_err(answer)?;
        Ok(InteractionReply::ephemeral(format!(
            "📨 Asked <@{}> to hand you weekly timing `#{}`. They have 24 hours; \
             the request is posted in the timing's channel.",
            fixed.owner(),
            short_id(&fixed.id)
        )))
    }

    /// An Accept or Decline press on a posted request. The requester's own
    /// Decline withdraws it.
    pub(super) async fn owner_press(
        &self,
        invocation: &Invocation,
    ) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let request_id = args.text("request").unwrap_or_default();
        let accept = args.text("answer") == Some("accept");
        let invoker: &Invoker = &invocation.invoker;
        let me = id_text(invoker.user_id);
        let now = self.ctx.now();
        let store = &*self.ctx.store;
        let request = store
            .owner_request(request_id.to_owned())
            .await
            .map_err(super::context::store_failed)?
            .ok_or_else(|| CommandError::User(OwnershipError::UnknownRequest.to_string()))?;
        if !accept && request.requester == me {
            ownership::withdraw(store, request_id, &me, now)
                .await
                .map_err(answer)?;
            return Ok(InteractionReply::ephemeral("↩️ Request withdrawn."));
        }
        let (ctx, _) = self.ctx.write_context().await?;
        let origin = self.ctx.origin(invoker, invocation.interaction.id);
        let staff = self.ctx.is_admin(invoker);
        let desk = ownership::OwnerDesk {
            store,
            writer: &*self.ctx.writer,
            ctx: &ctx,
        };
        let (decided, change) = match desk
            .decide(origin, request_id, &me, staff, accept, now)
            .await
        {
            Ok(done) => done,
            Err(OwnershipError::Refused(OwnershipRefusal::Closed)) => {
                return Err(CommandError::User(
                    "That request was already answered.".into(),
                ));
            }
            Err(error) => return Err(answer(error)),
        };
        let timing = short_id(&change.fixed.id);
        // An accept whose close lost to a withdraw still moved the owner.
        Ok(InteractionReply::ephemeral(if accept {
            format!(
                "👑 <@{}> now owns weekly timing `#{timing}`.",
                decided.requester
            )
        } else {
            format!(
                "Declined <@{}>'s request for `#{timing}`.",
                decided.requester
            )
        }))
    }
}
