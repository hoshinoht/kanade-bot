use super::{
    issuer::{Holder, Lookup},
    matcher::{at_word_start, escape_len, link_token_at, prev_char, token_mention_at, word_run},
    session::PseudonymSession,
};
use crate::infrastructure::llm::identity::codec::DecodeError;

/// Shown for a member ref with no known name; never an id or a ping.
const NAMELESS: &str = "someone";

impl PseudonymSession {
    pub(super) fn decode_links(&self, text: &str, offset: usize) -> Result<String, DecodeError> {
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            if let Some((len, token)) = link_token_at(rest) {
                match self.link_target(token) {
                    Some(
                        super::session::LinkTarget::Url(url)
                        | super::session::LinkTarget::Literal(url),
                    ) => out.push_str(url),
                    None if self.source_link_tokens.contains(token) => out.push_str(token),
                    None => return Err(DecodeError::UnknownToken { offset: offset + i }),
                }
                i += len;
                continue;
            }
            let ch = rest.chars().next().expect("non-empty rest");
            out.push(ch);
            i += ch.len_utf8();
        }
        Ok(out)
    }

    /// Model output → identities: tokens become user ids (`reply` false) or
    /// the member's name (`reply` true; `<@Token>` loses its brackets). A pool
    /// form this session neither issued nor saw in source is an unknown token.
    pub(super) fn decode(
        &self,
        text: &str,
        reply: bool,
        restore_links: bool,
    ) -> Result<String, DecodeError> {
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            let prev = prev_char(text, i);
            if restore_links && let Some((len, token)) = link_token_at(rest) {
                match self.link_target(token) {
                    Some(
                        super::session::LinkTarget::Url(url)
                        | super::session::LinkTarget::Literal(url),
                    ) => out.push_str(url),
                    None if self.source_link_tokens.contains(token) => out.push_str(token),
                    None => return Err(DecodeError::UnknownToken { offset: i }),
                }
                i += len;
                continue;
            }
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
                out.push_str(self.render(index, true, i + at)?);
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
                            out.push_str(self.render(index, reply, i)?);
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

    /// A shared name word is no member: refused as a ref, shown as the word.
    fn render(&self, index: usize, reply: bool, offset: usize) -> Result<&str, DecodeError> {
        Ok(match &self.issuer.issued[index].holder {
            Holder::Literal(text) => text,
            Holder::Shared(word) if reply => word,
            Holder::Shared(_) => return Err(DecodeError::UnknownToken { offset }),
            Holder::Member(user_id) if reply => {
                self.names.get(user_id).map_or(NAMELESS, String::as_str)
            }
            Holder::Member(user_id) => user_id,
        })
    }
}
