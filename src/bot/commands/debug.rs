//! `/debug` (v4 `DebugGroup`). `ping` and `clear_test` post and delete real
//! test cards, prefixed `🧪 TEST — `, whose ✅/❌ drive the real RSVP flow and
//! which never touch the run's reminder rows; posting goes through the
//! [`DebugCards`](super::context::DebugCards) port. `reminders` lists stored
//! reminder rows (read only); `materialise` runs the scheduler writer's
//! idempotent materialisation, the same write `/fixed add` makes. `status`,
//! `upcoming` and `extract` are dropped (the admin app replaces them), and
//! `tick` is omitted: the delivery loop owns its one `Delivery` and lease and
//! ticks every `KANADE_TICK_SECONDS` (30 s), with no on-demand seam to call.

use std::sync::Arc;

use chrono::TimeDelta;
use twilight_model::application::command::Command;

use super::access::Gate;
use super::build::{choices, command, picked, subcommand};
use super::context::{CommandContext, TestKind, TestPosted};
use super::dispatch::{ChoicesFuture, CommandError, CommandFuture, SlashCommand};
use super::invocation::Invocation;
use super::lookup::{RunPicker, everything, refused, run_choices};
use super::options::Args;
use super::text::{format_bosses, local_day, local_time};
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;
use crate::domain::ids::{resolve_id, short_id};
use crate::domain::schedule::Reminder;

pub use crate::bot::delivery::TEST_PREFIX;
/// Until test cards have a delivery path.
pub const TEST_CARDS_UNAVAILABLE: &str = "Test cards aren't available right now.";
/// v4 caps `/debug` replies at 1900 characters.
const REPLY_CHARS: usize = 1900;

fn capped(text: &str) -> String {
    text.chars().take(REPLY_CHARS).collect()
}

/// v4 `render_reminder_rows`.
fn reminder_rows(reminders: &[&Reminder], zone: chrono_tz::Tz) -> String {
    if reminders.is_empty() {
        return "_none_".to_owned();
    }
    reminders
        .iter()
        .map(|reminder| {
            let state = if reminder.sent_at.is_some() {
                "sent"
            } else {
                "pending"
            };
            let message = reminder
                .message_id
                .as_deref()
                .map(|id| format!(" · msg `{id}`"))
                .unwrap_or_default();
            format!(
                "run `#{}` · `{}` · {} {} · {state}{message}",
                short_id(&reminder.run_id),
                reminder.kind,
                local_day(reminder.fire_at, zone),
                local_time(reminder.fire_at, zone)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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

    /// v4 `reminders`: one run's rows, else every row soonest first.
    async fn reminders(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let snapshot = everything(&self.ctx).await?;
        let rows: Vec<&Reminder> = match Args(&invocation.options).text("run_id") {
            Some(raw) => {
                let Ok(run_id) = resolve_id(raw, snapshot.runs.iter().map(|run| run.id.as_str()))
                else {
                    return Err(CommandError::User(format!("No run matches `{raw}`.")));
                };
                snapshot
                    .reminders
                    .iter()
                    .filter(|reminder| reminder.run_id == run_id)
                    .collect()
            }
            None => {
                let mut rows: Vec<&Reminder> = snapshot.reminders.iter().collect();
                rows.sort_by_key(|reminder| reminder.fire_at);
                rows
            }
        };
        Ok(InteractionReply::ephemeral(capped(&reminder_rows(
            &rows,
            self.ctx.policy.zone(),
        ))))
    }

    /// v4 `materialise`: the writer's idempotent materialisation of the
    /// current and coming boss weeks, listing the runs it created.
    async fn materialise(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let (ctx, _) = self.ctx.write_context().await?;
        let created = self
            .ctx
            .writer
            .materialise(self.ctx.plain_origin(&invocation.invoker), &ctx)
            .await
            .map_err(refused)?;
        if created.is_empty() {
            return Ok(InteractionReply::ephemeral(
                "Nothing new - both weeks were already materialised.",
            ));
        }
        let snapshot = everything(&self.ctx).await?;
        let zone = self.ctx.policy.zone();
        let lines: Vec<String> = snapshot
            .runs
            .iter()
            .filter(|run| created.contains(&run.id))
            .take(20)
            .map(|run| {
                format!(
                    "run `#{}` · {} · {} {}",
                    short_id(&run.id),
                    format_bosses(&run.bosses),
                    local_day(run.datetime, zone),
                    local_time(run.datetime, zone)
                )
            })
            .collect();
        Ok(InteractionReply::ephemeral(capped(&format!(
            "Created {} run(s):\n{}",
            created.len(),
            lines.join("\n")
        ))))
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
                subcommand(
                    "reminders",
                    "List reminder rows",
                    vec![picked("run_id", "Limit to one run (optional)", false)],
                ),
                subcommand(
                    "materialise",
                    "Force materialisation of both weeks",
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
                Some("reminders") => self.reminders(invocation).await,
                Some("materialise") => self.materialise(invocation).await,
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
