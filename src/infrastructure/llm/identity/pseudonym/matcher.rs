//! Offset-safe text scanning: word boundaries, Unicode case-insensitive needle
//! matching, mentions, URLs and digit runs. All offsets are char boundaries.

/// Letters and digits. `_` and `-` separate words, so a name inside a
/// channel or code-ish name (`hstar-jonas_lau`) is still a whole word.
pub(in crate::infrastructure::llm::identity) fn is_word(ch: char) -> bool {
    ch.is_alphanumeric()
}

/// Scripts written without spaces (kana, CJK ideographs, Thai), where a name
/// may sit directly next to other words.
fn unspaced(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0E00..=0x0E7F
            | 0x3040..=0x30FF
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0xFF66..=0xFF9F
            | 0x20000..=0x3134F
    )
}

/// Two adjacent chars belong to one word, so a match may not split them.
fn joins(a: char, b: char) -> bool {
    is_word(a) && is_word(b) && !unspaced(a) && !unspaced(b)
}

/// The char before `at`, or `None` at the start or right after a possible
/// backslash escape (`\n`, `\t`, `\r`, `\b`, `\f`, `\uXXXX`), which counts
/// as a word boundary so escaped text cannot hide a name. Whether the
/// backslash is itself escaped is ignored: masking under both readings is the
/// safe side, and decoding reads it the same way.
pub(in crate::infrastructure::llm::identity) fn prev_char(text: &str, at: usize) -> Option<char> {
    let head = &text[..at];
    if after_escape(head.as_bytes()) {
        return None;
    }
    head.chars().next_back()
}

/// Length of the possible backslash escape starting at `at`. Decoding and
/// literal-noting copy it whole; encoding does not, so the letter after a
/// stray backslash (`\bobby`) can still start a name.
pub(super) fn escape_len(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(at) != Some(&b'\\') {
        return None;
    }
    match bytes.get(at + 1)? {
        b'n' | b't' | b'r' | b'b' | b'f' => Some(2),
        b'u' if bytes.get(at + 2..at + 6)?.iter().all(u8::is_ascii_hexdigit) => Some(6),
        _ => None,
    }
}

fn after_escape(head: &[u8]) -> bool {
    let n = head.len();
    (n >= 2 && head[n - 2] == b'\\' && matches!(head[n - 1], b'n' | b't' | b'r' | b'b' | b'f'))
        || (n >= 6
            && head[n - 6] == b'\\'
            && head[n - 5] == b'u'
            && head[n - 4..].iter().all(u8::is_ascii_hexdigit))
}

/// A word starts at `rest` (a match may begin here).
pub(super) fn at_word_start(prev: Option<char>, rest: &str) -> bool {
    match (prev, rest.chars().next()) {
        (Some(p), Some(first)) => !joins(p, first),
        _ => true,
    }
}

/// Lowercased chars of a needle, compared char by char against text.
pub(in crate::infrastructure::llm::identity) fn fold(text: &str) -> Vec<char> {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// Byte length of `rest`'s prefix that equals `folded` case-insensitively
/// and does not split a word on either side.
pub(in crate::infrastructure::llm::identity) fn match_at(
    prev: Option<char>,
    rest: &str,
    folded: &[char],
) -> Option<usize> {
    let mut matched = 0;
    let mut end = None;
    for (at, ch) in rest.char_indices() {
        for lower in ch.to_lowercase() {
            if folded.get(matched) != Some(&lower) {
                return None;
            }
            matched += 1;
        }
        if matched == folded.len() {
            end = Some(at + ch.len_utf8());
            break;
        }
    }
    let end = end?;
    let first = rest.chars().next()?;
    let last = rest[..end].chars().next_back()?;
    let split_before = prev.is_some_and(|p| joins(p, first));
    let split_after = rest[end..].chars().next().is_some_and(|n| joins(last, n));
    (!split_before && !split_after).then_some(end)
}

/// Maximal run of word chars that [`joins`] links, so `Midoriさん` splits into
/// `Midori` and `さ`, `ん`.
pub(super) fn word_run(rest: &str) -> &str {
    let mut chars = rest.char_indices();
    let Some((_, mut prev)) = chars.next().filter(|&(_, ch)| is_word(ch)) else {
        return "";
    };
    let len = chars
        .find(|&(_, ch)| {
            let split = !joins(prev, ch);
            prev = ch;
            split
        })
        .map_or(rest.len(), |(at, _)| at);
    &rest[..len]
}

pub(in crate::infrastructure::llm::identity) fn digit_run(rest: &str) -> &str {
    let len = rest.bytes().take_while(u8::is_ascii_digit).count();
    &rest[..len]
}

/// `<@123>` or `<@!123>`: (matched length, digits).
pub(super) fn mention_at(rest: &str) -> Option<(usize, &str)> {
    let (skip, body) = mention_body(rest)?;
    let digits = digit_run(body);
    (!digits.is_empty() && body[digits.len()..].starts_with('>'))
        .then(|| (skip + digits.len() + 1, digits))
}

/// `<#123>` (channel) or `<@&123>` (role): (matched length, digits, is_role).
pub(super) fn channel_or_role_at(rest: &str) -> Option<(usize, &str, bool)> {
    let (skip, body, role) = if let Some(body) = rest.strip_prefix("<#") {
        (2, body, false)
    } else {
        (3, rest.strip_prefix("<@&")?, true)
    };
    let digits = digit_run(body);
    (!digits.is_empty() && body[digits.len()..].starts_with('>'))
        .then(|| (skip + digits.len() + 1, digits, role))
}

/// `<@Word>` or `<@!Word>`: (matched length, offset of the word, word).
pub(super) fn token_mention_at(rest: &str) -> Option<(usize, usize, &str)> {
    let (skip, body) = mention_body(rest)?;
    let word = word_run(body);
    (!word.is_empty() && body[word.len()..].starts_with('>'))
        .then(|| (skip + word.len() + 1, skip, word))
}

fn mention_body(rest: &str) -> Option<(usize, &str)> {
    let body = rest.strip_prefix("<@")?;
    Some(match body.strip_prefix('!') {
        Some(body) => (3, body),
        None => (2, body),
    })
}

/// An `http://` or `https://` URL runs to whitespace, `<`, `>`, `"`, a
/// backtick or a backslash.
pub(super) fn url_len(rest: &str) -> Option<usize> {
    let scheme = ["https://", "http://"].into_iter().find(|scheme| {
        rest.get(..scheme.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(scheme))
    })?;
    let len = rest
        .char_indices()
        .skip(scheme.len())
        .find(|&(_, ch)| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '`' | '\\'))
        .map_or(rest.len(), |(at, _)| at);
    Some(len)
}
