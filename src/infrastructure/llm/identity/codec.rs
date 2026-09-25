use std::fmt;

/// Whether a codec hides member identities from the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodecMode {
    Passthrough,
    Pseudonymizing,
}

/// A roster member as the ports know it; `user_id` is the decimal Discord id.
#[derive(Clone, PartialEq, Eq)]
pub struct Member {
    pub user_id: String,
    pub display_name: String,
    pub nickname: Option<String>,
    pub aliases: Vec<String>,
}

impl fmt::Debug for Member {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Member")
            .field("user_id_bytes", &self.user_id.len())
            .field("has_nickname", &self.nickname.is_some())
            .field("alias_count", &self.aliases.len())
            .finish()
    }
}

/// Model output named an identity token the session never issued; callers
/// quarantine the output as malformed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    UnknownToken { offset: usize },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownToken { offset } => {
                write!(f, "unknown identity token at byte {offset}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

/// Chooses how member identities appear in model traffic. Extraction opens one
/// session per request; chat keeps one session for a whole conversation so
/// tokens stay consistent across rounds.
pub trait IdentityCodec: Send + Sync {
    fn mode(&self) -> CodecMode;
    fn open(&self, roster: &[Member]) -> Box<dyn IdentitySession>;
}

/// Encodes everything identity-bearing that the ports put in a prompt and
/// decodes everything identity-bearing the model sends back. Mapping state
/// lives only in the session and is never persisted.
pub trait IdentitySession: Send {
    /// The label shown for a message author (`name` is their Discord name).
    fn author_label(&mut self, user_id: &str, name: &str) -> String;

    /// How the model refers to a member in participant lists, extractor
    /// `participants` and tool arguments.
    fn member_ref(&mut self, user_id: &str) -> String;

    /// A mention as rendered in prompts.
    fn mention(&mut self, user_id: &str) -> String {
        format!("<@{}>", self.member_ref(user_id))
    }

    fn participants(&mut self, user_ids: &[&str]) -> Vec<String> {
        user_ids.iter().map(|id| self.member_ref(id)).collect()
    }

    /// Message bodies and other free text, including raw Discord mentions.
    fn text(&mut self, text: &str) -> String;

    /// Tool results before they are appended to the transcript.
    fn tool_result(&mut self, content: &str) -> String;

    /// The closed set of member refs issued so far, for a strict extractor
    /// schema enum; call after encoding the prompt. `None` means unconstrained.
    fn participant_enum(&self) -> Option<Vec<String>>;

    /// One member ref from model output back to its user id form.
    fn decode_ref(&self, value: &str) -> Result<String, DecodeError>;

    /// Tool-call arguments or structured output (JSON text).
    fn decode_json(&self, json: &str) -> Result<String, DecodeError>;

    /// A free-text reply that will be shown to members.
    fn decode_reply(&self, text: &str) -> Result<String, DecodeError>;
}
