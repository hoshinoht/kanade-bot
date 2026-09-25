//! `/debug status`, the first command on the v5 path. It is read-only and
//! needs no storage, so it proves registration, gating and replies end to
//! end. It reports only what the adapter knows; v4's heartbeat, roster, run
//! and provider lines return when their owners land.

use tokio::time::Instant;
use twilight_model::application::command::{
    Command, CommandOption, CommandOptionType, CommandType,
};
use twilight_model::application::interaction::InteractionContextType;
use twilight_model::guild::Permissions;
use twilight_model::id::Id;

use super::access::Gate;
use super::dispatch::{CommandError, CommandFuture, SlashCommand};
use super::invocation::Invocation;
use crate::bot::transport::InteractionReply;

/// v4 `format_uptime`: whole minutes, with hours and days when present.
pub fn format_uptime(seconds: u64) -> String {
    let (days, rem) = (seconds / 86_400, seconds % 86_400);
    let (hours, minutes) = (rem / 3_600, rem % 3_600 / 60);
    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

/// The `/debug` group, currently holding only `status`.
#[derive(Clone, Copy, Debug)]
pub struct DebugCommand {
    started: Instant,
}

impl DebugCommand {
    /// `started` is the process start on the runtime's monotonic clock.
    pub fn new(started: Instant) -> Self {
        Self { started }
    }

    fn status(&self) -> InteractionReply {
        let uptime = format_uptime(self.started.elapsed().as_secs());
        InteractionReply::ephemeral(format!("**uptime** {uptime}\n**storage** not connected"))
    }
}

impl SlashCommand for DebugCommand {
    // `dm_permission` is deprecated in favour of `contexts` but is a required
    // field of the struct; it stays unset.
    #[allow(deprecated)]
    fn definition(&self) -> Command {
        Command {
            application_id: None,
            contexts: Some(vec![InteractionContextType::Guild]),
            // Hidden from ordinary members' pickers; the gate still decides.
            default_member_permissions: Some(Permissions::ADMINISTRATOR),
            dm_permission: None,
            description: "Testing aids (admins only)".to_owned(),
            description_localizations: None,
            guild_id: None,
            id: None,
            integration_types: None,
            kind: CommandType::ChatInput,
            name: "debug".to_owned(),
            name_localizations: None,
            nsfw: None,
            options: vec![subcommand("status", "Bot health and configuration")],
            version: Id::new(1),
        }
    }

    fn gate(&self) -> Gate {
        Gate::Debug
    }

    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a> {
        Box::pin(async move {
            match invocation.path.get(1).map(String::as_str) {
                Some("status") => Ok(self.status()),
                other => Err(CommandError::Internal(format!(
                    "unknown /debug subcommand {other:?}"
                ))),
            }
        })
    }
}

fn subcommand(name: &str, description: &str) -> CommandOption {
    CommandOption {
        autocomplete: None,
        channel_types: None,
        choices: None,
        description: description.to_owned(),
        description_localizations: None,
        kind: CommandOptionType::SubCommand,
        max_length: None,
        max_value: None,
        min_length: None,
        min_value: None,
        name: name.to_owned(),
        name_localizations: None,
        options: None,
        required: None,
    }
}
