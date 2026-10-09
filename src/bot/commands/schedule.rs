//! `/schedule`: a boss week's runs as a public embed that pings nobody
//! (v4 `commands.schedule`), or, while the live message style is
//! `redesigned`, the redesigned layout as one Components V2 container
//! (`delivery::cards::redesign::schedule_components`); a week over the V2
//! budget gets the redesigned embed (`schedule_embed`) instead.

use std::sync::Arc;

use serde_json::json;
use twilight_model::application::command::Command;
use twilight_model::channel::message::Embed;
use twilight_model::channel::message::embed::{EmbedField, EmbedFooter};

use super::access::Gate;
use super::build::{choices, command, flag};
use super::context::{CommandContext, store_failed};
use super::dispatch::{CommandError, CommandFuture, SlashCommand};
use super::invocation::Invocation;
use super::lookup::{materialised_weeks, on_run};
use super::options::Args;
use super::text::{group_by_day, local_day, roster_delta, schedule_line};
use crate::bot::delivery::cards::redesign::{
    NO_MARKS, ScheduleScope, ScheduleWeek, component_count, schedule_components, schedule_embed,
    text_chars, within_budget,
};
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;
use crate::domain::members::MemberProfile;
use crate::domain::schedule::{RsvpState, Run, ScheduleSnapshot};
use crate::domain::scheduler::Scope;
use crate::domain::settings::MessageStyle;
use crate::runtime::logging;

/// discord.py `Colour.blurple()`.
const BLURPLE: u32 = 0x5865F2;
const FOOTER: &str = "✅/❌ react on a reminder to RSVP · /amend to move a run";

fn hidden_note(hidden: usize) -> String {
    format!("{hidden} past/cancelled run(s) hidden — `show_past:True` to see them")
}

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

/// v4 `_roster_delta`: this week's party against its weekly timing.
fn delta(snapshot: &ScheduleSnapshot, run: &Run, profiles: &[MemberProfile]) -> String {
    let Some(fixed) = run
        .fixed_run_id
        .as_deref()
        .and_then(|id| snapshot.fixed_runs.iter().find(|fixed| fixed.id == id))
    else {
        return String::new();
    };
    let out: Vec<String> = fixed
        .participants
        .iter()
        .filter(|uid| !run.participants.contains(uid))
        .map(|uid| member_name(profiles, uid))
        .collect();
    let joined: Vec<String> = run
        .participants
        .iter()
        .filter(|uid| !fixed.participants.contains(uid))
        .map(|uid| member_name(profiles, uid))
        .collect();
    roster_delta(&out, &joined)
}

pub struct ScheduleCommand {
    ctx: Arc<CommandContext>,
}

impl ScheduleCommand {
    pub fn new(ctx: Arc<CommandContext>) -> Self {
        Self { ctx }
    }

    /// The live message style (the saved setting, read per command).
    fn style(&self) -> MessageStyle {
        self.ctx
            .config
            .as_ref()
            .map_or(MessageStyle::Classic, |desk| {
                desk.subscribe()
                    .borrow()
                    .settings
                    .notifications
                    .message_style
            })
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
        if self.style() == MessageStyle::Redesigned {
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
                attendance: self.ctx.policy.attendance.mode,
                catalog: Some(self.ctx.catalog.as_ref()),
                marks: &NO_MARKS,
                empty: empty_text(scope),
            };
            let name = |user: &str| member_name(&profiles, user);
            let components = schedule_components(&view, &name);
            if within_budget(&components) {
                return Ok(InteractionReply::v2(components, false));
            }
            logging::event(
                "WARN",
                "schedule_v2_over_budget",
                json!({
                    "components": component_count(&components),
                    "chars": text_chars(&components),
                    "fallback": "embed",
                }),
            );
            let embed = schedule_embed(&view, &name);
            return Ok(InteractionReply::public("").with_embed(embed));
        }
        let runs: Vec<&Run> = everything
            .iter()
            .copied()
            .filter(|run| show_past || run.status.is_live())
            .collect();
        let hidden = everything.len() - runs.len();

        let zone = self.ctx.policy.zone();
        let mut embed = Embed {
            author: None,
            color: Some(BLURPLE),
            description: None,
            fields: Vec::new(),
            footer: None,
            image: None,
            kind: "rich".to_owned(),
            provider: None,
            thumbnail: None,
            timestamp: None,
            title: Some(format!("Boss week of {} ({scope})", local_day(week, zone))),
            url: None,
            video: None,
        };
        if runs.is_empty() {
            let mut text = empty_text(scope).to_owned();
            if hidden > 0 {
                text.push_str(&format!("\n{}", hidden_note(hidden)));
            }
            embed.description = Some(text);
        } else {
            for (heading, day_runs) in group_by_day(&runs, zone) {
                let lines: Vec<String> = day_runs
                    .iter()
                    .map(|run| {
                        let rsvps: std::collections::BTreeMap<String, RsvpState> = snapshot
                            .rsvps
                            .iter()
                            .filter(|rsvp| rsvp.run_id == run.id)
                            .map(|rsvp| (rsvp.user_id.clone(), rsvp.state))
                            .collect();
                        schedule_line(run, zone, &rsvps, &delta(&snapshot, run, &profiles))
                    })
                    .collect();
                embed.fields.push(EmbedField {
                    inline: false,
                    name: heading,
                    value: lines.join("\n"),
                });
            }
            let mut footer = FOOTER.to_owned();
            if hidden > 0 {
                footer.push_str(&format!(" · {}", hidden_note(hidden)));
            }
            embed.footer = Some(EmbedFooter {
                icon_url: None,
                proxy_icon_url: None,
                text: footer,
            });
        }
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
