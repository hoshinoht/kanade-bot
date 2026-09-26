use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt,
    sync::Arc,
};

use super::{
    Shared,
    issuer::Issuer,
    matcher::fold,
    normalize::{name_keys, near_match, normalise},
};
use crate::infrastructure::llm::identity::codec::{
    DecodeError, IdentitySession, IssuedName, Member, ScanName, ScanNeedles,
};

/// Prefix of the opaque tokens that stand for stray snowflakes (ids of
/// nobody in the session: messages, channels, roles, outsiders).
const OPAQUE: &str = "Ref";

/// Word needles need at least this many characters: two-letter words
/// (`Li`, `Ng`) would mask too much ordinary text and abbreviations.
const WORD_NEEDLE_CHARS: usize = 3;

/// The words of a name with two or more words, long enough to mask alone.
fn name_words(name: &str) -> Vec<&str> {
    let words: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    if words.len() < 2 {
        return Vec::new();
    }
    words
        .into_iter()
        .filter(|word| word.chars().count() >= WORD_NEEDLE_CHARS)
        .collect()
}

/// A 17–20 digit run that is no known id: a stray snowflake.
pub(super) fn is_snowflake(digits: &str) -> bool {
    (17..=20).contains(&digits.len())
}

/// Whom a needle stands for.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum Owner {
    Member(String),
    /// A name word several members share: masked, attributed to nobody.
    Shared,
}

/// A name the session masks in text.
pub(super) struct Needle {
    pub(super) folded: Vec<char>,
    pub(super) text: String,
    pub(super) owner: Owner,
    collides: bool,
    /// One word of a multi-word name, not a whole name.
    word: bool,
}

/// One prompt's (extraction request's, chat question's) identity mapping.
/// Tokens are issued lazily on first use.
pub struct PseudonymSession {
    pub(super) shared: Arc<Shared>,
    /// User id → the name a reply shows (roster display name, else the last
    /// author label).
    pub(super) names: HashMap<String, String>,
    /// Longest first.
    pub(super) needles: Vec<Needle>,
    skipped_short: usize,
    roster_ids: BTreeSet<String>,
    pub(super) issuer: Issuer,
    /// Extraction message refs: `[n]` stands for `message_refs[n - 1]`.
    message_refs: Vec<String>,
    /// Stray snowflakes: `Ref<n>` stands for `opaque[n - 1]`.
    opaque: Vec<String>,
    /// `Ref<n>` words seen in source text (lowercase); never issued.
    pub(super) source_refs: HashSet<String>,
}

impl PseudonymSession {
    pub(super) fn new(shared: Arc<Shared>, roster: &[Member]) -> Self {
        let issuer = Issuer::new(
            &shared.pool,
            Arc::clone(&shared.forms),
            Arc::clone(&shared.random),
        );
        let mut session = Self {
            shared,
            names: HashMap::new(),
            needles: Vec::new(),
            skipped_short: 0,
            roster_ids: BTreeSet::new(),
            issuer,
            message_refs: Vec::new(),
            opaque: Vec::new(),
            source_refs: HashSet::new(),
        };
        for member in roster {
            if session.is_bot(&member.user_id) {
                continue;
            }
            session.roster_ids.insert(member.user_id.clone());
            session
                .names
                .insert(member.user_id.clone(), member.display_name.clone());
            let names = std::iter::once(&member.display_name)
                .chain(&member.nickname)
                .chain(&member.aliases);
            for name in names {
                session.add_needle(name, &member.user_id);
            }
        }
        session
    }

    pub(super) fn is_bot(&self, user_id: &str) -> bool {
        self.shared.bot.user_id.as_deref() == Some(user_id)
    }

    pub(super) fn bot_name(&self) -> &str {
        &self.shared.bot.name
    }

    /// Roster ids and every id a token was issued for.
    pub(super) fn is_known_id(&self, digits: &str) -> bool {
        self.roster_ids.contains(digits) || self.issuer.has_member(digits)
    }

    /// The name is masked from now on (and a copy without leading/trailing
    /// decoration such as emoji); pool names near it are withdrawn.
    fn add_needle(&mut self, name: &str, user_id: &str) {
        let trimmed = name.trim();
        let bare = trimmed.trim_matches(|c: char| !c.is_alphanumeric());
        if trimmed.is_empty() {
            return;
        }
        self.issuer.exclude(&name_keys(trimmed));
        if trimmed.chars().count() < 2 {
            self.skipped_short += 1;
            return;
        }
        let forms = std::iter::once(trimmed).chain((bare != trimmed).then_some(bare));
        for form in forms {
            if form.chars().count() >= 2 {
                self.push_whole(form, user_id);
            }
        }
        // Each word of a multi-word name on its own (`jonas` of `Jonas lau`):
        // guilds name party channels and nicknames after first names.
        for word in name_words(trimmed) {
            let lower = word.to_lowercase();
            if !self.shared.lexicon.collides(word) && !self.shared.unmaskable.contains(&lower) {
                self.push_word(word, user_id);
            }
        }
        // Longest first; at equal length a whole name before a word.
        self.needles
            .sort_by_key(|needle| (std::cmp::Reverse(needle.folded.len()), needle.word));
    }

    /// A whole name: another member's word needle equal to it goes (a
    /// whole name is never somebody else's word).
    fn push_whole(&mut self, text: &str, user_id: &str) {
        let folded = fold(text);
        let owner = Owner::Member(user_id.to_owned());
        self.needles
            .retain(|n| !(n.word && n.folded == folded && n.owner != owner));
        if self
            .needles
            .iter()
            .any(|n| !n.word && n.folded == folded && n.owner == owner)
        {
            return;
        }
        self.needles.push(Needle {
            folded,
            text: text.to_owned(),
            owner,
            collides: self.shared.lexicon.collides(text),
            word: false,
        });
    }

    /// A name word: skipped where it is some member's whole name; shared by
    /// two members it stands for nobody from now on (earlier text keeps the
    /// member it was encoded for).
    fn push_word(&mut self, text: &str, user_id: &str) {
        let folded = fold(text);
        let owner = Owner::Member(user_id.to_owned());
        if self.needles.iter().any(|n| !n.word && n.folded == folded) {
            return;
        }
        if let Some(existing) = self
            .needles
            .iter_mut()
            .find(|n| n.word && n.folded == folded)
        {
            if existing.owner != owner {
                existing.owner = Owner::Shared;
            }
            return;
        }
        self.needles.push(Needle {
            folded,
            text: text.to_owned(),
            owner,
            collides: false,
            word: true,
        });
    }

    /// The token for a member, or the bot's name for the bot.
    pub(super) fn token(&mut self, user_id: &str) -> String {
        if self.is_bot(user_id) {
            return self.bot_name().to_owned();
        }
        self.issuer.member(user_id).to_owned()
    }

    /// The opaque token for a stray snowflake (issued once per digits).
    pub(super) fn opaque_token(&mut self, digits: &str) -> String {
        let index = match self.opaque.iter().position(|known| known == digits) {
            Some(index) => index,
            None => {
                self.opaque.push(digits.to_owned());
                self.opaque.len() - 1
            }
        };
        self.opaque_name(index)
    }

    /// `Ref<n>`, skipping any `Ref<n>` spelled in source text.
    fn opaque_name(&self, index: usize) -> String {
        let mut n = 0;
        let mut seen = 0;
        loop {
            n += 1;
            let name = format!("{OPAQUE}{n}");
            if self.source_refs.contains(&name.to_ascii_lowercase()) {
                continue;
            }
            if seen == index {
                return name;
            }
            seen += 1;
        }
    }

    /// The digits an issued opaque token stands for.
    pub(super) fn opaque_digits(&self, word: &str) -> Option<&str> {
        if !Self::is_opaque_form(word) {
            return None;
        }
        (0..self.opaque.len())
            .find(|&index| self.opaque_name(index).eq_ignore_ascii_case(word))
            .map(|index| self.opaque[index].as_str())
    }

    /// Whether a source word spells an opaque token (`ref12`).
    pub(super) fn is_opaque_form(word: &str) -> bool {
        word.len() > OPAQUE.len()
            && word.is_char_boundary(OPAQUE.len())
            && word[..OPAQUE.len()].eq_ignore_ascii_case(OPAQUE)
            && word[OPAQUE.len()..].bytes().all(|b| b.is_ascii_digit())
    }

    /// What a boundary scanner must not see in a request built through this
    /// session (also available as [`IdentitySession::scan_needles`]).
    pub fn needles(&self) -> ScanNeedles {
        let tokens: Vec<String> = self.issuer.issued.iter().map(|i| i.token.clone()).collect();
        let token_keys: Vec<String> = tokens.iter().map(|t| normalise(t)).collect();
        ScanNeedles {
            names: self
                .needles
                .iter()
                .map(|n| {
                    let key = normalise(&n.text);
                    ScanName {
                        text: n.text.clone(),
                        collides: n.collides,
                        token_clash: token_keys.iter().any(|t| near_match(t, &key)),
                    }
                })
                .collect(),
            tokens,
            ids: self
                .roster_ids
                .iter()
                .map(String::as_str)
                .chain(self.issuer.member_ids())
                .collect::<BTreeSet<&str>>()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            skipped_short: self.skipped_short,
        }
    }
}

impl IdentitySession for PseudonymSession {
    fn author_label(&mut self, user_id: &str, name: &str) -> String {
        if self.is_bot(user_id) {
            return self.bot_name().to_owned();
        }
        self.add_needle(name, user_id);
        if !name.trim().is_empty() && !self.roster_ids.contains(user_id) {
            self.names
                .insert(user_id.to_owned(), name.trim().to_owned());
        }
        self.token(user_id)
    }

    fn member_ref(&mut self, user_id: &str) -> String {
        self.token(user_id)
    }

    fn mention(&mut self, user_id: &str) -> String {
        if self.is_bot(user_id) {
            return format!("@{}", self.bot_name());
        }
        format!("<@{}>", self.token(user_id))
    }

    fn text(&mut self, text: &str) -> String {
        self.encode(text)
    }

    fn tool_result(&mut self, content: &str) -> String {
        self.encode_tool_result(content)
    }

    fn participant_enum(&self) -> Option<Vec<String>> {
        Some(self.issuer.member_tokens())
    }

    fn decode_ref(&self, value: &str) -> Result<String, DecodeError> {
        if let Some(bot_id) = &self.shared.bot.user_id
            && !self.bot_name().is_empty()
            && value.trim().eq_ignore_ascii_case(self.bot_name())
        {
            return Ok(bot_id.clone());
        }
        self.decode(value, false)
    }

    fn decode_json(&self, json: &str) -> Result<String, DecodeError> {
        self.decode(json, false)
    }

    fn decode_reply(&self, text: &str) -> Result<String, DecodeError> {
        self.decode(text, true)
    }

    fn scan_needles(&self) -> Option<ScanNeedles> {
        Some(self.needles())
    }

    fn former_name(&mut self, user_id: &str, name: &str) {
        if self.is_bot(user_id) {
            return;
        }
        self.add_needle(name, user_id);
        // Registered oldest first, so a departed member shows the latest.
        if !self.roster_ids.contains(user_id) && !name.trim().is_empty() {
            self.names
                .insert(user_id.to_owned(), name.trim().to_owned());
        }
    }

    fn masks(&self) -> bool {
        true
    }

    fn message_ref(&mut self, message_id: &str) -> String {
        let index = match self.message_refs.iter().position(|id| id == message_id) {
            Some(index) => index,
            None => {
                self.message_refs.push(message_id.to_owned());
                self.message_refs.len() - 1
            }
        };
        (index + 1).to_string()
    }

    fn decode_message_ref(&self, value: &str) -> Result<String, DecodeError> {
        let trimmed = value.trim();
        let bare = trimmed
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .unwrap_or(trimmed)
            .trim();
        bare.parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .and_then(|index| self.message_refs.get(index))
            .cloned()
            .ok_or(DecodeError::UnknownToken { offset: 0 })
    }

    fn mapping(&self) -> Option<Vec<IssuedName>> {
        Some(
            self.issuer
                .issued
                .iter()
                .filter_map(|issued| match &issued.holder {
                    super::issuer::Holder::Member(user_id) => Some(IssuedName {
                        token: issued.token.clone(),
                        user_id: user_id.clone(),
                        display_name: self.names.get(user_id).cloned(),
                    }),
                    super::issuer::Holder::Literal(_) | super::issuer::Holder::Shared(_) => None,
                })
                .collect(),
        )
    }
}

impl fmt::Debug for PseudonymSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PseudonymSession")
            .field("issued", &self.issuer.issued.len())
            .field("needles", &self.needles.len())
            .field(
                "collisions",
                &self.needles.iter().filter(|n| n.collides).count(),
            )
            .field("skipped_short", &self.skipped_short)
            .field("literals", &self.issuer.literal_count())
            .field("message_refs", &self.message_refs.len())
            .field("opaque", &self.opaque.len())
            .field("pool_available", &self.issuer.available())
            .finish()
    }
}
