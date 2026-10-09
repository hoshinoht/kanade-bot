//! `/fixed add|list|edit|remove`: the weekly baseline timings (v4
//! `FixedGroup`), through the shared scheduler writer.

use std::sync::Arc;

use twilight_model::application::command::{Command, CommandOption};

use super::access::{Gate, Invoker};
use super::build::{choices, command, picked, subcommand, text, text_channel, user};
use super::context::CommandContext;
use super::dispatch::{ChoicesFuture, CommandError, CommandFuture, SlashCommand};
use super::invocation::Invocation;
use super::lookup::{can_modify_fixed, fixed_choices, on_fixed, refused, resolve};
use super::options::Args;
use super::participants::resolve_participants;
use super::text::fixed_run_line;
use crate::api::write::WriteContext;
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;
use crate::domain::history::{Expect, Origin, RowKey};
use crate::domain::ids::short_id;
use crate::domain::schedule::{
    FixedEdit, FixedEditChoices, FixedEditRequest, FixedRun, NewFixedRun,
};
use crate::domain::scheduler::{SchedulerError, Scope};
use crate::domain::weeks::{parse_hhmm, parse_weekday};

const DAYS: [(&str, &str); 7] = [
    ("Mon", "mon"),
    ("Tue", "tue"),
    ("Wed", "wed"),
    ("Thu", "thu"),
    ("Fri", "fri"),
    ("Sat", "sat"),
    ("Sun", "sun"),
];

const PICK_ID: &str = "Pick from the dropdown, or paste an id like `a1b2c3d4`";
const PARTICIPANTS: &str = "Extra people by name, e.g. `MY, alvin` - the pickers above are easier";
const MEMBERS: [&str; 6] = [
    "member1", "member2", "member3", "member4", "member5", "member6",
];

fn member_pickers(first: &str) -> Vec<CommandOption> {
    MEMBERS
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let description = if index == 0 {
                first
            } else {
                "Someone else on the run"
            };
            user(name, description, false)
        })
        .collect()
}

fn user_error(message: impl Into<String>) -> CommandError {
    CommandError::User(message.into())
}

pub struct FixedCommand {
    ctx: Arc<CommandContext>,
}

impl FixedCommand {
    pub fn new(ctx: Arc<CommandContext>) -> Self {
        Self { ctx }
    }

    async fn timings(&self) -> Result<Vec<FixedRun>, CommandError> {
        Ok(self
            .ctx
            .store
            .snapshot(Scope::Weeks(Vec::new()))
            .await
            .map_err(super::context::store_failed)?
            .fixed_runs)
    }

    async fn timing(&self, id: &str) -> Result<FixedRun, CommandError> {
        self.timings()
            .await?
            .into_iter()
            .find(|fixed| fixed.id == id)
            .ok_or_else(|| CommandError::Internal(format!("fixed run {id} vanished")))
    }

    /// The directory for this write, with the channels the command checked
    /// as watched (a thread counts under its watched parent, as in v4).
    async fn write_context(&self, watched: &[&str]) -> Result<WriteContext, CommandError> {
        let (mut ctx, _) = self.ctx.write_context().await?;
        for channel in watched {
            ctx.directory.watch(channel);
        }
        Ok(ctx)
    }

    fn picked(args: Args<'_>) -> Vec<String> {
        MEMBERS.iter().filter_map(|name| args.user(name)).collect()
    }

    async fn add(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let bosses = self
            .ctx
            .catalog
            .parse(args.text("bosses").unwrap_or_default())
            .map_err(|error| user_error(error.to_string()))?;
        let time = parse_hhmm(args.text("time").unwrap_or_default())
            .map_err(|error| user_error(error.to_string()))?;
        let weekday = parse_weekday(args.text("day").unwrap_or_default())
            .map_err(|error| user_error(error.to_string()))?;
        let channel = invocation.channel_id.map(id_text).unwrap_or_default();
        if !self.ctx.channels.is_watched(&channel) {
            return Err(user_error(
                "This channel isn't watched, so a run here would never get its pings. \
                 Run `/fixed add` in your party's channel, or add this channel to \
                 `CHAT_CHANNEL_IDS` / its category to `CHAT_CATEGORY_IDS`.",
            ));
        }
        let profiles = self.ctx.profiles().await?;
        let participants =
            resolve_participants(&profiles, args.text("participants"), &Self::picked(args))
                .map_err(user_error)?;
        let invoker = id_text(invocation.invoker.user_id);
        let new = NewFixedRun {
            owner_id: invoker.clone(),
            channel_id: Some(channel.clone()),
            bosses,
            weekday,
            time,
            participants: participants.clone(),
            note: args.text("note").map(str::to_owned),
        };
        let ctx = self.write_context(&[&channel]).await?;
        let origin = self
            .ctx
            .origin(&invocation.invoker, invocation.interaction.id);
        // One commit: the timing and its runs in the materialised weeks.
        let fixed_id = match self.ctx.writer.add_fixed(origin.clone(), new, &ctx).await {
            Ok(id) => id,
            Err(SchedulerError::AlreadyApplied { .. }) => self.created(&origin).await?,
            Err(error) => return Err(refused(error)),
        };
        let fixed = self.timing(&fixed_id).await?;
        // Only listed participants are pinged.
        let not_on_it = if participants.contains(&invoker) {
            ""
        } else {
            "\n(you're the owner but not on this run — it won't ping you; \
             `/fixed edit` to add yourself)"
        };
        Ok(InteractionReply::ephemeral(format!(
            "✅ Fixed run `#{}` added — this channel is its home channel, so its pings land \
             here.\n{}{not_on_it}",
            short_id(&fixed_id),
            fixed_run_line(&fixed, &self.ctx.catalog)
        )))
    }

    /// The timing a redelivered `/fixed add` created the first time.
    async fn created(&self, origin: &Origin) -> Result<String, CommandError> {
        let request = origin.request_id.clone().unwrap_or_default();
        let record = self
            .ctx
            .store
            .recorded_change(origin.actor.clone(), request)
            .await
            .map_err(super::context::store_failed)?;
        record
            .and_then(|record| {
                record
                    .rows
                    .iter()
                    .find_map(|row| match (&row.key, &row.before) {
                        (RowKey::FixedRun(id), None) => Some(id.clone()),
                        _ => None,
                    })
            })
            .ok_or_else(|| CommandError::Internal("replayed add has no record".into()))
    }

    async fn list(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let only_mine = Args(&invocation.options).text("scope").unwrap_or("mine") == "mine";
        let user = id_text(invocation.invoker.user_id);
        let rows: Vec<FixedRun> = self
            .timings()
            .await?
            .into_iter()
            .filter(|fixed| !only_mine || on_fixed(fixed, &user))
            .collect();
        if rows.is_empty() {
            return Ok(InteractionReply::ephemeral(if only_mine {
                "You're not on any fixed run. `/fixed list scope:all` shows every party's."
            } else {
                "No fixed runs yet - add one with `/fixed add`."
            }));
        }
        let body: Vec<String> = rows
            .iter()
            .map(|fixed| fixed_run_line(fixed, &self.ctx.catalog))
            .collect();
        Ok(InteractionReply::ephemeral(body.join("\n")))
    }

    /// The timing named by `id` that the invoker may change.
    async fn editable(&self, invoker: &Invoker, raw: &str) -> Result<FixedRun, CommandError> {
        let timings = self.timings().await?;
        let id = resolve(
            raw,
            timings.iter().map(|fixed| fixed.id.as_str()),
            "fixed run",
        )?;
        let fixed = timings
            .into_iter()
            .find(|fixed| fixed.id == id)
            .ok_or_else(|| CommandError::Internal(format!("fixed run {id} vanished")))?;
        if !can_modify_fixed(
            &fixed,
            &id_text(invoker.user_id),
            self.ctx.is_admin(invoker),
        ) {
            return Err(user_error(format!(
                "You're not on fixed run `#{}`.",
                short_id(&id)
            )));
        }
        Ok(fixed)
    }

    async fn edit(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let fixed = self
            .editable(&invocation.invoker, args.text("id").unwrap_or_default())
            .await?;
        let mut edit = FixedEdit::default();
        if let Some(bosses) = args.text("bosses") {
            edit.bosses = Some(
                self.ctx
                    .catalog
                    .parse(bosses)
                    .map_err(|error| user_error(error.to_string()))?,
            );
        }
        if let Some(time) = args.text("time") {
            edit.time = Some(parse_hhmm(time).map_err(|error| user_error(error.to_string()))?);
        }
        if let Some(day) = args.text("day") {
            edit.weekday = Some(parse_weekday(day).map_err(|error| user_error(error.to_string()))?);
        }
        let picked = Self::picked(args);
        let typed = args.text("participants");
        if typed.is_some() || !picked.is_empty() {
            let profiles = self.ctx.profiles().await?;
            edit.participants =
                Some(resolve_participants(&profiles, typed, &picked).map_err(user_error)?);
        }
        let mut watched = Vec::new();
        if let Some(channel) = args.channel("channel") {
            if !self.ctx.channels.is_watched(&channel) {
                return Err(user_error(format!(
                    "<#{channel}> isn't a watched channel, so its runs would never get their \
                     pings."
                )));
            }
            watched.push(channel.clone());
            edit.channel_id = Some(channel);
        }
        if let Some(note) = args.text("note") {
            edit.note = Some(note.to_owned());
        }
        if edit == FixedEdit::default() {
            return Ok(InteractionReply::ephemeral("Nothing to change."));
        }
        let watched: Vec<&str> = watched.iter().map(String::as_str).collect();
        let ctx = self.write_context(&watched).await?;
        // v4's slash edit moves every live run, amended ones included.
        let request = FixedEditRequest {
            fixed_id: fixed.id.clone(),
            edit,
            choices: FixedEditChoices::UpdateAll,
        };
        let origin = self
            .ctx
            .origin(&invocation.invoker, invocation.interaction.id);
        match self
            .ctx
            .writer
            .edit_fixed(origin, Expect::default(), request, &ctx)
            .await
        {
            Ok(()) | Err(SchedulerError::AlreadyApplied { .. }) => {}
            Err(error) => return Err(refused(error)),
        }
        let updated = self.timing(&fixed.id).await?;
        Ok(InteractionReply::ephemeral(format!(
            "✅ Updated.\n{}",
            fixed_run_line(&updated, &self.ctx.catalog)
        )))
    }

    async fn remove(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let fixed = self
            .editable(
                &invocation.invoker,
                Args(&invocation.options).text("id").unwrap_or_default(),
            )
            .await?;
        let (ctx, _) = self.ctx.write_context().await?;
        let origin = self
            .ctx
            .origin(&invocation.invoker, invocation.interaction.id);
        let cancelled = match self.ctx.writer.retire_fixed(origin, &fixed.id, &ctx).await {
            Ok(count) => count,
            // The first delivery's count is not recorded.
            Err(SchedulerError::AlreadyApplied { .. }) => 0,
            Err(error) => return Err(refused(error)),
        };
        Ok(InteractionReply::ephemeral(format!(
            "🗑️ Fixed run `#{}` removed ({cancelled} upcoming run(s) cancelled).",
            short_id(&fixed.id)
        )))
    }
}

impl SlashCommand for FixedCommand {
    fn definition(&self) -> Command {
        let mut add = vec![
            text(
                "bosses",
                "e.g. `hstar, hfa` - each boss needs a difficulty prefix (e/n/h/c/x)",
                true,
            ),
            choices("day", "Day of the week the run happens", true, &DAYS),
            text("time", "Start time, HH:MM in the guild timezone", true),
        ];
        add.extend(member_pickers(
            "Someone on the run (include yourself if you're on it)",
        ));
        add.push(text("participants", PARTICIPANTS, false));
        add.push(text("note", "Optional note", false));

        let mut edit = vec![
            picked("id", PICK_ID, true),
            text("bosses", "New bosses, e.g. `hstar, hfa`", false),
            choices("day", "New day of the week", false, &DAYS),
            text("time", "New start time, HH:MM", false),
        ];
        edit.extend(member_pickers(
            "Replaces the whole participant list (you are NOT added automatically)",
        ));
        edit.push(text("participants", PARTICIPANTS, false));
        edit.push(text_channel(
            "channel",
            "Move the run's home channel - where its pings are posted",
        ));
        edit.push(text("note", "New note", false));

        command(
            "fixed",
            "Manage the weekly baseline boss timings",
            false,
            vec![
                subcommand("add", "Add a fixed weekly run", add),
                subcommand(
                    "list",
                    "List the fixed weekly runs",
                    vec![choices(
                        "scope",
                        "`mine` (default: on it or you own it) or `all`",
                        false,
                        &[("mine", "mine"), ("all", "all")],
                    )],
                ),
                subcommand("edit", "Edit a fixed weekly run", edit),
                subcommand(
                    "remove",
                    "Remove a fixed weekly run",
                    vec![picked("id", PICK_ID, true)],
                ),
            ],
        )
    }

    fn gate(&self) -> Gate {
        Gate::BossingRole
    }

    fn defer(&self) -> Option<bool> {
        Some(true)
    }

    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async move {
            match invocation.path.get(1).map(String::as_str) {
                Some("add") => self.add(invocation).await,
                Some("list") => self.list(invocation).await,
                Some("edit") => self.edit(invocation).await,
                Some("remove") => self.remove(invocation).await,
                other => Err(CommandError::Internal(format!(
                    "unknown /fixed subcommand {other:?}"
                ))),
            }
        })
    }

    fn autocomplete<'a>(&'a self, invocation: &'a Invocation) -> ChoicesFuture<'a> {
        Box::pin(async move {
            match invocation.focused() {
                Some(("id", typed)) => fixed_choices(&self.ctx, &invocation.invoker, typed).await,
                _ => Vec::new(),
            }
        })
    }
}
