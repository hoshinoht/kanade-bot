//! Code-owned text inside a prompt fragment stays literal: only the gaps
//! between protected pieces go through the session, so a member named like a
//! word of the rules (`Will`) never rewrites the rules themselves.

use super::codec::IdentitySession;

/// A code-owned piece of a prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Protected {
    /// Copied verbatim wherever it occurs.
    Exact(String),
    /// From `start` through the next `end` (both included): a code-rendered
    /// line with variable parts (dates, a model alias).
    Span { start: String, end: String },
}

/// `text` with every protected piece copied verbatim and the rest encoded
/// with [`IdentitySession::text`]. Passthrough output equals `text`.
pub fn encode_protected(
    session: &mut dyn IdentitySession,
    text: &str,
    protected: &[Protected],
) -> String {
    let ranges = ranges(text, protected);
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (start, end) in ranges {
        if start > at {
            out.push_str(&session.text(&text[at..start]));
        }
        out.push_str(&text[start..end]);
        at = end;
    }
    if at < text.len() {
        out.push_str(&session.text(&text[at..]));
    }
    out
}

/// Non-overlapping protected byte ranges, in order; at each position the
/// longest match wins.
fn ranges(text: &str, protected: &[Protected]) -> Vec<(usize, usize)> {
    let mut found: Vec<(usize, usize)> = Vec::new();
    for piece in protected {
        match piece {
            Protected::Exact(exact) if !exact.is_empty() => {
                found.extend(
                    text.match_indices(exact.as_str())
                        .map(|(at, hit)| (at, at + hit.len())),
                );
            }
            Protected::Span { start, end } if !start.is_empty() && !end.is_empty() => {
                let mut from = 0;
                while let Some(offset) = text[from..].find(start.as_str()) {
                    let begin = from + offset;
                    let Some(close) = text[begin + start.len()..].find(end.as_str()) else {
                        break;
                    };
                    let stop = begin + start.len() + close + end.len();
                    found.push((begin, stop));
                    from = stop;
                }
            }
            _ => {}
        }
    }
    found.sort_by_key(|&(start, end)| (start, std::cmp::Reverse(end)));
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in found {
        match merged.last_mut() {
            Some(last) if start < last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::llm::identity::PassthroughSession;

    struct Upper;

    impl IdentitySession for Upper {
        fn author_label(&mut self, _: &str, name: &str) -> String {
            name.into()
        }
        fn member_ref(&mut self, id: &str) -> String {
            id.into()
        }
        fn text(&mut self, text: &str) -> String {
            text.to_uppercase()
        }
        fn tool_result(&mut self, content: &str) -> String {
            content.into()
        }
        fn participant_enum(&self) -> Option<Vec<String>> {
            None
        }
        fn decode_ref(&self, value: &str) -> Result<String, super::super::DecodeError> {
            Ok(value.into())
        }
        fn decode_json(&self, json: &str) -> Result<String, super::super::DecodeError> {
            Ok(json.into())
        }
        fn decode_reply(&self, text: &str) -> Result<String, super::super::DecodeError> {
            Ok(text.into())
        }
    }

    #[test]
    fn only_gaps_are_encoded() {
        let protected = [
            Protected::Exact("you will".into()),
            Protected::Span {
                start: "Now: ".into(),
                end: ".".into(),
            },
        ];
        let text = "hi you will see. Now: May 1. bye";
        assert_eq!(
            encode_protected(&mut Upper, text, &protected),
            "HI you will SEE. Now: May 1. BYE"
        );
        assert_eq!(
            encode_protected(&mut PassthroughSession, text, &protected),
            text
        );
        assert_eq!(encode_protected(&mut Upper, "abc", &[]), "ABC");
    }
}
