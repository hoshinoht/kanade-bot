use super::error::BossParseError;
use super::table::BossTable;
use super::text::{is_list_separator, normalise, split_letter};
use crate::domain::pytext::{split_whitespace, strip};

/// Longest word run joined against the alias table: a difficulty word plus a
/// spaced three-word name, without unbounded backtracking.
const MAX_PHRASE_WORDS: usize = 5;

impl BossTable {
    /// Resolve one token, e.g. `ncarling` -> `NCarling`.
    ///
    /// # Errors
    /// [`BossParseError`] for an empty, bare, wrong-difficulty or unknown token.
    pub fn parse_token(&self, token: &str) -> Result<String, BossParseError> {
        let key = normalise(token);
        if key.is_empty() {
            return Err(BossParseError::new("empty boss token"));
        }
        if let Some(at) = self.alias(&key) {
            return Err(BossParseError::new(
                self.missing_difficulty(token, &self.bosses[at]),
            ));
        }
        if let Some((letter, rest)) = split_letter(&key)
            && let (Some(_), Some(at)) = (self.difficulty(letter), self.alias(rest))
        {
            let boss = &self.bosses[at];
            if !boss.has_difficulty(letter) {
                return Err(BossParseError::new(self.wrong_difficulty(boss, letter)));
            }
            return Ok(boss.canonical(letter));
        }
        Err(BossParseError::new(format!("unknown boss `{token}`")))
    }

    /// Parse a comma/space separated boss list into unique canonical tokens,
    /// in the order given.
    ///
    /// Multi-word names match greedily, and a leading spelled-out difficulty
    /// (`Normal black mage`) folds into the prefix.
    ///
    /// # Errors
    /// [`BossParseError`] reporting every bad token at once, joined by `; `.
    pub fn parse(&self, text: &str) -> Result<Vec<String>, BossParseError> {
        let chunks: Vec<&str> = text
            .split(is_list_separator)
            .filter(|chunk| !strip(chunk).is_empty())
            .collect();
        if chunks.is_empty() {
            return Err(BossParseError::new("no bosses given"));
        }
        let letters = self.difficulty_words();
        let mut out: Vec<String> = Vec::new();
        let mut problems: Vec<String> = Vec::new();
        for chunk in chunks {
            let words: Vec<&str> = split_whitespace(chunk).collect();
            let mut index = 0;
            while index < words.len() {
                let word = words[index];
                if let Some(&letter) = letters.get(&word.to_lowercase()) {
                    // Fold a spoken difficulty only when a boss follows, so a
                    // stray "hard" stays an unknown boss rather than a silent prefix.
                    if let Some((at, length)) = self.longest_alias(&words, index + 1) {
                        let boss = &self.bosses[at];
                        if boss.has_difficulty(letter) {
                            push_unique(&mut out, boss.canonical(letter));
                        } else {
                            problems.push(self.wrong_difficulty(boss, letter));
                        }
                        index += 1 + length;
                        continue;
                    }
                } else if let Some((at, length)) = self.longest_alias(&words, index) {
                    let spoken = words[index..index + length].join(" ");
                    problems.push(self.missing_difficulty(&spoken, &self.bosses[at]));
                    index += length;
                    continue;
                }
                match self.parse_token(word) {
                    Ok(canonical) => push_unique(&mut out, canonical),
                    Err(error) => problems.push(error.message().to_owned()),
                }
                index += 1;
            }
        }
        if problems.is_empty() {
            Ok(out)
        } else {
            Err(BossParseError::new(problems.join("; ")))
        }
    }

    /// Short names of the bosses a loose sentence names, in the order said.
    ///
    /// Difficulty-agnostic and forgiving, unlike [`BossTable::parse`]: it answers
    /// "did they name a boss at all", so `hard jupiter`, `hjup` and `jupiter` all
    /// name Jupiter while weekdays, stray words and run ids name nothing.
    pub fn names_in(&self, text: &str) -> Vec<String> {
        let words: Vec<&str> = text
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|word| !word.is_empty())
            .collect();
        let mut out: Vec<String> = Vec::new();
        let mut index = 0;
        while index < words.len() {
            let (found, length) = match self.longest_alias(&words, index) {
                Some((at, length)) => (Some(at), length),
                None => (self.prefixed_alias(words[index]), 1),
            };
            if let Some(at) = found {
                let short = &self.bosses[at].short;
                if !out.contains(short) {
                    out.push(short.clone());
                }
            }
            index += length;
        }
        out
    }

    /// Boss named by a prefixed token such as `hjup`, ignoring the difficulty.
    fn prefixed_alias(&self, word: &str) -> Option<usize> {
        let key = normalise(word);
        let (letter, rest) = split_letter(&key)?;
        self.difficulty(letter)?;
        self.alias(rest)
    }

    /// Longest alias phrase at `words[start..]` as `(boss index, word count)`;
    /// longest first so "the first adversary" wins over "the".
    fn longest_alias(&self, words: &[&str], start: usize) -> Option<(usize, usize)> {
        let top = words.len().min(start + MAX_PHRASE_WORDS);
        (start + 1..=top).rev().find_map(|end| {
            self.alias(&normalise(&words[start..end].join(" ")))
                .map(|at| (at, end - start))
        })
    }
}

fn push_unique(out: &mut Vec<String>, canonical: String) {
    if !out.contains(&canonical) {
        out.push(canonical);
    }
}
