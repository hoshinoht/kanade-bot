//! `/debug ping` and `/debug clear_test` (v4 `DebugGroup`): real test
//! cards, prefixed `🧪 TEST — `, whose ✅/❌ drive the real RSVP flow and
//! which never touch the run's reminder rows. Posting goes through the
//! [`DebugCards`](super::context::DebugCards) port. `/debug status`,
//! `extract` are dropped (the admin app replaces them); `reminders`, `tick`
//! and `materialise` are deferred.

use std::sync::Arc;

use chrono::TimeDelta;
use twilight_model::application::command::Command;

use super::access::Gate;
use super::build::{choices, command, picked, subcommand};
use super::context::{CommandContext, TestKind, TestPosted};
use super::dispatch::{ChoicesFuture, CommandError, CommandFuture, SlashCommand};
use super::invocation::Invocation;
use super::lookup::{RunPicker, everything, run_choices};
use super::options::Args;
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;
use crate::domain::ids::{resolve_id, short_id};

pub use crate::bot::delivery::TEST_PREFIX;
/// Until test cards have a delivery path.
pub const TEST_CARDS_UNAVAILABLE: &str = "Test cards aren't available right now.";

pub struct DebugCommand {
    ctx: Arc<CommandContext>,
}

impl DebugCommand {
    pub fn new(ctx: Arc<CommandContext>) -> Self {
        Self { ctx }
    }

    fn unavailable() -> CommandError {
        CommandError::User(TEST_CARDS_UNAVAILABLE.into())
    }

    async fn ping(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let raw = args.text("run_id").unwrap_or_default();
        let snapshot = everything(&self.ctx).await?;
        // /debug reaches any run.
        let Ok(run_id) = resolve_id(raw, snapshot.runs.iter().map(|run| run.id.as_str())) else {
            return Err(CommandError::User(format!("No run matches `{raw}`.")));
        };
        let wanted = args.text("kind").unwrap_or_default();
        let Some(kind) = TestKind::parse(wanted) else {
            return Err(CommandError::User(format!(
                "Don't know how to render `{wanted}`."
            )));
        };
        let cards = self
            .ctx
            .debug_cards
            .as_ref()
            .ok_or_else(Self::unavailable)?;
        let posted = cards
            .post(run_id.to_owned(), kind, id_text(invocation.invoker.user_id))
            .await
            .map_err(CommandError::Internal)?;
        Ok(InteractionReply::ephemeral(match posted {
            TestPosted::Posted { channel_id } => format!(
                "✅ Posted a `{}` test for run `#{}` in <#{channel_id}>. Its ✅/❌ drive the real \
                 RSVP flow; the scheduled reminders are untouched.",
                kind.as_str(),
                short_id(run_id)
            ),
            TestPosted::Unreachable => {
                return Err(CommandError::User(
                    "That run's home channel isn't reachable.".into(),
                ));
            }
            TestPosted::Unconfirmed => "⚠️ Delivery of the test message was not confirmed. \
                                        Check the channel before retrying."
                .to_owned(),
        }))
    }

    async fn clear_test(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let cards = self
            .ctx
            .debug_cards
            .as_ref()
            .ok_or_else(Self::unavailable)?;
        let channel = invocation.channel_id.map(id_text).unwrap_or_default();
        let since = self.ctx.now() - TimeDelta::hours(24);
        let (deleted, failed) = cards
            .clear(channel, since)
            .await
            .map_err(CommandError::Internal)?;
        let tail = if failed > 0 {
            format!(", {failed} could not be deleted.")
        } else {
            ".".to_owned()
        };
        Ok(InteractionReply::ephemeral(format!(
            "🧹 Removed {deleted} test message(s){tail}"
        )))
    }
}

impl SlashCommand for DebugCommand {
    fn definition(&self) -> Command {
        let kinds: Vec<(&str, &str)> = TestKind::ALL
            .iter()
            .map(|kind| (kind.as_str(), kind.as_str()))
            .collect();
        command(
            "debug",
            "Testing aids (admins only)",
            true,
            vec![
                subcommand(
                    "ping",
                    "Post a test reminder for a run right now",
                    vec![
                        picked(
                            "run_id",
                            "Pick from the dropdown, or paste an id like `a1b2c3d4`",
                            true,
                        ),
                        choices("kind", "Which message to post", true, &kinds),
                    ],
                ),
                subcommand(
                    "clear_test",
                    "Delete this channel's 🧪 TEST messages from the last 24h",
                    Vec::new(),
                ),
            ],
        )
    }

    fn gate(&self) -> Gate {
        Gate::Debug
    }

    fn defer(&self) -> Option<bool> {
        Some(true)
    }

    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async move {
            match invocation.path.get(1).map(String::as_str) {
                Some("ping") => self.ping(invocation).await,
                Some("clear_test") => self.clear_test(invocation).await,
                other => Err(CommandError::Internal(format!(
                    "unknown /debug subcommand {other:?}"
                ))),
            }
        })
    }

    fn autocomplete<'a>(&'a self, invocation: &'a Invocation) -> ChoicesFuture<'a> {
        Box::pin(async move {
            match invocation.focused() {
                Some(("run_id", typed)) => {
                    run_choices(&self.ctx, &invocation.invoker, typed, RunPicker::Everything).await
                }
                _ => Vec::new(),
            }
        })
    }
}
