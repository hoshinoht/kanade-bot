//! `/character link|unlink|data|share` (module `character_data`; registered
//! after the retained commands only while the module is on, so the guild
//! bulk overwrite removes it when off). Open to every member (`Gate::Anyone`):
//! the roster is the ownership proof, and seeing or deleting one's own data
//! must never depend on a role. Every reply is ephemeral except the share
//! card, which the bot posts itself. Every read goes through
//! `CharacterData::member_characters` (`current_link` / `data`, fail-closed).
//!
//! The notice's Link / Cancel buttons come back as internal subcommands
//! (`link_press`, `cancel_press`) and replace the notice in place.

mod link;
mod manage;

use std::sync::Arc;

use twilight_model::application::command::{Command, CommandOptionChoice};

use super::access::Gate;
use super::build::{command, picked, subcommand};
use super::context::{CommandContext, store_failed};
use super::dispatch::{ChoicesFuture, CommandError, CommandFuture, SlashCommand, choice};
use super::invocation::Invocation;
use crate::bot::chardata::{DiscordPosts, copy};
use crate::bot::ids::id_text;
use crate::bot::transport::{DiscordTransport, InteractionReply};
use crate::infrastructure::character_data::{CharacterData, MemberCharacter};
use kanade_domain::characters::ign_key;
use kanade_domain::chardata::LinkStatus;

/// The notice's buttons, as internal subcommands.
pub const LINK_PRESS: &str = "link_press";
pub const CANCEL_PRESS: &str = "cancel_press";

/// What `/character` works with besides the shared context.
pub struct CharacterParts<T> {
    pub service: Arc<CharacterData>,
    pub posts: Arc<DiscordPosts<T>>,
    /// `https://<public host>/privacy`; `None` without a public listener
    /// (the notice then names no page).
    pub privacy_url: Option<String>,
}

// By hand: `T` itself need not be `Clone`.
impl<T> Clone for CharacterParts<T> {
    fn clone(&self) -> Self {
        Self {
            service: Arc::clone(&self.service),
            posts: Arc::clone(&self.posts),
            privacy_url: self.privacy_url.clone(),
        }
    }
}

pub struct CharacterCommand<T> {
    ctx: Arc<CommandContext>,
    service: Arc<CharacterData>,
    posts: Arc<DiscordPosts<T>>,
    privacy_url: Option<String>,
}

impl<T> CharacterCommand<T> {
    pub fn new(ctx: Arc<CommandContext>, parts: CharacterParts<T>) -> Self {
        Self {
            ctx,
            service: parts.service,
            posts: parts.posts,
            privacy_url: parts.privacy_url,
        }
    }

    async fn mine(&self, invocation: &Invocation) -> Result<Vec<MemberCharacter>, CommandError> {
        self.service
            .member_characters(&id_text(invocation.invoker.user_id))
            .await
            .map_err(store_failed)
    }
}

/// The character `typed` names: an autocomplete value (its id), else its
/// IGN in any letter case.
fn resolve<'a>(characters: &'a [MemberCharacter], typed: &str) -> Option<&'a MemberCharacter> {
    let typed = typed.trim();
    characters
        .iter()
        .find(|entry| entry.character.id == typed)
        .or_else(|| {
            let key = ign_key(typed);
            characters
                .iter()
                .find(|entry| entry.character.ign_key() == key)
        })
}

fn reply(text: impl Into<String>) -> Result<InteractionReply, CommandError> {
    Ok(InteractionReply::ephemeral(text))
}

impl<T: DiscordTransport + 'static> SlashCommand for CharacterCommand<T> {
    fn definition(&self) -> Command {
        command(
            "character",
            "Your characters' NEXON Open API data: link, see, share or delete it",
            false,
            vec![
                subcommand(
                    "link",
                    "Link one of your roster characters to its NEXON Open API data",
                    vec![picked(
                        "character",
                        "One of your characters on the roster",
                        true,
                    )],
                ),
                subcommand(
                    "unlink",
                    "Delete a linked character's NEXON data now",
                    vec![picked("character", "A linked character, or All", true)],
                ),
                subcommand(
                    "data",
                    "See everything Kanade keeps for you from NEXON",
                    Vec::new(),
                ),
                subcommand(
                    "share",
                    "Post a linked character's card in this channel",
                    vec![picked("character", "One of your linked characters", true)],
                ),
            ],
        )
    }

    fn gate(&self) -> Gate {
        Gate::Anyone
    }

    /// The share card and the first fetch can take longer than 3 s.
    fn defer(&self) -> Option<bool> {
        Some(true)
    }

    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async move {
            match invocation.path.get(1).map(String::as_str) {
                Some("link") => self.link(invocation).await,
                Some("unlink") => self.unlink(invocation).await,
                Some("data") => self.data(invocation).await,
                Some("share") => self.share(invocation).await,
                Some(LINK_PRESS) => self.link_press(invocation).await,
                Some(CANCEL_PRESS) => self.cancel_press(invocation).await,
                _ => Err(CommandError::Internal("unknown /character path".into())),
            }
        })
    }

    fn autocomplete<'a>(&'a self, invocation: &'a Invocation) -> ChoicesFuture<'a> {
        Box::pin(async move { self.choices(invocation).await.unwrap_or_default() })
    }
}

impl<T> CharacterCommand<T> {
    /// The `character:` choices of `link` (those waiting for the member
    /// first, then the rest, each with its state), `unlink` (linked, stale
    /// and pending, then All) and `share` (linked ones).
    async fn choices(
        &self,
        invocation: &Invocation,
    ) -> Result<Vec<CommandOptionChoice>, CommandError> {
        let Some(("character", typed)) = invocation.focused() else {
            return Ok(Vec::new());
        };
        let typed = typed.trim().to_ascii_lowercase();
        let characters = self.mine(invocation).await?;
        let matching = characters
            .iter()
            .filter(|entry| entry.character.ign.to_ascii_lowercase().contains(&typed));
        let label = |entry: &MemberCharacter, state: &str| {
            copy::pick_other(
                &entry.character.ign,
                entry.character.class.as_deref(),
                state,
            )
        };
        let state = |entry: &MemberCharacter| match entry.link.as_ref().map(|link| link.link.status)
        {
            None => "not linked",
            Some(LinkStatus::Active) => "linked",
            Some(LinkStatus::Stale) => "stale, link again",
            Some(LinkStatus::Pending) => "waiting for you",
        };
        let mut out = Vec::new();
        match invocation.path.get(1).map(String::as_str) {
            Some("link") => {
                let (mut waiting, mut others): (Vec<_>, Vec<_>) = matching
                    .filter(|entry| entry.linkable())
                    .partition(|entry| {
                        entry
                            .link
                            .as_ref()
                            .is_some_and(|link| link.link.status == LinkStatus::Pending)
                    });
                waiting.append(&mut others);
                out.extend(
                    waiting.into_iter().map(|entry| {
                        choice(&label(entry, state(entry)), entry.character.id.clone())
                    }),
                );
            }
            Some("unlink") => {
                let linked: Vec<_> = characters
                    .iter()
                    .filter(|entry| entry.link.is_some())
                    .collect();
                for entry in linked
                    .iter()
                    .filter(|entry| entry.character.ign.to_ascii_lowercase().contains(&typed))
                {
                    let state = match state(entry) {
                        "waiting for you" => "pending",
                        "stale, link again" => "stale",
                        other => other,
                    };
                    out.push(choice(&label(entry, state), entry.character.id.clone()));
                }
                if !linked.is_empty() {
                    out.push(choice("All", copy::ALL));
                }
            }
            Some("share") => {
                for entry in matching.filter(|entry| {
                    entry.linkable()
                        && entry
                            .link
                            .as_ref()
                            .is_some_and(|link| link.link.status == LinkStatus::Active)
                }) {
                    out.push(choice(&label(entry, "linked"), entry.character.id.clone()));
                }
            }
            _ => {}
        }
        Ok(out)
    }
}
