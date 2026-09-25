//! A slash-command interaction reduced to what dispatch needs.

use twilight_model::application::interaction::application_command::{
    CommandDataOption, CommandOptionValue,
};
use twilight_model::application::interaction::{Interaction, InteractionData, InteractionType};
use twilight_model::guild::Permissions;
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, GuildMarker},
};

use super::access::Invoker;
use crate::bot::transport::InteractionRef;

/// One guild slash-command invocation.
#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    pub interaction: InteractionRef,
    pub guild_id: Id<GuildMarker>,
    pub channel_id: Option<Id<ChannelMarker>>,
    pub invoker: Invoker,
    /// Command, then subcommand group/subcommand names.
    pub path: Vec<String>,
    /// The leaf command's options.
    pub options: Vec<CommandDataOption>,
}

impl Invocation {
    /// `None` for anything but a guild chat-input command with a member.
    pub fn from_interaction(interaction: &Interaction) -> Option<Self> {
        if interaction.kind != InteractionType::ApplicationCommand {
            return None;
        }
        let guild_id = interaction.guild_id?;
        let member = interaction.member.as_ref()?;
        let user_id = member.user.as_ref().map(|user| user.id)?;
        let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
            return None;
        };
        let mut path = vec![data.name.clone()];
        let mut options = data.options.clone();
        while let [only] = options.as_slice() {
            let (CommandOptionValue::SubCommand(inner)
            | CommandOptionValue::SubCommandGroup(inner)) = &only.value
            else {
                break;
            };
            path.push(only.name.clone());
            options = inner.clone();
        }
        Some(Self {
            interaction: InteractionRef::new(interaction.id, interaction.token.clone()),
            guild_id,
            channel_id: interaction.channel.as_ref().map(|channel| channel.id),
            invoker: Invoker {
                user_id,
                roles: member.roles.clone(),
                is_guild_admin: member
                    .permissions
                    .is_some_and(|permissions| permissions.contains(Permissions::ADMINISTRATOR)),
            },
            path,
            options,
        })
    }
}
