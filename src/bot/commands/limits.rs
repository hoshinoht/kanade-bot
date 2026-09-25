//! `/limits`: the invoker's chat allowance from the pilot's snapshot (v4
//! `commands.limits`).

use std::sync::Arc;

use twilight_model::application::command::Command;

use super::access::Gate;
use super::build::command;
use super::context::CommandContext;
use super::dispatch::{CommandError, CommandFuture, SlashCommand};
use super::invocation::Invocation;
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;
use crate::chat::gate::retry_note;
use crate::chat::pilot::AllowanceSnapshot;

pub const STAFF_LIMITS_REPLY: &str = "No limits for staff — fire away! 🎀🐾";
/// Until the chat pilot is wired into `serve`.
pub const LIMITS_UNAVAILABLE: &str = "Chat limits aren't available right now.";

const BAR_SEGMENTS: usize = 12;

/// v4 `usage_bar`: used allowance as blocks, rounding up.
pub fn usage_bar(used: usize, count: usize) -> String {
    let filled = if count == 0 {
        0
    } else {
        (BAR_SEGMENTS * used).div_ceil(count).min(BAR_SEGMENTS)
    };
    format!(
        "{}{}",
        "▰".repeat(filled),
        "▱".repeat(BAR_SEGMENTS - filled)
    )
}

/// The v4 reply for `user_id` from one allowance snapshot.
pub fn limits_text(snapshot: &AllowanceSnapshot, user_id: &str) -> String {
    let own = snapshot
        .members
        .iter()
        .find(|usage| usage.member_id == user_id);
    let (count, used, resets_in) = own.map_or((snapshot.member_default.0, 0, 0.0), |usage| {
        (usage.limit, usage.used, usage.resets_in_s)
    });
    let left = count.saturating_sub(used);
    let used = count - left;
    // With none left, the oldest answer freeing up is v4's `retry_after`.
    let when = if left == 0 {
        format!(
            "None left — ask me again in about {}.",
            retry_note(resets_in)
        )
    } else if used > 0 {
        format!(
            "{left} left — a spent one comes back in about {}.",
            retry_note(resets_in)
        )
    } else {
        "Your whole allowance is there — ask away.".to_owned()
    };
    let mut lines = vec![
        format!("🎀 **Your chat answers** — {used} of {count} used"),
        usage_bar(used, count),
        when,
    ];
    let pool = &snapshot.pool;
    if pool.used >= pool.limit {
        lines.push(format!(
            "-# The guild's shared pool is spent too, so nobody is being answered for about {}.",
            retry_note(pool.resets_in_s)
        ));
    }
    lines.join("\n")
}

pub struct LimitsCommand {
    ctx: Arc<CommandContext>,
}

impl LimitsCommand {
    pub fn new(ctx: Arc<CommandContext>) -> Self {
        Self { ctx }
    }
}

impl SlashCommand for LimitsCommand {
    fn definition(&self) -> Command {
        command(
            "limits",
            "See how many chat answers you have left",
            false,
            Vec::new(),
        )
    }

    fn gate(&self) -> Gate {
        Gate::BossingRole
    }

    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async move {
            if self
                .ctx
                .access
                .policy
                .is_staff(&invocation.invoker, invocation.owner_id)
            {
                return Ok(InteractionReply::ephemeral(STAFF_LIMITS_REPLY));
            }
            let Some(allowance) = &self.ctx.allowance else {
                return Err(CommandError::User(LIMITS_UNAVAILABLE.into()));
            };
            Ok(InteractionReply::ephemeral(limits_text(
                &allowance.snapshot(),
                &id_text(invocation.invoker.user_id),
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_rounds_up_like_v4() {
        assert_eq!(usage_bar(0, 4), "▱".repeat(12));
        assert_eq!(
            usage_bar(1, 4),
            format!("{}{}", "▰".repeat(3), "▱".repeat(9))
        );
        assert_eq!(
            usage_bar(1, 5),
            format!("{}{}", "▰".repeat(3), "▱".repeat(9))
        );
        assert_eq!(usage_bar(9, 4), "▰".repeat(12));
    }
}
