//! Registration payload builders. Every command is guild-only; registration
//! itself is guild-scoped (`register_guild_commands`), never global.

use twilight_model::application::command::{
    Command, CommandOption, CommandOptionChoice, CommandOptionChoiceValue, CommandOptionType,
    CommandOptionValue, CommandType,
};
use twilight_model::application::interaction::InteractionContextType;
use twilight_model::channel::ChannelType;
use twilight_model::guild::Permissions;
use twilight_model::id::Id;

/// A chat-input command. `admin_only` hides it from ordinary members'
/// pickers (v4 `default_permissions(administrator=True)`); the gate still
/// decides.
// `dm_permission` is deprecated in favour of `contexts` but is a required
// field of the struct; it stays unset.
#[allow(deprecated)]
pub fn command(
    name: &str,
    description: &str,
    admin_only: bool,
    options: Vec<CommandOption>,
) -> Command {
    Command {
        application_id: None,
        contexts: Some(vec![InteractionContextType::Guild]),
        default_member_permissions: admin_only.then_some(Permissions::ADMINISTRATOR),
        dm_permission: None,
        description: description.to_owned(),
        description_localizations: None,
        guild_id: None,
        id: None,
        integration_types: None,
        kind: CommandType::ChatInput,
        name: name.to_owned(),
        name_localizations: None,
        nsfw: None,
        options,
        version: Id::new(1),
    }
}

fn option(kind: CommandOptionType, name: &str, description: &str) -> CommandOption {
    CommandOption {
        autocomplete: None,
        channel_types: None,
        choices: None,
        description: description.to_owned(),
        description_localizations: None,
        kind,
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

pub fn subcommand(name: &str, description: &str, options: Vec<CommandOption>) -> CommandOption {
    CommandOption {
        options: Some(options),
        ..option(CommandOptionType::SubCommand, name, description)
    }
}

fn required(mut option: CommandOption, required: bool) -> CommandOption {
    option.required = required.then_some(true);
    option
}

pub fn text(name: &str, description: &str, is_required: bool) -> CommandOption {
    required(
        option(CommandOptionType::String, name, description),
        is_required,
    )
}

/// A text option whose values come from the command's autocomplete.
pub fn picked(name: &str, description: &str, is_required: bool) -> CommandOption {
    CommandOption {
        autocomplete: Some(true),
        ..text(name, description, is_required)
    }
}

/// A text option limited to `(label, value)` choices.
pub fn choices(
    name: &str,
    description: &str,
    is_required: bool,
    values: &[(&str, &str)],
) -> CommandOption {
    CommandOption {
        choices: Some(
            values
                .iter()
                .map(|(label, value)| CommandOptionChoice {
                    name: label.chars().take(100).collect(),
                    name_localizations: None,
                    value: CommandOptionChoiceValue::String((*value).to_owned()),
                })
                .collect(),
        ),
        ..text(name, description, is_required)
    }
}

pub fn user(name: &str, description: &str, is_required: bool) -> CommandOption {
    required(
        option(CommandOptionType::User, name, description),
        is_required,
    )
}

/// A text channel picker (discord.py `TextChannel`).
pub fn text_channel(name: &str, description: &str) -> CommandOption {
    CommandOption {
        channel_types: Some(vec![ChannelType::GuildText]),
        ..option(CommandOptionType::Channel, name, description)
    }
}

/// An optional integer option within `min..=max`.
pub fn integer(name: &str, description: &str, min: i64, max: i64) -> CommandOption {
    CommandOption {
        min_value: Some(CommandOptionValue::Integer(min)),
        max_value: Some(CommandOptionValue::Integer(max)),
        ..option(CommandOptionType::Integer, name, description)
    }
}

pub fn flag(name: &str, description: &str) -> CommandOption {
    option(CommandOptionType::Boolean, name, description)
}
