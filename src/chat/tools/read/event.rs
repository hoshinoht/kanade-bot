//! Event bosses: knowledge documents outside the catalog, named by their key
//! or an alias, optionally with a difficulty word around the name.

use crate::domain::catalog::BossTable;

/// One event boss a guide store can render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventBoss {
    pub key: String,
    pub name: String,
    pub availability: String,
    pub aliases: Vec<String>,
}

/// Lowercased (Unicode-aware) with whitespace and separators removed.
fn squash(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn names_match(event: &EventBoss, key: &str) -> bool {
    !key.is_empty()
        && std::iter::once(&event.key)
            .chain(&event.aliases)
            .any(|name| squash(name) == key)
}

/// The event named by `text` and the difficulty letter stated with it.
///
/// Accepts the key or an alias (case-insensitive), optionally preceded or
/// followed by a catalog difficulty label or prefix letter, or glued to a
/// prefix letter like a canonical token (`HKai`).
///
/// # Errors
/// The two difficulty labels when the words state conflicting ones.
pub fn match_event<'a>(
    events: &'a [EventBoss],
    catalog: &BossTable,
    text: &str,
) -> Result<Option<(&'a EventBoss, Option<String>)>, String> {
    let words: Vec<String> = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '/' | '+' | '&'))
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    let stated = |word: &str| {
        catalog
            .difficulties()
            .iter()
            .find(|d| d.label().to_lowercase() == word || d.letter() == word)
            .map(|d| d.letter().to_owned())
    };
    let mut start = 0;
    let mut end = words.len();
    let mut letters: Vec<String> = Vec::new();
    // Peel difficulty words only while a name remains between them.
    if end - start > 1
        && let Some(letter) = stated(&words[start])
    {
        letters.push(letter);
        start += 1;
    }
    if end - start > 1
        && let Some(letter) = stated(&words[end - 1])
    {
        letters.push(letter);
        end -= 1;
    }
    let key = squash(&words[start..end].concat());
    let found = events.iter().find(|event| names_match(event, &key));
    let found = match found {
        Some(event) => Some(event),
        None if letters.is_empty() => {
            // A canonical-token style prefix, e.g. `HKai`.
            let mut chars = key.chars();
            let letter = chars
                .next()
                .map(String::from)
                .filter(|l| stated(l).is_some());
            let rest = chars.as_str();
            match letter.and_then(|l| Some((l, events.iter().find(|e| names_match(e, rest))?))) {
                Some((letter, event)) => {
                    letters.push(letter);
                    Some(event)
                }
                None => None,
            }
        }
        None => None,
    };
    let Some(event) = found else {
        return Ok(None);
    };
    letters.dedup();
    match letters.as_slice() {
        [] => Ok(Some((event, None))),
        [letter] => Ok(Some((event, Some(letter.clone())))),
        [first, second, ..] => Err(format!(
            "conflicting difficulties: {} and {}",
            catalog.difficulty_name(first),
            catalog.difficulty_name(second)
        )),
    }
}
