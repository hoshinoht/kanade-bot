use super::{
    issuer::Lookup,
    matcher::{
        at_word_start, channel_or_role_at, digit_run, escape_len, link_token_at, match_at,
        mention_at, prev_char, url_len, word_run,
    },
    session::{Owner, PseudonymSession, is_snowflake},
};

impl PseudonymSession {
    /// Member-sourced text → tokens. Complete URLs become link tokens before
    /// name/id matching, including when the scheme follows word characters.
    pub(super) fn encode(&mut self, text: &str) -> String {
        self.note_literals(text);
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            // After an escape's backslash both its letter (`\bobby`) and the
            // position after the escape (`\nBob`) may start a match.
            let prev = prev_char(text, i);
            if let Some((len, digits)) = mention_at(rest) {
                let rendered = if self.is_bot(digits) {
                    format!("@{}", self.bot_name())
                } else {
                    format!("<@{}>", self.token(digits))
                };
                out.push_str(&rendered);
                i += len;
                continue;
            }
            if let Some((len, digits, role)) = channel_or_role_at(rest) {
                let rendered = self.channel_or_role(digits, role);
                out.push_str(&rendered);
                i += len;
                continue;
            }
            if let Some((len, token)) = link_token_at(rest) {
                let replacement = if self.link_target(token).is_some() {
                    self.shadow_link_literal(token)
                } else {
                    token.to_owned()
                };
                out.push_str(&replacement);
                i += len;
                continue;
            }
            if let Some(len) = url_len(rest) {
                let url = self.encode_url(&rest[..len]);
                out.push_str(&url);
                i += len;
                continue;
            }
            if let Some((len, owner, word)) = self.needle_at(prev, rest) {
                let token = match owner {
                    Owner::Member(user_id) => self.token(&user_id),
                    Owner::Shared => self.issuer.shared(&word).to_owned(),
                };
                out.push_str(&token);
                i += len;
                continue;
            }
            if prev.is_none_or(|ch| !ch.is_ascii_digit()) {
                let digits = digit_run(rest);
                if !digits.is_empty() {
                    let masked = self.digits_token(digits);
                    out.push_str(masked.as_deref().unwrap_or(digits));
                    i += digits.len();
                    continue;
                }
            }
            if at_word_start(prev, rest) {
                let word = word_run(rest);
                if let Lookup::Token { core, .. } = self.issuer.lookup(word) {
                    let shadow = self.issuer.shadow(&word[..core]).to_owned();
                    out.push_str(&shadow);
                    out.push_str(&word[core..]);
                    i += word.len();
                    continue;
                }
            }
            let ch = rest.chars().next().expect("non-empty rest");
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }

    /// The token (or bot name) for a roster, issued or bot id.
    pub(super) fn id_token(&mut self, digits: &str) -> Option<String> {
        (self.is_bot(digits) || self.is_known_id(digits)).then(|| self.token(digits))
    }

    /// A known id's token, else an opaque `Ref<n>` for a stray snowflake.
    pub(super) fn digits_token(&mut self, digits: &str) -> Option<String> {
        self.id_token(digits)
            .or_else(|| is_snowflake(digits).then(|| self.opaque_token(digits)))
    }

    /// `#name` / `@name` (the name encoded as text) when the guild knows the
    /// channel or role, else the mention around an opaque ref.
    fn channel_or_role(&mut self, digits: &str, role: bool) -> String {
        let named = self.shared.mentions.as_ref().and_then(|names| {
            if role {
                names.role(digits)
            } else {
                names.channel(digits)
            }
        });
        match named {
            Some(name) => {
                let name = self.encode(&name);
                format!("{}{name}", if role { '@' } else { '#' })
            }
            None => {
                let token = self
                    .digits_token(digits)
                    .unwrap_or_else(|| digits.to_owned());
                format!("{}{token}>", if role { "<@&" } else { "<#" })
            }
        }
    }

    /// A whole URL is opaque to the model and can only be restored locally.
    fn encode_url(&mut self, url: &str) -> String {
        self.link_token(url)
    }

    /// Pool forms spelled in source text are never issued in this session.
    pub(super) fn note_literals(&mut self, text: &str) {
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            if let Some((len, token)) = link_token_at(rest) {
                self.source_link_tokens.insert(token.to_owned());
                i += len;
                continue;
            }
            if let Some(len) = escape_len(text, i) {
                // Either reading of `\tomoe` may be a pool form.
                self.issuer.note_source(word_run(&text[i + 1..]));
                i += len;
                continue;
            }
            let word = word_run(rest);
            if !word.is_empty() && at_word_start(prev_char(text, i), rest) {
                if Self::is_opaque_form(word) {
                    self.source_refs.insert(word.to_ascii_lowercase());
                }
                self.issuer.note_source(word);
                i += word.len();
            } else {
                i += rest.chars().next().map_or(1, char::len_utf8);
            }
        }
    }

    fn needle_at(&self, prev: Option<char>, rest: &str) -> Option<(usize, Owner, String)> {
        let first = rest.chars().next()?.to_lowercase().next()?;
        self.needles.iter().find_map(|needle| {
            if needle.folded.first() != Some(&first) {
                return None;
            }
            match_at(prev, rest, &needle.folded)
                .map(|len| (len, needle.owner.clone(), needle.text.clone()))
        })
    }
}
