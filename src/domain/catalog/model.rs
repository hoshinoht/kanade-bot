/// One catalog difficulty: its lowercase prefix letter and display label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Difficulty {
    pub(super) letter: String,
    pub(super) label: String,
}

impl Difficulty {
    /// Lowercase prefix, e.g. `h`.
    pub fn letter(&self) -> &str {
        &self.letter
    }

    /// Display label, e.g. `Hard`.
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// One validated catalog boss.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Boss {
    pub(super) short: String,
    pub(super) full: String,
    pub(super) level: Option<u64>,
    pub(super) difficulties: Vec<String>,
    pub(super) aliases: Vec<String>,
    pub(super) portrait: Option<String>,
    pub(super) guide_colour: Option<u32>,
}

impl Boss {
    pub fn short(&self) -> &str {
        &self.short
    }

    pub fn full(&self) -> &str {
        &self.full
    }

    pub fn level(&self) -> Option<u64> {
        self.level
    }

    /// Prefix letters this boss has in game, in catalog order.
    pub fn difficulties(&self) -> &[String] {
        &self.difficulties
    }

    pub fn aliases(&self) -> &[String] {
        &self.aliases
    }

    /// Explicit portrait basename; otherwise portraits are looked up by short name.
    pub fn portrait(&self) -> Option<&str> {
        self.portrait.as_deref()
    }

    /// RGB colour for the guide surface.
    pub fn guide_colour(&self) -> Option<u32> {
        self.guide_colour
    }

    pub(super) fn has_difficulty(&self, letter: &str) -> bool {
        self.difficulties.iter().any(|own| own == letter)
    }

    /// Canonical token for `letter`, e.g. `HStar`.
    pub fn canonical(&self, letter: &str) -> String {
        format!("{}{}", letter.to_uppercase(), self.short)
    }
}

/// A boss named outside the scheduling grammar.
///
/// `difficulty` is a validated prefix when the speaker gave one; a bare name
/// deliberately leaves it `None` rather than choosing one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BossReference {
    pub short: String,
    pub difficulty: Option<String>,
}

/// The parts of a canonical token, for rich rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BossDetail {
    pub token: String,
    pub short: String,
    pub full: String,
    pub level: Option<u64>,
    pub letter: String,
    pub difficulty: String,
}
