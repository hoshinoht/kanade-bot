//! Tool results that are JSON objects or arrays are encoded structurally:
//! every string literal (keys included) is unescaped, encoded and re-escaped,
//! so escapes cannot hide a name; bytes outside changed strings are kept.

use serde::de::IgnoredAny;

use super::{matcher::digit_run, session::PseudonymSession};

impl PseudonymSession {
    pub(super) fn encode_tool_result(&mut self, content: &str) -> String {
        let head = content.trim_start();
        let container = head.starts_with('{') || head.starts_with('[');
        if container && serde_json::from_str::<IgnoredAny>(content).is_ok() {
            self.encode_json(content)
        } else {
            self.encode(content)
        }
    }

    /// `json` must be valid JSON.
    fn encode_json(&mut self, json: &str) -> String {
        for (start, end) in string_spans(json) {
            let text = unescape(&json[start..end]);
            self.note_literals(text.as_deref().unwrap_or(&json[start..end]));
        }
        let mut out = String::with_capacity(json.len());
        let mut at = 0;
        for (start, end) in string_spans(json) {
            self.encode_scalars(&json[at..start], &mut out);
            let literal = &json[start..end];
            // Only reached if even the sanitised literal will not unescape.
            let Some(text) = unescape(literal) else {
                let encoded = self.encode(literal);
                out.push_str(&encoded);
                at = end;
                continue;
            };
            let encoded = self.encode(&text);
            if encoded == text {
                out.push_str(literal);
            } else {
                out.push_str(&quote(&encoded));
            }
            at = end;
        }
        self.encode_scalars(&json[at..], &mut out);
        out
    }

    /// Outside strings only numbers can carry an id: a whole integer equal to
    /// a known id becomes its token as a JSON string.
    fn encode_scalars(&mut self, segment: &str, out: &mut String) {
        let bytes = segment.as_bytes();
        let mut i = 0;
        while i < segment.len() {
            let digits = digit_run(&segment[i..]);
            if digits.is_empty() {
                out.push(bytes[i] as char);
                i += 1;
                continue;
            }
            let end = i + digits.len();
            let whole = (i == 0 || !matches!(bytes[i - 1], b'-' | b'.' | b'e' | b'E' | b'+'))
                && !matches!(bytes.get(end), Some(b'.' | b'e' | b'E'));
            match whole.then(|| self.digits_token(digits)).flatten() {
                Some(token) => out.push_str(&quote(&token)),
                None => out.push_str(digits),
            }
            i = end;
        }
    }
}

/// Byte spans (quotes included) of every string literal in valid JSON.
fn string_spans(json: &str) -> Vec<(usize, usize)> {
    let bytes = json.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && bytes[i] != b'"' {
            i += if bytes[i] == b'\\' { 2 } else { 1 };
        }
        i = (i + 1).min(bytes.len());
        spans.push((start, i));
    }
    spans
}

/// A literal's text; lone surrogate escapes become U+FFFD, which only reaches
/// the output if the string had to be rewritten anyway.
fn unescape(literal: &str) -> Option<String> {
    serde_json::from_str(literal)
        .ok()
        .or_else(|| serde_json::from_str(&without_lone_surrogates(literal)).ok())
}

fn without_lone_surrogates(literal: &str) -> String {
    let bytes = literal.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        match surrogate_at(bytes, i) {
            Some(0xD800..=0xDBFF)
                if matches!(surrogate_at(bytes, i + 6), Some(0xDC00..=0xDFFF)) =>
            {
                out.extend_from_slice(&bytes[i..i + 12]);
                i += 12;
            }
            Some(_) => {
                out.extend_from_slice(br"\ufffd");
                i += 6;
            }
            None => {
                let len = 2.min(bytes.len() - i);
                out.extend_from_slice(&bytes[i..i + len]);
                i += len;
            }
        }
    }
    String::from_utf8(out).expect("only whole UTF-8 sequences and ASCII were copied")
}

/// The code unit of a `\uXXXX` escape at `at` when it is a surrogate.
fn surrogate_at(bytes: &[u8], at: usize) -> Option<u16> {
    let hex = bytes.get(at..at + 6)?.strip_prefix(br"\u")?;
    let code = u16::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
    (0xD800..=0xDFFF).contains(&code).then_some(code)
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).expect("strings serialize")
}
