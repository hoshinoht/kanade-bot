use std::fmt;

use super::PersonaError;

/// Tracked trusted fallback bundle.
pub const FALLBACK_PERSONA: &str = "kanade";
/// Tracked profile template; never a selectable candidate.
pub const EXAMPLE_PROFILE: &str = "example";

const MAX_ID_CHARS: usize = 50;
const MAX_ALIAS_CHARS: usize = 100;

fn is_slug(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some('a'..='z' | '0'..='9'))
        && value.len() <= MAX_ID_CHARS
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'))
}

macro_rules! slug_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: &str) -> Result<Self, PersonaError> {
                if is_slug(value) {
                    Ok(Self(value.to_owned()))
                } else {
                    Err(PersonaError::UnsafeId)
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

slug_id!(
    /// Canonical global persona ID; bundle paths derive from it.
    PersonaId
);
slug_id!(
    /// Reply profile ID; profile paths derive from it.
    ProfileId
);

/// Opaque Discord role ID used only for profile matching; never displayed.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RoleId(String);

impl RoleId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Debug for RoleId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RoleId(<redacted>)")
    }
}

/// Aliases are legacy selection tokens such as `persona.md`, never paths.
pub(crate) fn validate_alias(value: &str) -> Result<(), PersonaError> {
    let unsafe_alias = value.is_empty()
        || value.chars().count() > MAX_ALIAS_CHARS
        || value == "."
        || value == ".."
        || value.contains(['/', '\\'])
        || value.chars().any(char::is_control);
    if unsafe_alias {
        Err(PersonaError::UnsafeAlias)
    } else {
        Ok(())
    }
}
