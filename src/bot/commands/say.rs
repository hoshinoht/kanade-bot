//! `/say` (staff): post the invoker's words verbatim where the bot can
//! already post, never falling back to another channel (v4 `commands.say`).
//! Only users written as `<@id>` are notified; the allow-list never holds
//! roles or `@everyone`/`@here` (v5 mention rule, see `bot/mentions.rs`).

use std::sync::{Arc, LazyLock};

use regex::Regex;
use twilight_model::application::command::Command;

use super::access::Gate;
use super::build::{command, text, text_channel};
use super::context::CommandContext;
use super::dispatch::{CommandError, CommandFuture, SlashCommand};
use super::invocation::Invocation;
use super::options::Args;
use super::split::{CONTENT_LIMIT, units};
use crate::bot::ids::{id_text, parse_id};
use crate::bot::mentions;
use crate::bot::transport::{
    DiscordTransport, InteractionReply, Outcome, OutgoingMessage, RejectionKind,
};

/// v4 `SAY_LIMIT`: room under Discord's 2000 characters.
pub const SAY_LIMIT: usize = 1900;
const UNCONFIRMED: &str = "⚠️ Discord did not confirm delivery. Check the channel before retrying.";

static USER_MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<@!?(\d+)>").expect("pattern"));

/// v4 `mentions_in` users: what Discord renders as a user mention, in
/// order, once each. A bare number is not a mention.
pub fn mentioned_users(text: &str) -> Vec<String> {
    let mut users: Vec<String> = Vec::new();
    for found in USER_MENTION.captures_iter(text) {
        if !users.iter().any(|seen| seen == &found[1]) {
            users.push(found[1].to_owned());
        }
    }
    users
}

pub struct SayCommand<T> {
    ctx: Arc<CommandContext>,
    transport: Arc<T>,
}

impl<T> SayCommand<T> {
    pub fn new(ctx: Arc<CommandContext>, transport: Arc<T>) -> Self {
        Self { ctx, transport }
    }

    /// v4 `BossBot.no_access`.
    fn no_access(&self, channel_id: &str) -> String {
        let place = self.ctx.channels.name(channel_id).map_or_else(
            || format!("channel {channel_id}"),
            |name| format!("#{name}"),
        );
        let who = self.ctx.bot_name.as_deref().unwrap_or("the bot");
        format!(
            "the bot has no access to {place} - grant the {who} role View Channel + Send \
             Messages there"
        )
    }
}

impl<T: DiscordTransport> SayCommand<T> {
    async fn say(&self, invocation: &Invocation) -> Result<InteractionReply, CommandError> {
        let args = Args(&invocation.options);
        let target = args
            .channel("channel")
            .or_else(|| invocation.channel_id.map(id_text))
            .unwrap_or_default();
        let text = args.text("message").unwrap_or_default().trim();
        if text.is_empty() {
            return Err(CommandError::User("Nothing to say.".into()));
        }
        let length = text.chars().count();
        // Never split: a /say is one message. UTF-16 units bound what Discord counts.
        if length > SAY_LIMIT || units(text) > CONTENT_LIMIT {
            return Err(CommandError::User(format!(
                "That's {length} characters; keep it under {SAY_LIMIT}."
            )));
        }
        let channel = parse_id(&target)
            .filter(|_| self.ctx.channels.can_send(&target))
            .ok_or_else(|| CommandError::User(self.no_access(&target)))?;
        let users = mentioned_users(text);
        let message = OutgoingMessage {
            content: Some(text.to_owned()),
            embeds: Vec::new(),
            allowed_mentions: mentions::allow_users(&users),
            reply_to: None,
            attachments: Vec::new(),
        };
        match self.transport.create_message(channel, &message).await {
            Outcome::Delivered(_) => {}
            Outcome::DefinitelyRejected(
                RejectionKind::MissingAccess
                | RejectionKind::MissingPermissions
                | RejectionKind::UnknownChannel,
            ) => return Err(CommandError::User(self.no_access(&target))),
            Outcome::DefinitelyRejected(_) | Outcome::Ambiguous(_) => {
                return Ok(InteractionReply::ephemeral(UNCONFIRMED));
            }
        }
        let notified = if users.is_empty() {
            "notifying nobody".to_owned()
        } else {
            format!("notifying {} member(s)", users.len())
        };
        Ok(InteractionReply::ephemeral(format!(
            "✅ Posted in <#{target}> ({notified})."
        )))
    }
}

impl<T: DiscordTransport + 'static> SlashCommand for SayCommand<T> {
    fn definition(&self) -> Command {
        command(
            "say",
            "Post a message as the bot (admins only)",
            true,
            vec![
                text("message", "What the bot should post, word for word", true),
                text_channel("channel", "Where to post it (default: this channel)"),
            ],
        )
    }

    fn gate(&self) -> Gate {
        Gate::Staff
    }

    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(self.say(invocation))
    }
}
