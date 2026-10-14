/// A message-only error whose `Display` is the message.
macro_rules! message_error {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
        #[error("{message}")]
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
