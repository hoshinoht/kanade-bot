//! Boss facts in a dated sentence, checked against the guild catalog: every
//! named boss, difficulty word, prefixed token and bold span must be the
//! picked run's.

use std::sync::LazyLock;

use regex::Regex;

use super::super::{is_word, pattern};
use crate::domain::catalog::BossTable;

static BOLD: LazyLock<Regex> = LazyLock::new(|| pattern(r"\*\*(.+?)\*\*"));
/// Catalog aliases that are everyday words or people's names; written in
/// lowercase they name a boss only after a difficulty label, in bold or
/// inside a longer name (`climb` and `clot` read as prefixed `c`+`limb`/`lot`).
const ORDINARY: [&str; 9] = [
    "will", "lot", "star", "bell", "bella", "carl", "karl", "climb", "clot",
];
/// The longest `difficulty + name` phrase tried (a difficulty plus the
/// catalog's five-word phrase limit).
const PHRASE_WORDS: usize = 6;

/// The run's bosses as the catalog reads its label (`Hard MaleficStar + Hard
/// FA`): short names, and canonical `HMaleficStar`-style tokens.
pub(super) struct Own {
    names: Vec<String>,
    tokens: Vec<String>,
}

impl Own {
    pub(super) fn of(label: &str, catalog: &BossTable) -> Self {
        Own {
            names: catalog.names_in(label),
            tokens: catalog.parse(label).unwrap_or_default(),
        }
    }
}

/// `needle` in `hay` between non-word characters (emulated lookaround).
fn has_word(hay: &str, needle: &str) -> bool {
    !needle.is_empty()
        && hay.match_indices(needle).any(|(at, _)| {
            !hay[..at].chars().next_back().is_some_and(is_word)
                && !hay[at + needle.len()..].chars().next().is_some_and(is_word)
        })
}

/// ASCII words of `lowered` with their byte offsets.
fn words(lowered: &str) -> Vec<(usize, &str)> {
    lowered
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(|word| (word.as_ptr() as usize - lowered.as_ptr() as usize, word))
        .collect()
}

/// `lowered` with each lowercase, unmarked [`ORDINARY`] word blanked out, so
/// `will` or `a lot` names no boss while `Karl`, `Hard Will` or `**will**`
/// does. `line` is the original text at the same byte offsets.
fn without_ordinary(lowered: &str, line: &str, catalog: &BossTable) -> String {
    let labels: Vec<String> = catalog
        .difficulties()
        .iter()
        .map(|difficulty| difficulty.label().to_lowercase())
        .collect();
    let bold: Vec<(usize, usize)> = BOLD
        .find_iter(lowered)
        .map(|m| (m.start(), m.end()))
        .collect();
    let words = words(lowered);
    let mut masked = lowered.to_owned();
    for (index, &(at, word)) in words.iter().enumerate() {
        if !ORDINARY.contains(&word) {
            continue;
        }
        let previous = index.checked_sub(1).map(|i| words[i].1);
        let next = words.get(index + 1).map(|&(_, next)| next);
        let joined = |a: &str, b: &str| !catalog.names_in(&format!("{a}{b}")).is_empty();
        let marked = line[at..].starts_with(|c: char| c.is_ascii_uppercase())
            || previous.is_some_and(|p| labels.iter().any(|label| label == p))
            || bold.iter().any(|&(start, end)| start <= at && at < end)
            || previous.is_some_and(|p| joined(p, word))
            || next.is_some_and(|n| joined(word, n));
        if !marked {
            masked.replace_range(at..at + word.len(), &" ".repeat(word.len()));
        }
    }
    masked
}

/// Every difficulty word must start a `difficulty + boss` phrase that is
/// one of the run's tokens, and so must a difficulty letter or `hm`-style
/// shorthand standing before a boss (`N Carling`, `N-Carling`); every
/// prefixed token (`ncarling`) must be one too. A difficulty word with no
/// boss after it (`hard to say`) fails; a lone letter does not name one.
fn difficulties_match(masked: &str, own: &Own, catalog: &BossTable) -> bool {
    let difficulties: Vec<(String, String)> = catalog
        .difficulties()
        .iter()
        .map(|d| (d.letter().to_lowercase(), d.label().to_lowercase()))
        .collect();
    let difficulty = |word: &str| {
        difficulties.iter().find_map(|(letter, label)| {
            let shorthand = word == letter || word.strip_prefix(letter.as_str()) == Some("m");
            (word == label)
                .then_some((label, false))
                .or(shorthand.then_some((label, true)))
        })
    };
    let words: Vec<&str> = words(masked).into_iter().map(|(_, word)| word).collect();
    words.iter().enumerate().all(|(index, word)| {
        if let Some((label, shorthand)) = difficulty(word) {
            let top = words.len().min(index + PHRASE_WORDS);
            let phrase = (index + 2..=top).rev().find_map(|end| {
                let phrase = format!("{label} {}", words[index + 1..end].join(" "));
                match catalog.parse(&phrase) {
                    Ok(tokens) if tokens.len() == 1 => Some(own.tokens.contains(&tokens[0])),
                    _ => None,
                }
            });
            return phrase.unwrap_or(shorthand);
        }
        // A prefixed token names the same boss as its tail.
        let named = catalog.names_in(word);
        let prefixed = word.len() > 1 && !named.is_empty() && catalog.names_in(&word[1..]) == named;
        !prefixed
            || catalog
                .parse_token(word)
                .is_ok_and(|token| own.tokens.contains(&token))
    })
}

/// Whether every catalog boss the sentence names (any case, alias or
/// prefixed form), every difficulty and every bold span is the run's.
pub(super) fn bosses_match(lowered: &str, line: &str, label: &str, catalog: &BossTable) -> bool {
    let own = Own::of(label, catalog);
    let ours = |said: &str| {
        let named = catalog.names_in(said);
        !named.is_empty() && named.iter().all(|short| own.names.contains(short))
    };
    let masked = without_ordinary(lowered, line, catalog);
    catalog
        .names_in(&masked)
        .iter()
        .all(|short| own.names.contains(short))
        && difficulties_match(&masked, &own, catalog)
        && BOLD
            .captures_iter(lowered)
            .all(|c| has_word(label, c[1].trim()) || ours(&c[1]))
}
