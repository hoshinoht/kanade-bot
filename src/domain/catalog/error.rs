use std::fmt;

/// A message-only error whose `Display` is the message.
macro_rules! message_error {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name {
            message: String,
        }

        impl $name {
            pub(super) fn new(message: impl Into<String>) -> Self {
                Self {
                    message: message.into(),
                }
            }

            pub fn message(&self) -> &str {
                &self.message
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.message)
            }
        }

        impl std::error::Error for $name {}
    };
}

message_error!(
    /// Boss input that cannot be resolved to a canonical boss; the message is
    /// shown to members as-is.
    BossParseError
);

message_error!(
    /// A malformed boss catalog.
    BossTableError
);
