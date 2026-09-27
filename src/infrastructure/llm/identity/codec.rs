use std::{fmt, sync::Arc};

use super::scan::ScanExemptions;

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

/// What a session masks and which tokens it issued, for the scanner.
/// Memory only; `Debug` shows counts.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ScanNeedles {
    /// Names, nicknames, aliases and author labels the session masks.
    pub names: Vec<ScanName>,
    /// User ids the session masks (roster and every issued id).
    pub ids: Vec<String>,
    /// Every identity or link token issued so far; these may appear in requests.
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

    /// Code-owned words the boundary scanner never flags as a name.
    fn scan_exemptions(&self) -> Arc<ScanExemptions> {
        Arc::new(ScanExemptions::builtin())
    }
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

    /// A name this member was known by before (a nickname, display name or
    /// alias since changed or removed, or a member who left): masked and
    /// scanned from now on like a current name, without issuing a token
    /// until it is used. Stored text (chat history, anchors, the bot's own
    /// earlier replies) may still carry it.
    fn former_name(&mut self, _user_id: &str, _name: &str) {}

    /// Whether this session hides identities (cheap; no needle copy).
    fn masks(&self) -> bool {
        false
    }

    /// How a prompt shows a Discord message id (extraction's `[msg_id]`):
    /// a short per-session ref when masking, the id itself otherwise.
    fn message_ref(&mut self, message_id: &str) -> String {
        message_id.to_owned()
    }

    /// A message ref from model output back to its message id; a ref this
    /// session never issued is `UnknownToken`.
    fn decode_message_ref(&self, value: &str) -> Result<String, DecodeError> {
        Ok(value.to_owned())
    }

    /// Every member token issued so far with the identity it stands for,
    /// for the admin Model view. Never log it; `None` when nothing is masked.
    fn mapping(&self) -> Option<Vec<IssuedName>> {
        None
    }
}

/// One issued member token: what the model saw and who it was. Stored only
/// with the admin-only chat log; `Debug` hides everything but sizes.
#[derive(Clone, PartialEq, Eq)]
pub struct IssuedName {
    pub token: String,
    pub user_id: String,
    /// The roster display name, else the last author label, at issue time.
    pub display_name: Option<String>,
}

impl fmt::Debug for IssuedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedName")
            .field("token_bytes", &self.token.len())
            .field("has_name", &self.display_name.is_some())
            .finish()
    }
}

/// The roster a port loads per call; `None` when it cannot be read (a
/// masking port then fails closed).
pub trait RosterSource: Send + Sync {
    fn roster(&self) -> Option<Vec<Member>>;
}

/// Channel and role names for `<#id>` and `<@&id>` mentions (the guild
/// cache); unknown ids become opaque refs instead.
pub trait MentionNames: Send + Sync {
    fn channel(&self, id: &str) -> Option<String>;
    fn role(&self, id: &str) -> Option<String>;
}
