//! Typed reads of a leaf command's options.

use twilight_model::application::interaction::application_command::{
    CommandDataOption, CommandOptionValue,
};

use crate::bot::ids::id_text;

/// A leaf command's options by name; Discord has already checked their types.
#[derive(Clone, Copy, Debug)]
pub struct Args<'a>(pub &'a [CommandDataOption]);

impl<'a> Args<'a> {
    fn value(&self, name: &str) -> Option<&'a CommandOptionValue> {
        self.0
            .iter()
            .find(|option| option.name == name)
            .map(|option| &option.value)
    }

    pub fn text(&self, name: &str) -> Option<&'a str> {
        match self.value(name)? {
            CommandOptionValue::String(value) | CommandOptionValue::Focused(value, _) => {
                Some(value.as_str())
            }
            _ => None,
        }
    }

    /// A user picker's id as text.
    pub fn user(&self, name: &str) -> Option<String> {
        match self.value(name)? {
            CommandOptionValue::User(id) => Some(id_text(*id)),
            _ => None,
        }
    }

    /// A channel picker's id as text.
    pub fn channel(&self, name: &str) -> Option<String> {
        match self.value(name)? {
            CommandOptionValue::Channel(id) => Some(id_text(*id)),
            _ => None,
        }
    }

    pub fn flag(&self, name: &str) -> Option<bool> {
        match self.value(name)? {
            CommandOptionValue::Boolean(value) => Some(*value),
            _ => None,
        }
    }
}
