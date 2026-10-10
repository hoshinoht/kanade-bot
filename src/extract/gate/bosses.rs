use crate::domain::catalog::{Boss, BossTable};

/// Difficulty spelled out, as it turns up in chat: `exkalos`, `hardstar`.
/// Tried in this order.
const WORD_PREFIXES: [(&str, &str); 7] = [
    ("easy", "e"),
    ("normal", "n"),
    ("norm", "n"),
    ("hard", "h"),
    ("chaos", "c"),
    ("extreme", "x"),
    ("ex", "x"),
];

/// Ordinary words that must never be read as a misspelt boss.
const FUZZY_STOPWORDS: [&str; 6] = ["start", "starting", "started", "clear", "chair", "cheap"];

/// The shortest alias a typo may match: two-letter aliases are one edit from
/// far too much.
const MIN_FUZZY_LENGTH: usize = 4;

/// One boss token found in a message.
///
/// `canonical` is `None` when the token named no difficulty, or one the boss
/// lacks: it still counts as a boss signal, but nothing downstream may invent
/// the missing prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BossHit {
    pub token: String,
    pub short: String,
    pub difficulty: Option<String>,
    pub canonical: Option<String>,
    pub fuzzy: bool,
}

/// A catalog's alias keys in v4 insertion order, which decides the first
/// fuzzy match.
#[derive(Clone, Debug)]
pub struct BossLexicon<'a> {
    table: &'a BossTable,
    aliases: Vec<(String, &'a Boss)>,
}

impl<'a> BossLexicon<'a> {
    pub fn new(table: &'a BossTable) -> Self {
        let mut aliases: Vec<(String, &'a Boss)> = Vec::new();
        for boss in table.bosses() {
            let names = [boss.short(), boss.full()]
                .into_iter()
                .chain(boss.aliases().iter().map(String::as_str));
            for name in names {
                let key = alias_key(name);
                if !key.is_empty() && !aliases.iter().any(|(known, _)| *known == key) {
                    aliases.push((key, boss));
                }
            }
        }
        Self { table, aliases }
    }

    fn exact(&self, key: &str) -> Option<&'a Boss> {
        self.aliases
            .iter()
            .find(|(alias, _)| alias == key)
            .map(|&(_, boss)| boss)
    }

    /// `key` (prefix already stripped) -> `(boss, was_fuzzy)`.
    fn resolve(&self, key: &str, allow_fuzzy: bool) -> Option<(&'a Boss, bool)> {
        if let Some(boss) = self.exact(key) {
            return Some((boss, false));
        }
        if !allow_fuzzy || key.len() < MIN_FUZZY_LENGTH || FUZZY_STOPWORDS.contains(&key) {
            return None;
        }
        self.aliases
            .iter()
            .find(|(alias, _)| alias.len() >= MIN_FUZZY_LENGTH && within_one_edit(key, alias))
            .map(|&(_, boss)| (boss, true))
    }

    fn is_difficulty(&self, letter: &str) -> bool {
        self.table
            .difficulties()
            .iter()
            .any(|d| d.letter() == letter)
    }
}

/// v4 `bosses._normalise`: only ASCII letters and digits survive.
fn alias_key(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

/// At most one insertion, deletion or substitution apart (ASCII keys).
fn within_one_edit(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a == b {
        return true;
    }
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    if a.len() == b.len() {
        return a.iter().zip(b).filter(|(x, y)| x != y).count() == 1;
    }
    let (short, long) = if a.len() < b.len() { (a, b) } else { (b, a) };
    let (mut i, mut j, mut skipped) = (0, 0, false);
    while i < short.len() && j < long.len() {
        if short[i] == long[j] {
            i += 1;
            j += 1;
            continue;
        }
        if skipped {
            return false;
        }
        skipped = true;
        j += 1;
    }
    true
}

fn hit(token: &str, boss: &Boss, letter: Option<&str>, fuzzy: bool) -> BossHit {
    let canonical = letter
        .filter(|letter| boss.difficulties().iter().any(|own| own == letter))
        .map(|letter| boss.canonical(letter));
    BossHit {
        token: token.to_owned(),
        short: boss.short().to_owned(),
        difficulty: letter.map(str::to_owned),
        canonical,
        fuzzy,
    }
}

fn token_hit(lexicon: &BossLexicon<'_>, key: &str) -> Option<BossHit> {
    // The whole token is an alias: `limbo`, `star` (never `h` + `fa`).
    if let Some((boss, _)) = lexicon.resolve(key, false) {
        return Some(hit(key, boss, None, false));
    }
    // Only a difficulty prefix makes a fuzzy token unambiguously a boss.
    for (word, letter) in WORD_PREFIXES {
        if let Some(rest) = key.strip_prefix(word).filter(|rest| !rest.is_empty())
            && let Some((boss, fuzzy)) = lexicon.resolve(rest, true)
        {
            return Some(hit(key, boss, Some(letter), fuzzy));
        }
    }
    let (letter, rest) = key.split_at(1);
    if lexicon.is_difficulty(letter) && !rest.is_empty() {
        let (boss, fuzzy) = lexicon.resolve(rest, true)?;
        return Some(hit(key, boss, Some(letter), fuzzy));
    }
    None
}

/// Every boss token in `text`, in order, de-duplicated by `(short, difficulty)`.
pub fn find_bosses(text: &str, lexicon: &BossLexicon<'_>) -> Vec<BossHit> {
    let lowered = text.to_lowercase();
    let mut out: Vec<BossHit> = Vec::new();
    for key in super::scan::words(&lowered) {
        if let Some(found) = token_hit(lexicon, key)
            && !out
                .iter()
                .any(|seen| seen.short == found.short && seen.difficulty == found.difficulty)
        {
            out.push(found);
        }
    }
    out
}

/// The canonical names among `hits`, in order, de-duplicated.
pub fn canonical_bosses(hits: &[BossHit]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for canonical in hits.iter().filter_map(|hit| hit.canonical.as_ref()) {
        if !out.contains(canonical) {
            out.push(canonical.clone());
        }
    }
    out
}
