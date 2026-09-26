use super::{
    issuer::{Holder, Lookup},
    matcher::{at_word_start, escape_len, prev_char, token_mention_at, word_run},
    session::PseudonymSession,
};
use crate::infrastructure::llm::identity::codec::DecodeError;

/// Shown for a member ref with no known name; never an id or a ping.
const NAMELESS: &str = "someone";

impl PseudonymSession {
    /// Model output → identities: tokens become user ids (`reply` false) or
    /// the member's name (`reply` true; `<@Token>` loses its brackets). A pool
    /// form this session neither issued nor saw in source is an unknown token.
    pub(super) fn decode(&self, text: &str, reply: bool) -> Result<String, DecodeError> {
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            let prev = prev_char(text, i);
            if let Some(len) = escape_len(text, i) {
                out.push_str(&rest[..len]);
                i += len;
                continue;
            }
            if reply
                && let Some((len, at, word)) = token_mention_at(rest)
                && let Lookup::Token { index, core } = self.lookup_at(word, i + at)?
                && core == word.len()
            {
                out.push_str(self.render(index, true));
                i += len;
                continue;
            }
            if at_word_start(prev, rest) {
                let word = word_run(rest);
                if let Some(digits) = self.opaque_digits(word) {
                    out.push_str(digits);
                    i += word.len();
                    continue;
                }
                if !word.is_empty() {
                    match self.lookup_at(word, i)? {
                        Lookup::Token { index, core } => {
                            out.push_str(self.render(index, reply));
                            out.push_str(&word[core..]);
                        }
                        _ => out.push_str(word),
                    }
                    i += word.len();
                    continue;
                }
            }
            let ch = rest.chars().next().expect("non-empty rest");
            out.push(ch);
            i += ch.len_utf8();
        }
        Ok(out)
    }

    fn lookup_at(&self, word: &str, offset: usize) -> Result<Lookup, DecodeError> {
        match self.issuer.lookup(word) {
            Lookup::Unknown { .. } => Err(DecodeError::UnknownToken { offset }),
            found => Ok(found),
        }
    }

    fn render(&self, index: usize, reply: bool) -> &str {
        match &self.issuer.issued[index].holder {
            Holder::Literal(text) => text,
            Holder::Member(user_id) if reply => {
                self.names.get(user_id).map_or(NAMELESS, String::as_str)
            }
            Holder::Member(user_id) => user_id,
        }
    }
}
