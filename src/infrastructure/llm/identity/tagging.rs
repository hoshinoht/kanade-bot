//! Test-only codec: replaces identities with `ID<n>` tags so capture tests can
//! prove no raw roster name or snowflake reaches a provider request. Not the
//! stage-2 on mode (tags are sequential and matching is ASCII case folding).

use std::collections::BTreeMap;

use serde_json::Value;

use super::codec::{CodecMode, DecodeError, IdentityCodec, IdentitySession, Member};
use crate::infrastructure::llm::ChatRequest;

const SNOWFLAKE_DIGITS: std::ops::RangeInclusive<usize> = 17..=20;

#[derive(Clone, Copy, Debug, Default)]
pub struct TaggingCodec;

impl IdentityCodec for TaggingCodec {
    fn mode(&self) -> CodecMode {
        CodecMode::Pseudonymizing
    }

    fn open(&self, roster: &[Member]) -> Box<dyn IdentitySession> {
        Box::new(TaggingSession::new(roster))
    }
}

pub struct TaggingSession {
    roster: Vec<Member>,
    /// Name/alias needles, longest first, with the user id they stand for.
    needles: Vec<(String, String)>,
    issued: Vec<String>,
    tags: BTreeMap<String, usize>,
}

impl TaggingSession {
    pub fn new(roster: &[Member]) -> Self {
        let needles = sorted_needles(roster);
        Self {
            roster: roster.to_vec(),
            needles,
            issued: Vec::new(),
            tags: BTreeMap::new(),
        }
    }

    fn tag(&mut self, user_id: &str) -> String {
        let index = match self.tags.get(user_id) {
            Some(&index) => index,
            None => {
                self.issued.push(user_id.to_owned());
                self.tags.insert(user_id.to_owned(), self.issued.len());
                self.issued.len()
            }
        };
        format!("ID{index}")
    }

    fn is_known_id(&self, digits: &str) -> bool {
        self.roster.iter().any(|m| m.user_id == digits)
    }

    fn encode(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            if let Some((len, digits)) = mention_at(rest) {
                let tag = self.tag(digits);
                out.push_str(&format!("<@{tag}>"));
                i += len;
                continue;
            }
            if at_word_start(text, i) {
                let digits = digit_run(rest);
                if !digits.is_empty()
                    && (SNOWFLAKE_DIGITS.contains(&digits.len()) || self.is_known_id(digits))
                {
                    let tag = self.tag(digits);
                    out.push_str(&tag);
                    i += digits.len();
                    continue;
                }
                if let Some((len, user_id)) = needle_at(rest, &self.needles) {
                    let user_id = user_id.to_owned();
                    out.push_str(&self.tag(&user_id));
                    i += len;
                    continue;
                }
            }
            let ch = rest.chars().next().expect("non-empty rest");
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }

    fn decode_with(
        &self,
        text: &str,
        render: impl Fn(&str) -> String,
    ) -> Result<String, DecodeError> {
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            if at_word_start(text, i)
                && let Some((len, index)) = tag_at(rest)
            {
                let user_id = index
                    .checked_sub(1)
                    .and_then(|slot| self.issued.get(slot))
                    .ok_or(DecodeError::UnknownToken { offset: i })?;
                out.push_str(&render(user_id));
                i += len;
                continue;
            }
            let ch = rest.chars().next().expect("non-empty rest");
            out.push(ch);
            i += ch.len_utf8();
        }
        Ok(out)
    }
}

impl IdentitySession for TaggingSession {
    fn author_label(&mut self, user_id: &str, _name: &str) -> String {
        self.tag(user_id)
    }

    fn member_ref(&mut self, user_id: &str) -> String {
        self.tag(user_id)
    }

    fn text(&mut self, text: &str) -> String {
        self.encode(text)
    }

    fn tool_result(&mut self, content: &str) -> String {
        self.encode(content)
    }

    fn participant_enum(&self) -> Option<Vec<String>> {
        Some((1..=self.issued.len()).map(|n| format!("ID{n}")).collect())
    }

    fn decode_ref(&self, value: &str) -> Result<String, DecodeError> {
        self.decode_with(value, str::to_owned)
    }

    fn decode_json(&self, json: &str) -> Result<String, DecodeError> {
        self.decode_with(json, str::to_owned)
    }

    fn decode_reply(&self, text: &str) -> Result<String, DecodeError> {
        self.decode_with(text, |user_id| {
            self.roster
                .iter()
                .find(|m| m.user_id == user_id)
                .map_or_else(|| user_id.to_owned(), |m| m.display_name.clone())
        })
    }
}

/// Raw identities present in `haystack`: roster names, nicknames and aliases
/// (whole word, ASCII case-insensitive), roster user ids and any
/// snowflake-length digit run. Empty means the capture is clean.
pub fn find_leaks(haystack: &str, roster: &[Member]) -> Vec<String> {
    let needles = sorted_needles(roster);
    let mut leaks = Vec::new();
    let mut i = 0;
    while i < haystack.len() {
        let rest = &haystack[i..];
        if at_word_start(haystack, i) {
            let digits = digit_run(rest);
            if !digits.is_empty()
                && (SNOWFLAKE_DIGITS.contains(&digits.len())
                    || roster.iter().any(|m| m.user_id == digits))
            {
                leaks.push(digits.to_owned());
                i += digits.len();
                continue;
            }
            if let Some((len, _)) = needle_at(rest, &needles) {
                leaks.push(rest[..len].to_owned());
                i += len;
                continue;
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    leaks
}

/// [`find_leaks`] over every unescaped string (keys and values) of a request,
/// so JSON escapes such as `\n` cannot hide a name from the word-boundary check.
pub fn find_request_leaks(request: &ChatRequest, roster: &[Member]) -> Vec<String> {
    let value = serde_json::to_value(request).expect("chat requests serialize");
    let mut leaks = Vec::new();
    let mut stack = vec![&value];
    while let Some(value) = stack.pop() {
        match value {
            Value::String(text) => leaks.extend(find_leaks(text, roster)),
            Value::Number(number) => leaks.extend(find_leaks(&number.to_string(), roster)),
            Value::Array(items) => stack.extend(items),
            Value::Object(map) => {
                for (key, item) in map {
                    leaks.extend(find_leaks(key, roster));
                    stack.push(item);
                }
            }
            Value::Null | Value::Bool(_) => {}
        }
    }
    leaks
}

fn sorted_needles(roster: &[Member]) -> Vec<(String, String)> {
    let mut needles: Vec<(String, String)> = roster
        .iter()
        .flat_map(|m| {
            std::iter::once(&m.display_name)
                .chain(m.nickname.iter())
                .chain(m.aliases.iter())
                .filter(|name| !name.trim().is_empty())
                .map(|name| (name.clone(), m.user_id.clone()))
        })
        .collect();
    needles.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    needles
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn at_word_start(text: &str, i: usize) -> bool {
    text[..i].chars().next_back().is_none_or(|ch| !is_word(ch))
}

fn at_word_end(rest: &str, len: usize) -> bool {
    rest[len..].chars().next().is_none_or(|ch| !is_word(ch))
}

fn digit_run(rest: &str) -> &str {
    let len = rest.bytes().take_while(u8::is_ascii_digit).count();
    if at_word_end(rest, len) {
        &rest[..len]
    } else {
        ""
    }
}

/// `<@123>` or `<@!123>`: (matched length, digits).
fn mention_at(rest: &str) -> Option<(usize, &str)> {
    let body = rest.strip_prefix("<@")?;
    let (skip, body) = match body.strip_prefix('!') {
        Some(body) => (3, body),
        None => (2, body),
    };
    let len = body.bytes().take_while(u8::is_ascii_digit).count();
    (len > 0 && body[len..].starts_with('>')).then(|| (skip + len + 1, &body[..len]))
}

fn needle_at<'a>(rest: &str, needles: &'a [(String, String)]) -> Option<(usize, &'a str)> {
    needles.iter().find_map(|(needle, user_id)| {
        let len = needle.len();
        (rest.is_char_boundary(len)
            && rest.len() >= len
            && rest[..len].eq_ignore_ascii_case(needle)
            && at_word_end(rest, len))
        .then_some((len, user_id.as_str()))
    })
}

/// `ID<n>` (ASCII case-insensitive) ending at a word boundary.
fn tag_at(rest: &str) -> Option<(usize, usize)> {
    let prefix = rest.get(..2)?;
    if !prefix.eq_ignore_ascii_case("id") {
        return None;
    }
    let digits = rest[2..].bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 || !at_word_end(rest, 2 + digits) {
        return None;
    }
    // Overflowing indexes were never issued, so they decode as unknown.
    let index = rest[2..2 + digits].parse().unwrap_or(usize::MAX);
    Some((2 + digits, index))
}
