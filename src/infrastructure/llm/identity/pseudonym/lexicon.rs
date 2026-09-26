use std::{collections::BTreeSet, fmt};

use super::{
    normalize::{name_keys, near_match, normalise},
    words,
};
use crate::domain::catalog::BossTable;

/// Code-owned vocabulary pseudonyms must not resemble: calendar, difficulty,
/// MapleStory class/NPC/boss, schema words and the loaded boss catalog, plus a
/// static English stopword list. Also flags member names that collide with it.
#[derive(Clone)]
pub struct CodeLexicon {
    /// Normalised terms; block pool names by near-match.
    terms: BTreeSet<String>,
    /// Normalised stopwords; block pool names only when equal.
    stopwords: BTreeSet<String>,
}

impl CodeLexicon {
    /// The static lists only.
    pub fn builtin() -> Self {
        let lexicon = Self {
            terms: BTreeSet::new(),
            stopwords: split(words::STOPWORDS).map(normalise).collect(),
        };
        [
            words::CALENDAR,
            words::DIFFICULTY,
            words::GAME,
            words::SCHEMA,
        ]
        .into_iter()
        .fold(lexicon, |lexicon, list| lexicon.with_terms(split(list)))
    }

    /// Static lists plus every boss short/full name, alias, canonical form and
    /// difficulty label of the loaded catalog.
    pub fn from_boss_table(table: &BossTable) -> Self {
        let mut terms: Vec<String> = Vec::new();
        for difficulty in table.difficulties() {
            terms.push(difficulty.label().to_owned());
        }
        for boss in table.bosses() {
            terms.push(boss.short().to_owned());
            terms.push(boss.full().to_owned());
            terms.extend(boss.aliases().iter().cloned());
            terms.extend(boss.difficulties().iter().map(|l| boss.canonical(l)));
        }
        Self::builtin().with_terms(terms)
    }

    /// Adds terms (each whole and word by word).
    pub fn with_terms<I, S>(mut self, terms: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for term in terms {
            self.terms.extend(name_keys(term.as_ref()));
        }
        self
    }

    pub fn len(&self) -> usize {
        self.terms.len() + self.stopwords.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `name` may never be a pseudonym: a near-match of a term or equal
    /// to a stopword (after normalisation).
    pub fn excludes(&self, name: &str) -> bool {
        let key = normalise(name);
        self.stopwords.contains(&key) || self.terms.iter().any(|term| near_match(&key, term))
    }

    /// Whether a member name equals a code word, so masking it also masks that
    /// word in member text.
    pub fn collides(&self, name: &str) -> bool {
        let key = normalise(name);
        self.terms.contains(&key) || self.stopwords.contains(&key)
    }
}

impl fmt::Debug for CodeLexicon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CodeLexicon")
            .field("terms", &self.terms.len())
            .field("stopwords", &self.stopwords.len())
            .finish()
    }
}

fn split(list: &str) -> impl Iterator<Item = &str> {
    list.split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
}
