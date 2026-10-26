//! `/schedule`: a boss week's runs as one public Components V2 container
//! that pings nobody (`delivery::cards::redesign::schedule_components`); a
//! week over the V2 budget gets the embed (`schedule_embed`) instead.

use std::sync::Arc;

use twilight_model::application::command::Command;

use super::access::Gate;
use super::build::{choices, command, flag};
use super::context::{CommandContext, store_failed};
use super::dispatch::{CommandError, CommandFuture, SlashCommand};
use super::invocation::Invocation;
use super::lookup::{materialised_weeks, on_run};
use super::options::Args;
use crate::bot::delivery::cards::redesign::{
    NO_MARKS, ScheduleScope, ScheduleWeek, component_count, schedule_components, schedule_embed,
    text_chars, within_budget,
};
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;
use crate::domain::completion::RunEnds;
use crate::domain::members::MemberProfile;
use crate::domain::schedule::Run;
use crate::domain::scheduler::Scope;

fn empty_text(scope: &str) -> &'static str {
    match scope {
        "all" => "Nothing still to come. Add a baseline with `/fixed add`.",
        "mine" => "You have nothing left this week. `/schedule scope:all` shows everyone's.",
        _ => {
            "Nothing left in this channel this week. `/schedule scope:mine` shows yours, \
             `scope:all` the whole guild's."
        }
    }
}

/// v4 `_member_name`: nickname, display name, else the id.
fn member_name(profiles: &[MemberProfile], user_id: &str) -> String {
    profiles
        .iter()
        .find(|profile| profile.member.user_id == user_id)
        .and_then(|profile| {
            [&profile.member.nickname, &profile.member.display_name]
                .into_iter()
                .flatten()
                .find(|name| !name.is_empty())
                .cloned()
        })
        .unwrap_or_else(|| user_id.to_owned())
}

pub struct ScheduleCommand {
    ctx: Arc<CommandContext>,
}

impl ScheduleCommand {
    pub fn new(ctx: Arc<CommandContext>) -> Self {
        Self { ctx }
    }

    /// When runs end now: the saved run lengths (defaults without a desk).
    fn run_ends(&self) -> RunEnds {
        let catalog = Some(Arc::clone(&self.ctx.catalog));
        let policy = self.ctx.policy.clone();
        match &self.ctx.config {
            Some(desk) => desk.run_ends(Arc::clone(&self.ctx.catalog), policy).now(),
            None => RunEnds::new(Default::default(), catalog, policy),
        }
    }

    async fn schedule(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let channel = invocation.channel_id.map(id_text).unwrap_or_default();
        let default_scope = if self.ctx.channels.is_watched(&channel) {
            "channel"
        } else {
            "mine"
        };
        let scope = args.text("scope").unwrap_or(default_scope);
        let weeks = materialised_weeks(&self.ctx, self.ctx.now())?;
        let week = weeks[usize::from(args.text("week") == Some("next"))];
        let snapshot = self
            .ctx
            .store
            .snapshot(Scope::Weeks(vec![week]))
            .await
            .map_err(store_failed)?;
        let profiles = self.ctx.profiles().await?;
        let user = id_text(invocation.invoker.user_id);
        let everything: Vec<&Run> = snapshot
            .runs
            .iter()
            .filter(|run| run.week_start == week)
            .filter(|run| scope != "mine" || on_run(run, &user))
            .filter(|run| scope != "channel" || run.channel_id.as_deref() == Some(&channel))
            .collect();
        let show_past = args.flag("show_past").unwrap_or(false);
        // Live runs past their end are frozen until settled: shown as past.
        let ends = self.run_ends();
        let now = self.ctx.now();
        let ended: std::collections::BTreeSet<String> = everything
            .iter()
            .filter(|run| ends.frozen(run, now))
            .map(|run| run.id.clone())
            .collect();
        let name = self.ctx.channels.name(&channel);
        let view = ScheduleWeek {
            snapshot: &snapshot,
            runs: &everything,
            show_past,
            week_start: week,
            next: args.text("week") == Some("next"),
            scope: match scope {
                "mine" => ScheduleScope::Mine,
                "all" => ScheduleScope::All,
                _ => ScheduleScope::Channel(name.as_deref()),
            },
            zone: self.ctx.policy.zone(),
            catalog: Some(self.ctx.catalog.as_ref()),
            marks: &NO_MARKS,
            empty: empty_text(scope),
            ended: &ended,
        };
        let name = |user: &str| member_name(&profiles, user);
        let components = schedule_components(&view, &name);
        if within_budget(&components) {
            return Ok(InteractionReply::v2(components, false));
        }
        kanade_log::log!(
            warn,
            "schedule_v2_over_budget",
            components = component_count(&components),
            chars = text_chars(&components),
            fallback = "embed",
        );
        let embed = schedule_embed(&view, &name);
        Ok(InteractionReply::public("").with_embed(embed))
    }
}

impl SlashCommand for ScheduleCommand {
    fn definition(&self) -> Command {
        command(
            "schedule",
            "Show the boss schedule for a week",
            false,
            vec![
                choices(
                    "scope",
                    "`channel` (default in a party channel), `mine`, or `all`",
                    false,
                    &[("mine", "mine"), ("all", "all"), ("channel", "channel")],
                ),
                choices(
                    "week",
                    "`this` (default) or `next`",
                    false,
                    &[("this", "this"), ("next", "next")],
                ),
                flag(
                    "show_past",
                    "Include runs that already happened, and cancelled ones",
                ),
            ],
        )
    }

    fn gate(&self) -> Gate {
        Gate::BossingRole
    }

    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(self.schedule(invocation))
    }
}
