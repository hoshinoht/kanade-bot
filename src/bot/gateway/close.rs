//! Why the gateway closed for good, with what an operator should do.

use std::fmt;

/// The close that ended the gateway stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// 4004.
    AuthenticationFailed,
    /// 4010.
    InvalidShard,
    /// 4011.
    ShardingRequired,
    /// 4012.
    InvalidApiVersion,
    /// 4013.
    InvalidIntents,
    /// 4014: a privileged intent is not enabled for the application.
    DisallowedIntents,
    /// Any other code, or none (the stream ended without a close frame).
    Other(Option<u16>),
}

impl CloseReason {
    pub fn from_code(code: Option<u16>) -> Self {
        match code {
            Some(4004) => Self::AuthenticationFailed,
            Some(4010) => Self::InvalidShard,
            Some(4011) => Self::ShardingRequired,
            Some(4012) => Self::InvalidApiVersion,
            Some(4013) => Self::InvalidIntents,
            Some(4014) => Self::DisallowedIntents,
            other => Self::Other(other),
        }
    }

    pub fn code(self) -> Option<u16> {
        match self {
            Self::AuthenticationFailed => Some(4004),
            Self::InvalidShard => Some(4010),
            Self::ShardingRequired => Some(4011),
            Self::InvalidApiVersion => Some(4012),
            Self::InvalidIntents => Some(4013),
            Self::DisallowedIntents => Some(4014),
            Self::Other(code) => code,
        }
    }

    /// Operator-facing guidance.
    pub fn message(self) -> &'static str {
        match self {
            Self::AuthenticationFailed => {
                "Discord rejected the bot token; check or regenerate it in the Developer Portal"
            }
            Self::InvalidShard | Self::ShardingRequired => {
                "Discord refused the single-shard session; the bot is in too many guilds for one shard"
            }
            Self::InvalidApiVersion => "Discord refused the gateway API version; upgrade Twilight",
            Self::InvalidIntents => "Discord refused the requested intents as invalid",
            Self::DisallowedIntents => {
                "enable the Server Members and Message Content privileged intents for the \
                 application in the Discord Developer Portal (Bot page)"
            }
            Self::Other(_) => "the gateway connection closed and will not reconnect",
        }
    }
}

impl fmt::Display for CloseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.code() {
            Some(code) => write!(f, "gateway closed ({code}): {}", self.message()),
            None => write!(f, "gateway closed: {}", self.message()),
        }
    }
}
