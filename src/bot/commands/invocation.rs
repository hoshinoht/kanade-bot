//! A slash-command (or autocomplete) interaction reduced to what dispatch
//! needs.

use twilight_model::application::interaction::application_command::{
    CommandDataOption, CommandOptionValue,
};
use twilight_model::application::interaction::{Interaction, InteractionData, InteractionType};
use twilight_model::guild::Permissions;
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, GuildMarker, UserMarker},
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
    /// Server nickname, else global name, else username.
    pub invoker_name: Option<String>,
    /// Command, then subcommand group/subcommand names.
    pub path: Vec<String>,
    /// The leaf command's options.
    pub options: Vec<CommandDataOption>,
    /// An autocomplete request rather than a run.
    pub autocomplete: bool,
    /// The guild owner as the dispatcher knows it (staff checks in handlers).
    pub owner_id: Option<Id<UserMarker>>,
}

impl Invocation {
    /// `None` for anything but a guild chat-input command (or its
    /// autocomplete) with a member.
    pub fn from_interaction(interaction: &Interaction) -> Option<Self> {
        let autocomplete = match interaction.kind {
            InteractionType::ApplicationCommand => false,
            InteractionType::ApplicationCommandAutocomplete => true,
            _ => return None,
        };
        let guild_id = interaction.guild_id?;
        let member = interaction.member.as_ref()?;
        let user = member.user.as_ref()?;
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
                user_id: user.id,
                roles: member.roles.clone(),
                is_guild_admin: member
                    .permissions
                    .is_some_and(|permissions| permissions.contains(Permissions::ADMINISTRATOR)),
            },
            invoker_name: member
                .nick
                .clone()
                .or_else(|| user.global_name.clone())
                .or_else(|| Some(user.name.clone()))
                .filter(|name| !name.is_empty()),
            path,
            options,
            autocomplete,
            owner_id: None,
        })
    }

    /// The option being typed in an autocomplete request: `(name, text)`.
    pub fn focused(&self) -> Option<(&str, &str)> {
        self.options.iter().find_map(|option| match &option.value {
            CommandOptionValue::Focused(text, _) => Some((option.name.as_str(), text.as_str())),
            _ => None,
        })
    }
}
