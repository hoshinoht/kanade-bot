use std::{collections::HashSet, fmt, sync::LazyLock};

use serde_json::Value;

use super::super::pseudonym::matcher::is_word;
use crate::domain::catalog::BossTable;
use crate::infrastructure::llm::{schema_instruction, wire::EMPTY_TOOL_RESULT};

static BUILTIN: LazyLock<ScanExemptions> = LazyLock::new(|| {
    ScanExemptions::default()
        .with_texts([EMPTY_TOOL_RESULT])
        .with_texts(schema_instruction(&Value::Null).ok())
});

/// Words of code-owned prompt text (whole words, Unicode lowercase). The
/// boundary scanner skips a masked name only when every word of it is here,
/// so a member called `Will` does not refuse a prompt whose code-owned rules
/// say "will"; a name absent from code text (`Ken`) is always scanned.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ScanExemptions {
    words: HashSet<String>,
}

impl ScanExemptions {
    /// Only what the runner itself adds to a request: its empty-tool-result
    /// placeholder and schema instruction. Everything else comes from each
    /// role's real code-owned sources.
    pub fn builtin() -> Self {
        BUILTIN.clone()
    }

    /// Adds every word of each text.
    pub fn with_texts<I, S>(mut self, texts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for text in texts {
            self.words.extend(words(text.as_ref()));
        }
        self
    }

    /// Adds every key and string of a code-owned JSON value (tool
    /// definitions, output schemas).
    pub fn with_value(mut self, value: &Value) -> Self {
        let mut stack = vec![value];
        while let Some(value) = stack.pop() {
            match value {
                Value::String(text) => self.words.extend(words(text)),
                Value::Array(items) => stack.extend(items),
                Value::Object(map) => {
                    for (key, item) in map {
                        self.words.extend(words(key));
                        stack.push(item);
                    }
                }
                Value::Null | Value::Bool(_) | Value::Number(_) => {}
            }
        }
        self
    }

    /// Adds the words of every boss short/full name, alias, canonical form
    /// and difficulty label, as the BOSSES table and boss tools render them.
    pub fn with_boss_table(self, table: &BossTable) -> Self {
        let mut terms: Vec<String> = table
            .difficulties()
            .iter()
            .map(|difficulty| difficulty.label().to_owned())
            .collect();
        for boss in table.bosses() {
            terms.push(boss.short().to_owned());
            terms.push(boss.full().to_owned());
            terms.extend(boss.aliases().iter().cloned());
            terms.extend(boss.difficulties().iter().map(|l| boss.canonical(l)));
        }
        self.with_texts(terms)
    }

    pub fn extend(mut self, other: &Self) -> Self {
        self.words.extend(other.words.iter().cloned());
        self
    }

    /// A name is exempt when it has at least one word and all its words are
    /// code-owned.
    pub fn covers(&self, name: &str) -> bool {
        let mut words = words(name).peekable();
        words.peek().is_some() && words.all(|word| self.words.contains(&word))
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

impl fmt::Debug for ScanExemptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ScanExemptions")
            .field("words", &self.words.len())
            .finish()
    }
}

fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !is_word(c))
        .filter(|word| !word.is_empty())
        .map(|word| word.chars().flat_map(char::to_lowercase).collect())
}
