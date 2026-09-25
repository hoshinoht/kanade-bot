use std::collections::BTreeSet;

use super::error::BossParseError;
use super::model::BossReference;
use super::table::BossTable;
use super::text::{is_reference_separator, normalise, split_letter};

/// A boss index with the difficulty letter the speaker stated, if any.
type Found<'a> = (usize, Option<&'a str>);

impl BossTable {
    /// Resolve exactly one freely written boss reference without guessing.
    ///
    /// For knowledge and guide lookups, where a bare name is useful because it
    /// schedules nothing. Accepts an alias or full name, a canonical token, or a
    /// spelled-out difficulty phrase anywhere in a sentence.
    ///
    /// # Errors
    /// [`BossParseError`] when no boss, several bosses, or conflicting or
    /// unavailable difficulties are named.
    pub fn resolve_reference(&self, text: &str) -> Result<BossReference, BossParseError> {
        let words: Vec<&str> = text
            .split(is_reference_separator)
            .filter(|word| !word.is_empty())
            .collect();
        if words.is_empty() {
            return Err(BossParseError::new("no boss given"));
        }
        // A complete (possibly multi-word) name is the unambiguous case.
        if let Some(found) = self.reference_from_key(&normalise(text))? {
            return Ok(self.reference(found));
        }

        // Otherwise match every contiguous phrase so full names and spelled
        // difficulty phrases stay intact wherever they occur in a sentence.
        let difficulty_words = self.difficulty_words();
        let keys: Vec<String> = words.iter().map(|word| normalise(word)).collect();
        let mut found: Vec<(usize, BTreeSet<&str>)> = Vec::new();
        for start in 0..words.len() {
            let stated = difficulty_words.get(&words[start].to_lowercase()).copied();
            // normalise() distributes over concatenation, so phrase keys grow by word.
            let mut phrase = String::new();
            let mut after_stated = String::new();
            for end in start + 1..=words.len() {
                phrase.push_str(&keys[end - 1]);
                add(&mut found, self.reference_from_key(&phrase)?);
                if let Some(stated) = stated.filter(|_| end > start + 1) {
                    after_stated.push_str(&keys[end - 1]);
                    let embedded = self.reference_from_key(&after_stated)?;
                    add(&mut found, self.with_stated_difficulty(stated, embedded)?);
                }
            }
        }
        match found.as_slice() {
            [] => Err(BossParseError::new(format!("no boss found in `{text}`"))),
            [(at, letters)] => match letters.len() {
                0 | 1 => Ok(self.reference((*at, letters.first().copied()))),
                _ => {
                    let labels: Vec<&str> = letters.iter().map(|l| self.label(l)).collect();
                    Err(BossParseError::new(format!(
                        "conflicting difficulties: {}",
                        labels.join(", ")
                    )))
                }
            },
            _ => {
                let names: Vec<&str> = found
                    .iter()
                    .map(|(at, _)| self.bosses[*at].full.as_str())
                    .collect();
                Err(BossParseError::new(format!(
                    "multiple bosses found: {}",
                    names.join(", ")
                )))
            }
        }
    }

    /// One normalised name, optionally carrying a token prefix.
    fn reference_from_key<'a>(&'a self, key: &str) -> Result<Option<Found<'a>>, BossParseError> {
        if let Some(at) = self.alias(key) {
            return Ok(Some((at, None)));
        }
        if key.len() <= 1 {
            return Ok(None);
        }
        let Some((letter, alias)) = split_letter(key) else {
            return Ok(None);
        };
        let (Some(difficulty), Some(at)) = (self.difficulty(letter), self.alias(alias)) else {
            return Ok(None);
        };
        let boss = &self.bosses[at];
        if !boss.has_difficulty(letter) {
            return Err(BossParseError::new(self.wrong_difficulty(boss, letter)));
        }
        Ok(Some((at, Some(difficulty.letter()))))
    }

    /// Apply a spelled difficulty word to a bare or token-style reference.
    fn with_stated_difficulty<'a>(
        &'a self,
        stated: &'a str,
        embedded: Option<Found<'a>>,
    ) -> Result<Option<Found<'a>>, BossParseError> {
        let Some((at, embedded_letter)) = embedded else {
            return Ok(None);
        };
        if let Some(letter) = embedded_letter.filter(|&letter| letter != stated) {
            return Err(BossParseError::new(format!(
                "conflicting difficulties: {} and {}",
                self.label(stated),
                self.label(letter)
            )));
        }
        let boss = &self.bosses[at];
        if !boss.has_difficulty(stated) {
            return Err(BossParseError::new(self.wrong_difficulty(boss, stated)));
        }
        Ok(Some((at, Some(stated))))
    }

    fn reference(&self, (at, letter): Found<'_>) -> BossReference {
        BossReference {
            short: self.bosses[at].short.clone(),
            difficulty: letter.map(str::to_owned),
        }
    }
}

fn add<'a>(found: &mut Vec<(usize, BTreeSet<&'a str>)>, candidate: Option<Found<'a>>) {
    let Some((at, letter)) = candidate else {
        return;
    };
    let index = match found.iter().position(|(seen, _)| *seen == at) {
        Some(index) => index,
        None => {
            found.push((at, BTreeSet::new()));
            found.len() - 1
        }
    };
    if let Some(letter) = letter {
        found[index].1.insert(letter);
    }
}
