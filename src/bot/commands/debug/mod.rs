//! `/debug` (v4 `DebugGroup`). `ping` and `clear_test` post and delete real
//! test cards, prefixed `🧪 TEST — `, which never touch the run's reminder
//! rows; in the run's home channel their ✅/❌ drive the real RSVP flow,
//! elsewhere (and for sample runs and digests) they are display only.
//! `header` tries the persona header rewrite and posts the verdicts. All
//! three go through the [`DebugCards`](super::context::DebugCards) port.
//! `reminders` lists stored
//! reminder rows (read only); `materialise` runs the scheduler writer's
//! idempotent materialisation, the same write `/fixed add` makes. `status`,
//! `upcoming` and `extract` are dropped (the admin app replaces them), and
//! `tick` is omitted: the delivery loop owns its one `Delivery` and lease and
//! ticks every `KANADE_TICK_SECONDS` (30 s), with no on-demand seam to call.

mod header;
mod ping;

use std::sync::Arc;

use chrono::TimeDelta;
use twilight_model::application::command::Command;

use super::access::Gate;
use super::build::{command, picked, subcommand};
use super::context::{CommandContext, TestKind, TestPosted, TestReport, TestSubject};
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

/// The style and header notes after a ping's reply.
fn notes(request: &super::context::PingRequest, report: &TestReport) -> String {
    let mut text = String::new();
    if !matches!(
        report.posted,
        TestPosted::Posted { .. } | TestPosted::Sandboxed { .. }
    ) {
        return text;
    }
    if let Some(style) = request.style {
        text.push_str(&format!(" Style: `{}` (this post only).", style.as_str()));
    }
    match &report.header {
        Some(note) if note.rewritten => text.push_str(" Header: fresh rewrite (not stored)."),
        Some(note) => text.push_str(&format!(" Header: the seed ({}).", note.reason)),
        None if request.rewrite => text.push_str(&format!(
            " Header: `{}` has none to rewrite.",
            request.kind.as_str()
        )),
        None => {}
    }
    text
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
        let request = ping::request(&self.ctx, invocation).await?;
        let cards = self
            .ctx
            .debug_cards
            .as_ref()
            .ok_or_else(Self::unavailable)?;
        let report = cards
            .ping(request.clone())
            .await
            .map_err(CommandError::Internal)?;
        let kind = request.kind.as_str();
        let what = match &request.subject {
            TestSubject::Run(run_id) if request.kind == TestKind::Digest => {
                format!("`{kind}` test of run `#{}`'s boss week", short_id(run_id))
            }
            TestSubject::Run(run_id) => format!("`{kind}` test for run `#{}`", short_id(run_id)),
            TestSubject::Sample(_) => format!("sample `{kind}` test"),
            TestSubject::Week => format!("`{kind}` test of this boss week"),
        };
        let mut text = match &report.posted {
            TestPosted::Posted { channel_id } => format!(
                "✅ Posted a {what} in <#{channel_id}>. Its ✅/❌ drive the real RSVP flow; the \
                 scheduled reminders are untouched."
            ),
            TestPosted::Sandboxed { channel_id } => {
                let sandbox = if matches!(request.subject, TestSubject::Run(_))
                    && request.kind != TestKind::Digest
                {
                    "sandbox "
                } else {
                    ""
                };
                format!(
                    "✅ Posted a {sandbox}{what} in <#{channel_id}> (display only: nothing is \
                     stored for it and reactions do nothing here)."
                )
            }
            TestPosted::Unreachable => {
                let home =
                    request.channel.is_none() && matches!(request.subject, TestSubject::Run(_));
                return Err(CommandError::User(
                    if home {
                        "That run's home channel isn't reachable."
                    } else {
                        "That channel isn't reachable."
                    }
                    .into(),
                ));
            }
            TestPosted::Unconfirmed => "⚠️ Delivery of the test message was not confirmed. \
                                        Check the channel before retrying."
                .to_owned(),
        };
        text.push_str(&notes(&request, &report));
        Ok(InteractionReply::ephemeral(text))
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
                    "Post a test reminder for a run (or a sample run) right now",
                    ping::options(&kinds),
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
                subcommand(
                    "header",
                    "Try the persona header rewrite and post each verdict",
                    header::options(),
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
                Some("header") => header::run(&self.ctx, invocation).await,
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
