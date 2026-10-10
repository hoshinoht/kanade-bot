use std::collections::HashMap;

use super::model::{Boss, BossDetail, Difficulty};

/// An immutable, validated boss catalog; build it with [`BossTable::from_spec`].
#[derive(Clone, Debug)]
pub struct BossTable {
    pub(super) difficulties: Vec<Difficulty>,
    pub(super) bosses: Vec<Boss>,
    pub(super) by_short: HashMap<String, usize>,
    /// Normalised alias -> index into `bosses`.
    pub(super) aliases: HashMap<String, usize>,
}

impl BossTable {
    /// Difficulties in catalog order.
    pub fn difficulties(&self) -> &[Difficulty] {
        &self.difficulties
    }

    /// Bosses in catalog order.
    pub fn bosses(&self) -> &[Boss] {
        &self.bosses
    }

    pub fn boss(&self, short: &str) -> Option<&Boss> {
        self.by_short.get(short).map(|&at| &self.bosses[at])
    }

    /// Prefix letters joined for messages, e.g. `n/h/x`.
    pub fn prefixes(&self) -> String {
        let letters: Vec<&str> = self.difficulties.iter().map(Difficulty::letter).collect();
        letters.join("/")
    }

    /// The canonical names a boss actually has, e.g. `NKalos, XKalos`.
    pub fn valid_forms(&self, short: &str) -> Option<String> {
        self.boss(short).map(valid_forms)
    }

    /// `HStar` -> its difficulty and boss, or `None` if it is not in this catalog.
    pub fn split(&self, canonical: &str) -> Option<(&Difficulty, &Boss)> {
        let mut chars = canonical.chars();
        let letter = chars.next()?.to_lowercase().to_string();
        let boss = self.boss(chars.as_str())?;
        Some((self.difficulty(&letter)?, boss))
    }

    /// `HStar` -> `Radiant Star (Hard, Lv280)`; unknown names come back unchanged.
    pub fn describe(&self, canonical: &str) -> String {
        let Some((difficulty, boss)) = self.split(canonical) else {
            return canonical.to_owned();
        };
        match boss.level {
            Some(level) => format!("{} ({}, Lv{level})", boss.full, difficulty.label),
            None => format!("{} ({})", boss.full, difficulty.label),
        }
    }

    pub fn describe_all<S: AsRef<str>>(&self, canonicals: &[S]) -> String {
        let described: Vec<String> = canonicals
            .iter()
            .map(|name| self.describe(name.as_ref()))
            .collect();
        described.join(" · ")
    }

    /// The parts of a canonical name, or `None` if it is not in this catalog.
    pub fn detail(&self, canonical: &str) -> Option<BossDetail> {
        let (difficulty, boss) = self.split(canonical)?;
        Some(BossDetail {
            token: canonical.to_owned(),
            short: boss.short.clone(),
            full: boss.full.clone(),
            level: boss.level,
            letter: difficulty.letter.clone(),
            difficulty: difficulty.label.clone(),
        })
    }

    /// `h` -> `Hard`; unknown letters come back uppercased.
    pub fn difficulty_name(&self, letter: &str) -> String {
        let lowered = letter.to_lowercase();
        match self.difficulty(&lowered) {
            Some(difficulty) => difficulty.label.clone(),
            None => letter.to_uppercase(),
        }
    }

    /// Bosses in in-game list order: by level (unknown first), then short name.
    pub fn ordered(&self) -> Vec<&Boss> {
        let mut bosses: Vec<&Boss> = self.bosses.iter().collect();
        bosses.sort_by(|a, b| {
            (a.level.unwrap_or(0), &a.short).cmp(&(b.level.unwrap_or(0), &b.short))
        });
        bosses
    }

    pub(super) fn difficulty(&self, letter: &str) -> Option<&Difficulty> {
        self.difficulties.iter().find(|d| d.letter == letter)
    }

    pub(super) fn label<'a>(&'a self, letter: &'a str) -> &'a str {
        self.difficulty(letter).map_or(letter, Difficulty::label)
    }

    pub(super) fn alias(&self, key: &str) -> Option<usize> {
        self.aliases.get(key).copied()
    }

    /// Lowercased difficulty label -> letter; a later duplicate label wins.
    pub(super) fn difficulty_words(&self) -> HashMap<String, &str> {
        self.difficulties
            .iter()
            .map(|d| (d.label.to_lowercase(), d.letter.as_str()))
            .collect()
    }

    pub(super) fn wrong_difficulty(&self, boss: &Boss, letter: &str) -> String {
        format!(
            "{} has no {} difficulty - did you mean {}?",
            boss.full,
            self.label(letter),
            valid_forms(boss)
        )
    }

    pub(super) fn missing_difficulty(&self, token: &str, boss: &Boss) -> String {
        format!(
            "`{token}` is missing a difficulty prefix ({}) - try {}",
            self.prefixes(),
            valid_forms(boss)
        )
    }
}

fn valid_forms(boss: &Boss) -> String {
    let forms: Vec<String> = boss
        .difficulties
        .iter()
        .map(|l| boss.canonical(l))
        .collect();
    forms.join(", ")
}
