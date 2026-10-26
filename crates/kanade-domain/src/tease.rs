//! The running-joke note an admin may give an opted-in member (D10): at most
//! 80 characters of plain text that never names anyone on the roster. The
//! same rules are checked when the note is saved and again before it may be
//! sent, since names change after it was written.

/// The longest note, in characters.
pub const NOTE_MAX_CHARS: usize = 80;

/// Why a note is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteProblem {
    /// Over [`NOTE_MAX_CHARS`].
    TooLong,
    /// A mention, an `@`/`#` handle, a link, markdown or a line break.
    NotPlain,
    /// It contains a roster member's name, alias or IGN.
    NamesMember,
}

/// Characters that make Discord markdown, mentions or handles.
const NOT_PLAIN: &[char] = &['@', '#', '<', '>', '*', '_', '`', '~', '|', '[', ']', '\\'];

/// The note's problem, if any. `note` is the trimmed text; `names` are every
/// roster member's display name, nickname and aliases and every roster
/// character's IGN, matched case-insensitively as whole words.
pub fn note_problem<'a>(
    note: &str,
    names: impl IntoIterator<Item = &'a str>,
) -> Option<NoteProblem> {
    if note.chars().count() > NOTE_MAX_CHARS {
        return Some(NoteProblem::TooLong);
    }
    let lower = note.to_lowercase();
    if note
        .chars()
        .any(|c| c.is_control() || NOT_PLAIN.contains(&c))
        || lower.contains("://")
        || lower.contains("www.")
    {
        return Some(NoteProblem::NotPlain);
    }
    names
        .into_iter()
        .any(|name| names_word(&lower, &name.trim().to_lowercase()))
        .then_some(NoteProblem::NamesMember)
}

/// `name` appears in `text` with no ASCII letter or digit right before or
/// after it ("ali" is not in "alibi"; a name in a script without spaces
/// matches anywhere).
fn names_word(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    text.match_indices(name).any(|(at, _)| {
        !word(text[..at].chars().next_back()) && !word(text[at + name.len()..].chars().next())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: [&str; 4] = ["Alicia Quartz", "ali", "Bobbington", "ムーン"];

    #[test]
    fn plain_notes_pass() {
        for note in [
            "Still owes the guild a Lucid clear",
            "Forgot his alibi again",
            "Quartz is a mineral",
            &"x".repeat(80),
        ] {
            assert_eq!(note_problem(note, NAMES), None, "{note}");
        }
    }

    #[test]
    fn refusals() {
        for (note, problem) in [
            ("x".repeat(81).as_str(), NoteProblem::TooLong),
            ("pinging <@123>", NoteProblem::NotPlain),
            ("ask @someone", NoteProblem::NotPlain),
            ("lives in #general", NoteProblem::NotPlain),
            ("see https://example.invalid", NoteProblem::NotPlain),
            ("see www.example.invalid", NoteProblem::NotPlain),
            ("**loud**", NoteProblem::NotPlain),
            ("two\nlines", NoteProblem::NotPlain),
            ("Carried by ALICIA QUARTZ again", NoteProblem::NamesMember),
            ("Ali dropped the pot", NoteProblem::NamesMember),
            ("bobbington's biggest fan", NoteProblem::NamesMember),
            ("ムーンの友達", NoteProblem::NamesMember),
        ] {
            assert_eq!(note_problem(note, NAMES), Some(problem), "{note}");
        }
    }
}
