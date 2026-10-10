//! Typed outcomes with their stored (and API) spellings.

use std::fmt;

macro_rules! spelled {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            pub fn parse(value: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|item| item.as_str() == value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

spelled! {
    /// How a chat question ended. `withheld` and `clean_retry` are also
    /// flags on the interaction, so a filter on either matches the flag too.
    /// `unknown` is for v4-imported rows that map to nothing else.
    /// `profanity`: the guardrail deflected the question or retried or
    /// replaced the reply (details in `guardrail.profanity`).
    ChatOutcome {
        Answered => "answered",
        Refused => "refused",
        Clarified => "clarified",
        Error => "error",
        Timeout => "timeout",
        RateLimited => "rate_limited",
        TurnedAway => "turned_away",
        ContentBlocked => "content_blocked",
        Withheld => "withheld",
        CleanRetry => "clean_retry",
        Profanity => "profanity",
        Unknown => "unknown",
    }
}

spelled! {
    /// How one extraction pass ended (`identity_leak`: the provider-boundary
    /// scanner refused the request, nothing was sent); `unknown` as for chat.
    ExtractionOutcome {
        Proposed => "proposed",
        NoChange => "no_change",
        Failed => "failed",
        TurnedAway => "turned_away",
        ContentBlocked => "content_blocked",
        SelfServiceLink => "self_service_link",
        IdentityLeak => "identity_leak",
        Unknown => "unknown",
    }
}

spelled! {
    /// A rescan job's state (v4 spellings). Done, failed and cancelled are
    /// final.
    RescanStatus {
        Queued => "queued",
        Running => "running",
        Done => "done",
        Failed => "failed",
        Cancelled => "cancelled",
    }
}

impl RescanStatus {
    pub fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }
}
