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

/// Raw identities a session masks, for a provider-boundary scanner. Memory
/// only; `Debug` shows counts.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ScanNeedles {
    /// Names, nicknames, aliases and author labels the session masks.
    pub names: Vec<ScanName>,
    /// User ids the session masks (roster and every issued id).
    pub ids: Vec<String>,
    /// Every token issued so far; these may appear in requests.
    pub tokens: Vec<String>,
    /// Names shorter than two characters, which are never masked.
    pub skipped_short: usize,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ScanName {
    pub text: String,
    /// Equals a code-lexicon word or stopword; masking it also masks that
    /// word in member text, and a scanner must not flag it in code-owned text.
    pub collides: bool,
    /// Equals or near-matches an issued token (an author registered after
    /// that token was issued), so the scanner cannot tell the two apart.
    pub token_clash: bool,
}

impl ScanNeedles {
    pub fn collisions(&self) -> usize {
        self.names.iter().filter(|name| name.collides).count()
    }
}

impl fmt::Debug for ScanNeedles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScanNeedles")
            .field("names", &self.names.len())
            .field("ids", &self.ids.len())
            .field("tokens", &self.tokens.len())
            .field("collisions", &self.collisions())
            .field("skipped_short", &self.skipped_short)
            .finish()
    }
}

impl fmt::Debug for ScanName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScanName")
            .field("bytes", &self.text.len())
            .field("collides", &self.collides)
            .field("token_clash", &self.token_clash)
            .finish()
    }
}

/// Chooses how member identities appear in model traffic. Extraction opens one
/// session per request and chat one per question; mappings never outlive it.
pub trait IdentityCodec: Send + Sync {
    fn mode(&self) -> CodecMode;
    fn open(&self, roster: &[Member]) -> Box<dyn IdentitySession>;
}

/// Encodes everything identity-bearing that the ports put in a prompt and
/// decodes everything identity-bearing the model sends back. Mapping state
/// lives only in the session and is never persisted.
pub trait IdentitySession: Send {
    /// The label shown for a message author (`name` is their Discord name).
    /// The name is masked in text encoded afterwards, so register every author
    /// before encoding text that may mention them.
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

    /// What this session masks, for a boundary scanner; `None` when it masks
    /// nothing (passthrough).
    fn scan_needles(&self) -> Option<ScanNeedles> {
        None
    }
}
