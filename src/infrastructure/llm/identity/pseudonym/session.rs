use std::{
    collections::{BTreeSet, HashMap},
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
    DecodeError, IdentitySession, Member, ScanName, ScanNeedles,
};

/// A name the session masks in text.
pub(super) struct Needle {
    pub(super) folded: Vec<char>,
    pub(super) text: String,
    pub(super) user_id: String,
    collides: bool,
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
            if form.chars().count() < 2 {
                continue;
            }
            let folded = fold(form);
            if self
                .needles
                .iter()
                .any(|n| n.folded == folded && n.user_id == user_id)
            {
                continue;
            }
            self.needles.push(Needle {
                folded,
                text: form.to_owned(),
                user_id: user_id.to_owned(),
                collides: self.shared.lexicon.collides(form),
            });
        }
        self.needles
            .sort_by_key(|needle| std::cmp::Reverse(needle.folded.len()));
    }

    /// The token for a member, or the bot's name for the bot.
    pub(super) fn token(&mut self, user_id: &str) -> String {
        if self.is_bot(user_id) {
            return self.bot_name().to_owned();
        }
        self.issuer.member(user_id).to_owned()
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
            .field("pool_available", &self.issuer.available())
            .finish()
    }
}
