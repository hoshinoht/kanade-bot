//! A member's own settings: `/pings` (mention level) and `/style` (reply
//! style), plus staff-only `/nick` (chat aliases). Members are not history
//! rows: these are portal-owned member edits, as the admin API makes them.

use std::sync::Arc;

use twilight_model::application::command::Command;

use super::access::Gate;
use super::build::{choices, command, picked, text, user};
use super::context::{CommandContext, store_failed};
use super::dispatch::{
    ChoicesFuture, CommandError, CommandFuture, MAX_CHOICES, SlashCommand, choice,
};
use super::invocation::Invocation;
use super::options::Args;
use super::text::mention;
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;
use crate::domain::members::{GatewayMember, MemberProfile, PingLevel, PortalEdit, is_valid_alias};
use crate::domain::scheduler::StoreError;

/// v4 `PING_LEVEL_HELP`, in the command's own words.
pub fn ping_help(level: PingLevel) -> &'static str {
    match level {
        PingLevel::Essential => {
            "only when you need to answer — the morning card, the countdowns you haven't ✅'d, \
             a card waiting on your ✅, and someone dropping out of your run"
        }
        PingLevel::All => {
            "everything that lists you, including moves, swaps and weekly-timing changes"
        }
        PingLevel::Off => "never — you'll still be named in every post, just not notified",
    }
}

/// v4 `STAFF_LIMITS_REPLY` lives with `/limits`; `/style` refuses with this.
const NO_CHAT_ACCESS: &str = "You need chatbot access to set a reply style.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Pings,
    Style,
    Nick,
}

pub struct MemberCommand {
    ctx: Arc<CommandContext>,
    kind: Kind,
}

impl MemberCommand {
    pub fn pings(ctx: Arc<CommandContext>) -> Self {
        Self {
            ctx,
            kind: Kind::Pings,
        }
    }

    pub fn style(ctx: Arc<CommandContext>) -> Self {
        Self {
            ctx,
            kind: Kind::Style,
        }
    }

    pub fn nick(ctx: Arc<CommandContext>) -> Self {
        Self {
            ctx,
            kind: Kind::Nick,
        }
    }

    /// v4 upserts a row for a role member the roster has not synced yet.
    async fn own_row(&self, invocation: &Invocation) -> Result<(), CommandError> {
        let invoker = &invocation.invoker;
        self.ctx
            .members
            .ensure(GatewayMember {
                user_id: id_text(invoker.user_id),
                display_name: invocation.invoker_name.clone(),
                nickname: None,
                has_role: invoker
                    .roles
                    .contains(&self.ctx.access.policy.bossing_role_id),
                is_bot: false,
                roles: invoker.roles.iter().map(|role| id_text(*role)).collect(),
                is_guild_admin: invoker.is_guild_admin,
            })
            .await
            .map_err(store_failed)
    }

    async fn edit(&self, user_id: String, edit: PortalEdit) -> Result<MemberProfile, CommandError> {
        match self.ctx.store.edit_member(user_id.clone(), edit).await {
            Ok(Some(profile)) => Ok(profile),
            Ok(None) => Err(CommandError::User(format!(
                "{} isn't on the roster yet - try again in a minute.",
                mention(&user_id)
            ))),
            Err(StoreError::Constraint(_)) => Err(CommandError::User(
                "That alias already names someone.".into(),
            )),
            Err(error) => Err(store_failed(error)),
        }
    }

    async fn run_pings(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        self.own_row(invocation).await?;
        let user = id_text(invocation.invoker.user_id);
        let Some(level) = Args(&invocation.options).text("level") else {
            let current = self
                .ctx
                .store
                .member(user)
                .await
                .map_err(store_failed)?
                .map_or(PingLevel::DEFAULT, |profile| profile.member.ping_level);
            let others: Vec<String> = PingLevel::ALL
                .iter()
                .filter(|level| **level != current)
                .map(|level| format!("`{}`", level.as_str()))
                .collect();
            return Ok(InteractionReply::ephemeral(format!(
                "🔔 You're on **{}** — {}.\n`/pings level:` to change it ({}).",
                current.as_str(),
                ping_help(current),
                others.join(" · ")
            )));
        };
        let chosen = PingLevel::parse_stored(level)
            .map_err(|error| CommandError::User(error.to_string()))?;
        self.edit(
            user,
            PortalEdit {
                ping_level: Some(chosen),
                ..PortalEdit::default()
            },
        )
        .await?;
        Ok(InteractionReply::ephemeral(format!(
            "🔔 Pings set to **{}** — {}.",
            chosen.as_str(),
            ping_help(chosen)
        )))
    }

    /// Selectable reply styles; `default` is offered separately.
    fn public_styles(&self) -> Vec<String> {
        self.ctx
            .config
            .as_ref()
            .map(|config| {
                config
                    .profile_choices()
                    .options
                    .into_iter()
                    .map(|persona| persona.key)
                    .filter(|key| key != "default")
                    .collect()
            })
            .unwrap_or_default()
    }

    /// v4 `_require_chat_access`: staff, or the chat pilot role.
    fn has_chat_access(&self, invocation: &Invocation) -> bool {
        let access = &self.ctx.access;
        access
            .policy
            .is_staff(&invocation.invoker, invocation.owner_id)
            || access.pilot_role.as_ref().is_some_and(|role| {
                invocation
                    .invoker
                    .roles
                    .iter()
                    .any(|held| id_text(*held) == *role)
            })
    }

    async fn run_style(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        if !self.has_chat_access(invocation) {
            return Err(CommandError::User(NO_CHAT_ACCESS.into()));
        }
        self.own_row(invocation).await?;
        let user = id_text(invocation.invoker.user_id);
        let public = self.public_styles();
        let Some(profile) = Args(&invocation.options).text("profile") else {
            let current = self
                .ctx
                .store
                .member(user)
                .await
                .map_err(store_failed)?
                .and_then(|profile| profile.reply_style);
            let saved = format!("**{}**", current.as_deref().unwrap_or("default"));
            let unavailable = current
                .as_ref()
                .is_some_and(|style| !public.contains(style));
            let suffix = if unavailable {
                " (currently unavailable)"
            } else {
                ""
            };
            let available = if public.is_empty() {
                ".".to_owned()
            } else {
                let listed: Vec<String> = public.iter().map(|name| format!("`{name}`")).collect();
                format!(", {}.", listed.join(", "))
            };
            return Ok(InteractionReply::ephemeral(format!(
                "Your saved reply style is {saved}{suffix}. Available: `default`{available}"
            )));
        };
        let requested = profile.trim().to_lowercase();
        let chosen = if requested == "default" {
            None
        } else if public.contains(&requested) {
            Some(requested)
        } else {
            return Ok(InteractionReply::ephemeral(
                "That reply style is not available. Use `/style` to see the public choices.",
            ));
        };
        self.edit(
            user,
            PortalEdit {
                reply_style: Some(chosen.clone()),
                ..PortalEdit::default()
            },
        )
        .await?;
        Ok(InteractionReply::ephemeral(format!(
            "Reply style preference saved as **{}**.",
            chosen.as_deref().unwrap_or("default")
        )))
    }

    async fn run_nick(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let target = args.user("user").unwrap_or_default();
        let alias = args.text("alias").unwrap_or_default().trim().to_lowercase();
        if alias.is_empty() {
            return Err(CommandError::User("Alias can't be empty.".into()));
        }
        // The store keeps aliases as one lowercase word (the extractor matches them so).
        if !is_valid_alias(&alias) {
            return Err(CommandError::User(
                "An alias is one word, e.g. `MY`.".into(),
            ));
        }
        let profile = self
            .edit(
                target.clone(),
                PortalEdit {
                    add_alias: Some(alias),
                    ..PortalEdit::default()
                },
            )
            .await?;
        let aliases: Vec<String> = profile
            .aliases
            .iter()
            .map(|alias| format!("`{alias}`"))
            .collect();
        Ok(InteractionReply::ephemeral(format!(
            "✅ {} is now also known as: {}",
            mention(&target),
            aliases.join(", ")
        )))
    }
}

impl SlashCommand for MemberCommand {
    fn definition(&self) -> Command {
        match self.kind {
            Kind::Pings => {
                let levels: Vec<(String, &str)> = PingLevel::ALL
                    .iter()
                    .map(|level| {
                        (
                            format!("{} — {}", level.as_str(), ping_help(*level)),
                            level.as_str(),
                        )
                    })
                    .collect();
                let levels: Vec<(&str, &str)> = levels
                    .iter()
                    .map(|(label, value)| (label.as_str(), *value))
                    .collect();
                command(
                    "pings",
                    "Choose how much the bot @mentions you",
                    false,
                    vec![choices(
                        "level",
                        "Leave this empty to see what you're on now",
                        false,
                        &levels,
                    )],
                )
            }
            Kind::Style => command(
                "style",
                "Choose how the bot replies to you",
                false,
                vec![picked(
                    "profile",
                    "Leave empty to see your saved choice; use default to reset",
                    false,
                )],
            ),
            Kind::Nick => command(
                "nick",
                "Attach a chat alias to a member (admins only)",
                true,
                vec![
                    user("user", "The member", true),
                    text("alias", "What they get called in chat, e.g. `MY`", true),
                ],
            ),
        }
    }

    fn gate(&self) -> Gate {
        match self.kind {
            Kind::Pings => Gate::BossingRole,
            Kind::Style => Gate::Anyone,
            // User decision (2026-09-25): staff only in v5.
            Kind::Nick => Gate::Staff,
        }
    }

    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async move {
            match self.kind {
                Kind::Pings => self.run_pings(invocation).await,
                Kind::Style => self.run_style(invocation).await,
                Kind::Nick => self.run_nick(invocation).await,
            }
        })
    }

    fn autocomplete<'a>(&'a self, invocation: &'a Invocation) -> ChoicesFuture<'a> {
        Box::pin(async move {
            let (Kind::Style, Some(("profile", typed))) = (self.kind, invocation.focused()) else {
                return Vec::new();
            };
            if !self.has_chat_access(invocation) {
                return Vec::new();
            }
            let term = typed.trim().to_lowercase();
            std::iter::once("default".to_owned())
                .chain(self.public_styles())
                .filter(|name| name.to_lowercase().contains(&term))
                .take(MAX_CHOICES)
                .map(|name| choice(&name, name.clone()))
                .collect()
        })
    }
}
