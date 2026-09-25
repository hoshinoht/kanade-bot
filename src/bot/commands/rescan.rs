//! `/rescan`: queue a re-read of this (or every watched) channel through the
//! rescan runner port, or stop the one running (v4 `commands.rescan`).

use std::sync::{Arc, Mutex, PoisonError};

use twilight_model::application::command::Command;

use super::access::Gate;
use super::build::{choices, command, flag};
use super::context::CommandContext;
use super::dispatch::{CommandError, CommandFuture, SlashCommand};
use super::invocation::Invocation;
use super::options::Args;
use crate::api::rescan::{RescanRunner, RescanView};
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;
use crate::domain::ids::short_id;
use crate::extract::rescan::{RescanError, RescanRequest};
use crate::extract::window::{DEFAULT_WINDOW, WINDOWS};

/// Until the extractor's rescan runner is wired into `serve`.
pub const RESCAN_UNAVAILABLE: &str = "Rescans aren't available right now.";

/// Jobs remembered for `cancel:True`.
const REMEMBERED: usize = 16;

/// v4 `WINDOW_LABELS`.
pub fn window_label(window: &str) -> &str {
    match window {
        "week" => "this boss week",
        "2weeks" => "this and last boss week",
        "48h" => "the last 48 hours",
        "24h" => "the last 24 hours",
        other => other,
    }
}

pub struct RescanCommand {
    ctx: Arc<CommandContext>,
    /// Jobs queued from Discord, newest last: the runner lists no active job.
    jobs: Mutex<Vec<String>>,
}

impl RescanCommand {
    pub fn new(ctx: Arc<CommandContext>) -> Self {
        Self {
            ctx,
            jobs: Mutex::new(Vec::new()),
        }
    }

    fn remember(&self, id: &str) {
        let mut jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        jobs.retain(|known| known != id);
        jobs.push(id.to_owned());
        let excess = jobs.len().saturating_sub(REMEMBERED);
        jobs.drain(..excess);
    }

    fn remembered(&self) -> Vec<String> {
        self.jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn failed(error: RescanError) -> CommandError {
        match error {
            RescanError::NoChannels | RescanError::Window(_) => {
                CommandError::User(error.to_string())
            }
            RescanError::Closed => CommandError::User(RESCAN_UNAVAILABLE.into()),
            RescanError::Store(error) => CommandError::Internal(error.to_string()),
        }
    }

    async fn cancel(&self, runner: &dyn RescanRunner) -> Result<InteractionReply, CommandError> {
        for id in self.remembered().iter().rev() {
            let Some(view) = runner.job(id.clone()).await.map_err(Self::failed)? else {
                continue;
            };
            if view.job.status.is_final() {
                continue;
            }
            let view = runner
                .cancel(id.clone())
                .await
                .map_err(Self::failed)?
                .unwrap_or(view);
            let done = view.job.results.as_array().map_or(0, Vec::len);
            return Ok(InteractionReply::ephemeral(format!(
                "🛑 `{}` will stop after the channel it is on ({done} of {} done).",
                short_id(&view.job.id),
                view.job.channels.len()
            )));
        }
        Ok(InteractionReply::ephemeral("Nothing is being re-read."))
    }

    fn queued(&self, view: &RescanView, everywhere: bool) -> InteractionReply {
        let names: Vec<String> = view
            .job
            .channels
            .iter()
            .map(|id| {
                self.ctx
                    .channels
                    .name(id)
                    .map_or_else(|| id.clone(), |name| format!("#{name}"))
            })
            .collect();
        let place = if names.is_empty() {
            "every watched channel".to_owned()
        } else {
            names.join(", ")
        };
        let posted = if everywhere {
            "each channel"
        } else {
            "this channel"
        };
        InteractionReply::ephemeral(format!(
            "🔎 Re-reading **{place}** ({}) — I'll post the cards in {posted} as I find them. \
             `/rescan cancel:True` stops it.\n-# job `{}`",
            window_label(&view.job.window),
            short_id(&view.job.id)
        ))
    }

    async fn rescan(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let Some(runner) = self.ctx.rescans.as_deref() else {
            return Err(CommandError::User(RESCAN_UNAVAILABLE.into()));
        };
        let args = Args(&invocation.options);
        if args.flag("cancel").unwrap_or(false) {
            return self.cancel(runner).await;
        }
        let everywhere = args.text("scope") == Some("all-channels");
        let here = invocation.channel_id.map(id_text).unwrap_or_default();
        if !everywhere && !self.ctx.channels.is_watched(&here) {
            return Err(CommandError::User(
                "This channel isn't watched, so there's nothing to re-read. \
                 `/rescan scope:all channels` reads the ones that are."
                    .into(),
            ));
        }
        let channels = if everywhere {
            self.ctx.channels.watched()
        } else {
            vec![self.ctx.channels.origin(&here)]
        };
        let view = runner
            .submit(RescanRequest {
                channels,
                window: args.text("window").unwrap_or(DEFAULT_WINDOW).to_owned(),
                source: "slash".to_owned(),
                automated: false,
                requested_by: Some(id_text(invocation.invoker.user_id)),
                unprocessed_only: false,
            })
            .await
            .map_err(Self::failed)?;
        self.remember(&view.job.id);
        Ok(self.queued(&view, everywhere))
    }
}

impl SlashCommand for RescanCommand {
    fn definition(&self) -> Command {
        let windows: Vec<(&str, &str)> = WINDOWS
            .iter()
            .map(|window| (window_label(window), *window))
            .collect();
        command(
            "rescan",
            "Re-read this channel's chat from Discord and propose any changes",
            false,
            vec![
                choices(
                    "window",
                    "How far back to read (default: this boss week)",
                    false,
                    &windows,
                ),
                choices(
                    "scope",
                    "This channel (default), or every watched channel",
                    false,
                    &[
                        ("this channel", "this-channel"),
                        ("all channels", "all-channels"),
                    ],
                ),
                flag("cancel", "Stop the rescan that is running"),
            ],
        )
    }

    fn gate(&self) -> Gate {
        Gate::BossingRole
    }

    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(self.rescan(invocation))
    }
}
