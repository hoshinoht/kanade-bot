use super::{
    issuer::Lookup,
    matcher::{
        at_word_start, digit_run, escape_len, match_at, mention_at, prev_char, url_len, word_run,
    },
    session::PseudonymSession,
};

impl PseudonymSession {
    /// Member-sourced text → tokens. Order at each position: mention, URL
    /// (copied except known ids), longest needle, known id digit run, a word
    /// that spells an issued token (shadowed); anything else is copied.
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
            if at_word_start(prev, rest)
                && let Some(len) = url_len(rest)
            {
                let url = self.encode_url(&rest[..len]);
                out.push_str(&url);
                i += len;
                continue;
            }
            if let Some((len, user_id)) = self.needle_at(prev, rest) {
                out.push_str(&self.token(&user_id));
                i += len;
                continue;
            }
            if prev.is_none_or(|ch| !ch.is_ascii_digit()) {
                let digits = digit_run(rest);
                if !digits.is_empty() {
                    let masked = self.id_token(digits);
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

    /// URLs keep their text except digit runs that are known ids.
    fn encode_url(&mut self, url: &str) -> String {
        let mut out = String::with_capacity(url.len());
        let mut i = 0;
        while i < url.len() {
            let rest = &url[i..];
            let digits = digit_run(rest);
            if !digits.is_empty() {
                let masked = self.id_token(digits);
                out.push_str(masked.as_deref().unwrap_or(digits));
                i += digits.len();
                continue;
            }
            let ch = rest.chars().next().expect("non-empty rest");
            out.push(ch);
            i += ch.len_utf8();
        }
        out
    }

    /// Pool forms spelled in source text are never issued in this session.
    pub(super) fn note_literals(&mut self, text: &str) {
        let mut i = 0;
        while i < text.len() {
            let rest = &text[i..];
            if let Some(len) = escape_len(text, i) {
                // Either reading of `\tomoe` may be a pool form.
                self.issuer.note_source(word_run(&text[i + 1..]));
                i += len;
                continue;
            }
            let word = word_run(rest);
            if !word.is_empty() && at_word_start(prev_char(text, i), rest) {
                self.issuer.note_source(word);
                i += word.len();
            } else {
                i += rest.chars().next().map_or(1, char::len_utf8);
            }
        }
    }

    fn needle_at(&self, prev: Option<char>, rest: &str) -> Option<(usize, String)> {
        let first = rest.chars().next()?.to_lowercase().next()?;
        self.needles.iter().find_map(|needle| {
            if needle.folded.first() != Some(&first) {
                return None;
            }
            match_at(prev, rest, &needle.folded).map(|len| (len, needle.user_id.clone()))
        })
    }
}
