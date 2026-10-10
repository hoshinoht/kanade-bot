//! Python `str` semantics that v4 messages and parsers depend on.

use std::fmt::Write;

/// `str.isspace()`: Unicode `White_Space` plus the ASCII separators U+001C..U+001F.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()` with no arguments.
pub(crate) fn strip(value: &str) -> &str {
    value.trim_matches(is_space)
}

/// `str.split()` with no arguments.
pub(crate) fn split_whitespace(value: &str) -> impl Iterator<Item = &str> {
    value.split(is_space).filter(|word| !word.is_empty())
}

/// First code point of every Unicode 15.0 `Nd` run (Python 3.12 `unicodedata`);
/// each run holds the digits 0-9 in order.
const DECIMAL_ZEROS: [u32; 68] = [
    0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
    0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
    0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0, 0xff10,
    0x104a0, 0x10d30, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450, 0x114d0, 0x11650,
    0x116c0, 0x11730, 0x118e0, 0x11950, 0x11c50, 0x11d50, 0x11da0, 0x11f50, 0x16a60, 0x16ac0,
    0x16b50, 0x1d7ce, 0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e950,
    0x1fbf0,
];

/// Value of a decimal digit as Python's regex `\d` and `int()` read it, so
/// fullwidth `９` counts like `9`.
pub(crate) fn decimal(c: char) -> Option<u32> {
    let code = u32::from(c);
    let run = DECIMAL_ZEROS
        .partition_point(|&zero| zero <= code)
        .checked_sub(1)?;
    let value = code - DECIMAL_ZEROS[run];
    (value < 10).then_some(value)
}

/// Unicode 15.0 digits that are not decimal (`Numeric_Type=Digit`, e.g. `²`, `①`,
/// Ethiopic `፩`), as inclusive ranges from Python 3.12 `unicodedata`.
const DIGIT_ONLY: [(u32, u32); 20] = [
    (0xb2, 0xb3),
    (0xb9, 0xb9),
    (0x1369, 0x1371),
    (0x19da, 0x19da),
    (0x2070, 0x2070),
    (0x2074, 0x2079),
    (0x2080, 0x2089),
    (0x2460, 0x2468),
    (0x2474, 0x247c),
    (0x2488, 0x2490),
    (0x24ea, 0x24ea),
    (0x24f5, 0x24fd),
    (0x24ff, 0x24ff),
    (0x2776, 0x277e),
    (0x2780, 0x2788),
    (0x278a, 0x2792),
    (0x10a40, 0x10a43),
    (0x10e60, 0x10e68),
    (0x11052, 0x1105a),
    (0x1f100, 0x1f10a),
];

/// `str.isdigit()` for one character: decimal digits plus [`DIGIT_ONLY`].
pub(crate) fn is_digit(c: char) -> bool {
    let code = u32::from(c);
    decimal(c).is_some()
        || DIGIT_ONLY
            .iter()
            .any(|&(low, high)| (low..=high).contains(&code))
}

/// `repr(str)`, including Python's quote choice and escapes.
///
/// Non-ASCII printability is approximated with the separator, format and
/// private-use ranges v4 users could plausibly paste; unassigned code points
/// print verbatim where Python would escape them.
pub(crate) fn repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(value.len() + 2);
    out.push(quote);
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_printable(c) => out.push(c),
            c => {
                let code = u32::from(c);
                // Writing to a String cannot fail.
                let _ = match code {
                    0..=0xff => write!(out, "\\x{code:02x}"),
                    0x100..=0xffff => write!(out, "\\u{code:04x}"),
                    _ => write!(out, "\\U{code:08x}"),
                };
            }
        }
    }
    out.push(quote);
    out
}

/// `repr(list[str])`.
pub(crate) fn list_repr<S: AsRef<str>>(items: &[S]) -> String {
    let inner: Vec<String> = items.iter().map(|item| repr(item.as_ref())).collect();
    format!("[{}]", inner.join(", "))
}

fn is_printable(c: char) -> bool {
    if c == ' ' {
        return true;
    }
    if c.is_control() || is_space(c) {
        return false;
    }
    !matches!(
        u32::from(c),
        0xad | 0x600..=0x605
            | 0x61c
            | 0x6dd
            | 0x70f
            | 0x180e
            | 0x200b..=0x200f
            | 0x202a..=0x202e
            | 0x2060..=0x2064
            | 0x2066..=0x206f
            | 0xe000..=0xf8ff
            | 0xfeff
            | 0xfff9..=0xfffb
            | 0xf0000..
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_matches_python_quoting() {
        assert_eq!(repr("caturday"), "'caturday'");
        assert_eq!(repr("it's"), "\"it's\"");
        assert_eq!(repr("'\""), "'\\'\"'");
        assert_eq!(
            repr("a\\b\n\u{7}\u{a0}\u{200b}é"),
            "'a\\\\b\\n\\x07\\xa0\\u200bé'"
        );
        assert_eq!(list_repr(&["q", "z"]), "['q', 'z']");
    }

    #[test]
    fn decimal_digits_follow_unicode_nd() {
        assert_eq!(decimal('7'), Some(7));
        assert_eq!(decimal('９'), Some(9));
        assert_eq!(decimal('٣'), Some(3));
        assert_eq!(decimal('\u{1fbf9}'), Some(9));
        assert_eq!(decimal('/'), None);
        assert_eq!(decimal(':'), None);
        assert_eq!(decimal('²'), None);
        for c in ['7', '٣', '²', '①', '፩', '\u{1f10a}'] {
            assert!(is_digit(c), "{c}");
        }
        for c in ['a', '½', 'Ⅻ', '\u{1f10b}'] {
            assert!(!is_digit(c), "{c}");
        }
    }

    #[test]
    fn whitespace_includes_python_separators() {
        assert_eq!(strip("\u{1f} x\u{3000}"), "x");
        assert_eq!(
            split_whitespace(" a\u{a0}b  c ").collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
    }
}
