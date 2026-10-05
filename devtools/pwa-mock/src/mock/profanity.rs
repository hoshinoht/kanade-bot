//! The chat profanity guardrail settings (`profanity`), mirroring the
//! server's PATCH rules and 422s. The built-in list here is a mild invented
//! placeholder: the real code-owned list stays in the server.

use serde::Serialize;
use serde_json::{Map, Value, json};

/// Invented placeholder words standing in for the server's built-in list.
pub const BUILTIN_WORDS: [&str; 6] = ["blarg", "drat", "frak", "gorram", "smeg", "zounds"];
/// The server's default line.
pub const DEFAULT_LINE: &str =
    "Ochitsuite! Let's keep it clean in here. Ask me again nicely and I'll help.";
const MAX_WORDS: usize = 100;
const WORD_CHARS: std::ops::RangeInclusive<usize> = 2..=32;
const MAX_LINE_CHARS: usize = 200;

#[derive(Clone, Serialize)]
pub struct Settings {
    pub extra_words: Vec<String>,
    pub allowed_words: Vec<String>,
    pub check_questions: bool,
    pub check_replies: bool,
    pub deflection_line: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            extra_words: Vec::new(),
            allowed_words: Vec::new(),
            check_questions: true,
            check_replies: true,
            deflection_line: DEFAULT_LINE.into(),
        }
    }
}

fn words(value: &Value, path: &str) -> Result<Vec<String>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{path} must be an array of words."))?;
    if items.len() > MAX_WORDS {
        return Err(format!("{path} holds at most {MAX_WORDS} words."));
    }
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let word = item
            .as_str()
            .map(|word| word.trim().to_lowercase())
            .filter(|word| {
                WORD_CHARS.contains(&word.chars().count()) && word.chars().all(char::is_alphabetic)
            })
            .ok_or_else(|| {
                format!("{path} takes single words of 2-32 letters (no digits, spaces or symbols).")
            })?;
        if out.contains(&word) {
            return Err(format!("“{word}” is listed twice in {path}."));
        }
        out.push(word);
    }
    Ok(out)
}

fn flag(value: &Value, path: &str) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("{path} must be true or false."))
}

impl Settings {
    pub fn view(&self) -> Value {
        let mut view = json!(self);
        view["builtin_words"] = json!(BUILTIN_WORDS);
        view
    }

    /// Whole words of the effective list (placeholder matching: lowercase
    /// letter runs, an optional trailing `s`).
    fn listed(&self, text: &str) -> Option<String> {
        let effective: Vec<&str> = BUILTIN_WORDS
            .iter()
            .copied()
            .filter(|word| !self.allowed_words.iter().any(|allowed| allowed == word))
            .chain(self.extra_words.iter().map(String::as_str))
            .collect();
        text.to_lowercase()
            .split(|c: char| !c.is_alphabetic())
            .find_map(|word| {
                effective
                    .iter()
                    .find(|entry| word == **entry || word.strip_suffix('s') == Some(entry))
                    .map(|entry| (*entry).to_owned())
            })
    }

    /// The section merged onto `self`, refused (422 message) as the server does.
    pub fn patch(&self, body: &Map<String, Value>) -> Result<Self, String> {
        let mut next = self.clone();
        if let Some(value) = body.get("extra_words") {
            next.extra_words = words(value, "profanity.extra_words")?;
        }
        if let Some(value) = body.get("allowed_words") {
            next.allowed_words = words(value, "profanity.allowed_words")?;
        }
        if let Some(value) = body.get("check_questions") {
            next.check_questions = flag(value, "profanity.check_questions")?;
        }
        if let Some(value) = body.get("check_replies") {
            next.check_replies = flag(value, "profanity.check_replies")?;
        }
        if let Some(value) = body.get("deflection_line") {
            next.deflection_line = value
                .as_str()
                .map(str::trim)
                .filter(|line| {
                    !line.is_empty()
                        && line.chars().count() <= MAX_LINE_CHARS
                        && !line.chars().any(char::is_control)
                })
                .ok_or_else(|| {
                    format!("The deflection line is one line of 1-{MAX_LINE_CHARS} characters.")
                })?
                .to_owned();
        }
        if let Some(word) = next
            .allowed_words
            .iter()
            .find(|word| !BUILTIN_WORDS.contains(&word.as_str()))
        {
            return Err(format!(
                "“{word}” is not on the built-in list, so it cannot be allowed again."
            ));
        }
        if let Some(word) = next
            .extra_words
            .iter()
            .find(|word| BUILTIN_WORDS.contains(&word.as_str()))
        {
            return Err(format!("“{word}” is already on the built-in list."));
        }
        if let Some(word) = next.listed(&next.deflection_line) {
            return Err(format!(
                "The deflection line uses the listed word “{word}”."
            ));
        }
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(body: Value) -> Result<Settings, String> {
        Settings::default().patch(body.as_object().unwrap())
    }

    #[test]
    fn words_are_normalised_and_bad_values_refused() {
        let saved = patch(json!({
            "extra_words": [" Heck "],
            "allowed_words": ["drat"],
            "deflection_line": "  Language, please!  ",
        }))
        .unwrap();
        assert_eq!(saved.extra_words, ["heck"]);
        assert_eq!(saved.allowed_words, ["drat"]);
        assert_eq!(saved.deflection_line, "Language, please!");
        for bad in [
            json!({ "extra_words": ["h3ck"] }),
            json!({ "extra_words": ["heck", "HECK"] }),
            json!({ "extra_words": ["frak"] }),
            json!({ "allowed_words": ["heck"] }),
            json!({ "check_replies": "no" }),
            json!({ "deflection_line": "" }),
            json!({ "deflection_line": "Oh frak, ask nicely." }),
            json!({ "extra_words": ["clean"] }),
        ] {
            assert!(patch(bad.clone()).is_err(), "{bad}");
        }
    }
}
