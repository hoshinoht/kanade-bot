//! Token issuance: random draws without replacement from the session's
//! filtered pool, the deterministic fallback, and source-literal bookkeeping.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use super::{Candidate, normalize::near_match};
use crate::infrastructure::llm::governor::Random;

/// Last-resort prefix, used only when every pool name is excluded; not an
/// English or schema word.
const SPARE: &str = "Nanashi";

pub(super) enum Holder {
    /// A user id.
    Member(String),
    /// Source text that spelled an already issued token; decodes to itself.
    Literal(String),
    /// A name word two or more members share (`Lim`): masked, but it stands
    /// for no member; replies show the word, refs refuse it.
    Shared(String),
}

pub(super) struct Issued {
    pub(super) token: String,
    pub(super) holder: Holder,
}

/// How an ASCII word of text relates to this session's tokens.
pub(super) enum Lookup {
    /// An issued token spelled by the first `core` bytes (the rest is a
    /// plural/possessive `s`).
    Token {
        index: usize,
        core: usize,
    },
    /// A pool form that appeared in source text; stays literal.
    Literal,
    /// A pool form this session never issued nor saw.
    Unknown {
        key: String,
    },
    Plain,
}

pub(super) struct Issuer {
    random: Arc<dyn Random>,
    /// Pool forms (lowercase) across sessions, for unknown-token detection.
    forms: Arc<HashSet<String>>,
    available: Vec<Candidate>,
    /// The filtered pool in list order, for the fallback.
    bases: Vec<Candidate>,
    fallback_round: usize,
    fallback_at: usize,
    spare_used: usize,
    pub(super) issued: Vec<Issued>,
    by_key: HashMap<String, usize>,
    by_user: HashMap<String, usize>,
    by_literal: HashMap<String, usize>,
    literals: HashSet<String>,
}

impl Issuer {
    pub(super) fn new(
        pool: &[Candidate],
        forms: Arc<HashSet<String>>,
        random: Arc<dyn Random>,
    ) -> Self {
        Self {
            random,
            forms,
            available: pool.to_vec(),
            bases: pool.to_vec(),
            fallback_round: 2,
            fallback_at: 0,
            spare_used: 0,
            issued: Vec::new(),
            by_key: HashMap::new(),
            by_user: HashMap::new(),
            by_literal: HashMap::new(),
            literals: HashSet::new(),
        }
    }

    /// Drop every pool name near-matching one of `keys` (normalised).
    pub(super) fn exclude(&mut self, keys: &[String]) {
        let clear = |c: &Candidate| !keys.iter().any(|key| near_match(&c.key, key));
        self.available.retain(clear);
        self.bases.retain(clear);
    }

    pub(super) fn member(&mut self, user_id: &str) -> &str {
        let index = match self.by_user.get(user_id) {
            Some(&index) => index,
            None => {
                let index = self.issue(Holder::Member(user_id.to_owned()));
                self.by_user.insert(user_id.to_owned(), index);
                index
            }
        };
        &self.issued[index].token
    }

    /// A fresh token standing for literal source text `core`.
    pub(super) fn shadow(&mut self, core: &str) -> &str {
        let key = core.to_ascii_lowercase();
        let index = match self.by_literal.get(&key) {
            Some(&index) => index,
            None => {
                let index = self.issue(Holder::Literal(core.to_owned()));
                self.by_literal.insert(key, index);
                index
            }
        };
        &self.issued[index].token
    }

    /// The token for a name word shared by several members.
    pub(super) fn shared(&mut self, word: &str) -> &str {
        let key = format!("shared:{}", word.to_lowercase());
        let index = match self.by_literal.get(&key) {
            Some(&index) => index,
            None => {
                let index = self.issue(Holder::Shared(word.to_owned()));
                self.by_literal.insert(key, index);
                index
            }
        };
        &self.issued[index].token
    }

    pub(super) fn has_member(&self, user_id: &str) -> bool {
        self.by_user.contains_key(user_id)
    }

    pub(super) fn member_ids(&self) -> impl Iterator<Item = &str> {
        self.by_user.keys().map(String::as_str)
    }

    pub(super) fn member_tokens(&self) -> Vec<String> {
        self.issued
            .iter()
            .filter(|issued| matches!(issued.holder, Holder::Member(_)))
            .map(|issued| issued.token.clone())
            .collect()
    }

    pub(super) fn literal_count(&self) -> usize {
        self.literals.len()
    }

    pub(super) fn available(&self) -> usize {
        self.available.len()
    }

    /// Source text contained this word: an unissued pool form becomes literal
    /// for the rest of the session and is never issued.
    pub(super) fn note_source(&mut self, word: &str) {
        if let Lookup::Unknown { key } = self.lookup(word) {
            self.available
                .retain(|c| c.name.to_ascii_lowercase() != key);
            self.literals.insert(key);
        }
    }

    pub(super) fn lookup(&self, word: &str) -> Lookup {
        if !word.is_ascii() {
            return Lookup::Plain;
        }
        let lower = word.to_ascii_lowercase();
        let singular = lower.strip_suffix('s').filter(|rest| !rest.is_empty());
        let candidates = std::iter::once(lower.as_str()).chain(singular);
        let mut form = None;
        for key in candidates {
            if let Some(&index) = self.by_key.get(key) {
                return Lookup::Token {
                    index,
                    core: key.len(),
                };
            }
            if self.literals.contains(key) {
                return Lookup::Literal;
            }
            if form.is_none() && self.is_form(key) {
                form = Some(key.to_owned());
            }
        }
        form.map_or(Lookup::Plain, |key| Lookup::Unknown { key })
    }

    /// A pool name, or a pool name or the spare prefix with a number.
    fn is_form(&self, key: &str) -> bool {
        if self.forms.contains(key) {
            return true;
        }
        let base = key.trim_end_matches(|c: char| c.is_ascii_digit());
        base.len() < key.len() && (self.forms.contains(base) || base.eq_ignore_ascii_case(SPARE))
    }

    fn issue(&mut self, holder: Holder) -> usize {
        let token = self.draw();
        let index = self.issued.len();
        self.by_key.insert(token.to_ascii_lowercase(), index);
        self.issued.push(Issued { token, holder });
        index
    }

    fn free(&self, token: &str) -> bool {
        let key = token.to_ascii_lowercase();
        !self.literals.contains(&key) && !self.by_key.contains_key(&key)
    }

    fn draw(&mut self) -> String {
        while !self.available.is_empty() {
            let at = (self.random.next_u64() % self.available.len() as u64) as usize;
            let name = self.available.swap_remove(at).name;
            if self.free(&name) {
                return name;
            }
        }
        loop {
            let candidate = self.fallback();
            if self.free(&candidate) {
                return candidate;
            }
        }
    }

    /// `<Name><n>` over the filtered pool in list order, n = 2, 3, …; when the
    /// pool is empty, `Nanashi<n>`.
    fn fallback(&mut self) -> String {
        if self.bases.is_empty() {
            self.spare_used += 1;
            return format!("{SPARE}{}", self.spare_used);
        }
        if self.fallback_at >= self.bases.len() {
            self.fallback_at = 0;
            self.fallback_round += 1;
        }
        let name = format!(
            "{}{}",
            self.bases[self.fallback_at].name, self.fallback_round
        );
        self.fallback_at += 1;
        name
    }
}
