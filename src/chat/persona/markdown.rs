//! v4 Markdown conventions reproduced exactly: outer stripping, line splitting,
//! the `# Persona:` name and `Good` example extraction. Hand-written matchers
//! mirror the v4 regular expressions; the persona oracle vectors pin them.

/// Python `str.isspace`: Unicode `White_Space` plus the ASCII separators U+001C..U+001F.
pub(crate) fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// Python `str.strip()`.
pub(crate) fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

fn strip_end(text: &str) -> &str {
    text.trim_end_matches(is_space)
}

fn strip_start(text: &str) -> &str {
    text.trim_start_matches(is_space)
}

/// Python `str.splitlines()` boundaries (`\r\n` counts once).
pub(crate) fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let breaks = matches!(
            c,
            '\n' | '\r'
                | '\u{0b}'
                | '\u{0c}'
                | '\u{1c}'
                | '\u{1d}'
                | '\u{1e}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if !breaks {
            continue;
        }
        lines.push(&text[start..index]);
        let mut next = index + c.len_utf8();
        if c == '\r' && matches!(chars.peek(), Some((_, '\n'))) {
            chars.next();
            next += 1;
        }
        start = next;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// An unfilled template slot such as `<A short reply>` is not content.
pub(crate) fn is_placeholder(text: &str) -> bool {
    text.starts_with('<')
}

fn strip_prefix_ignore_ascii_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// Up to `max` leading `*`/`_` characters.
fn skip_emphasis(text: &str, max: usize) -> &str {
    let count = text
        .chars()
        .take(max)
        .take_while(|c| matches!(c, '*' | '_'))
        .count();
    &text[count..]
}

/// `^\s*#\s+Persona\s*:\s*(.+?)\s*$` (case-insensitive), stripped.
fn persona_heading(line: &str) -> Option<&str> {
    let rest = strip_start(line).strip_prefix('#')?;
    let after = strip_start(rest);
    if after.len() == rest.len() {
        return None;
    }
    let rest = strip_prefix_ignore_ascii_case(after, "persona")?;
    let rest = strip_start(rest).strip_prefix(':')?;
    Some(strip(rest))
}

/// The name declared by `# Persona: ...`, or `"The assistant"`.
pub(crate) fn identity_name(identity: &str) -> &str {
    split_lines(identity)
        .into_iter()
        .filter_map(persona_heading)
        .find(|name| !name.is_empty() && !is_placeholder(name))
        .unwrap_or("The assistant")
}

/// `^\s*[*_]{0,2}\s*good\b[^*_]*[*_]{0,2}\s*:?\s*$` (case-insensitive).
fn is_good_heading(line: &str) -> bool {
    let rest = strip_start(skip_emphasis(strip_start(line), 2));
    let Some(tail) = strip_prefix_ignore_ascii_case(rest, "good") else {
        return false;
    };
    if tail.chars().next().is_some_and(is_word) {
        return false;
    }
    let Some(marker) = tail.find(['*', '_']) else {
        return true;
    };
    let after = skip_emphasis(&tail[marker..], 2);
    let after = strip_start(after);
    let after = after.strip_prefix(':').unwrap_or(after);
    strip_start(after).is_empty()
}

/// `^\s*>\s*`(.+)`\s*$`, returning the quoted text.
fn example_quote(line: &str) -> Option<&str> {
    let rest = strip_start(strip_start(line).strip_prefix('>')?).strip_prefix('`')?;
    let inner = strip_end(rest).strip_suffix('`')?;
    (!inner.is_empty()).then_some(inner)
}

fn only_run_of(text: &str, marker: char, min: usize) -> bool {
    let run = text.chars().take_while(|&c| c == marker).count();
    run >= min && strip(&text[run..]).is_empty()
}

/// `^\s*(?:#{1,6}\s|-{3,}\s*$|\*{3,}\s*$)|^\s*\*\*[^*]+\*\*\s*$`.
fn is_section_end(line: &str) -> bool {
    let text = strip_start(line);
    let hashes = text.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && text[hashes..].chars().next().is_some_and(is_space) {
        return true;
    }
    if only_run_of(text, '-', 3) || only_run_of(text, '*', 3) {
        return true;
    }
    strip_end(text)
        .strip_prefix("**")
        .and_then(|inner| inner.strip_suffix("**"))
        .is_some_and(|inner| !inner.is_empty() && !inner.contains('*'))
}

/// Quoted lines under each `Good` heading, in file order; empty sections dropped.
fn good_sections(text: &str) -> Vec<Vec<&str>> {
    let mut sections: Vec<Vec<&str>> = Vec::new();
    let mut open = false;
    let mut fenced = false;
    for line in split_lines(text) {
        if strip(line).starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if is_good_heading(line) {
            sections.push(Vec::new());
            open = true;
            continue;
        }
        if !open {
            continue;
        }
        if is_section_end(line) {
            open = false;
            continue;
        }
        let Some(example) = example_quote(line).map(strip) else {
            continue;
        };
        if !example.is_empty() && !is_placeholder(example) {
            sections
                .last_mut()
                .expect("an open section exists")
                .push(example);
        }
    }
    sections.retain(|section| !section.is_empty());
    sections
}

pub(crate) const MAX_EXAMPLES: usize = 8;
pub(crate) const MAX_EXAMPLE_CHARS: usize = 600;

/// Round-robin `Good` examples within the shared count and character budget.
pub(crate) fn good_examples(text: &str) -> Vec<String> {
    let sections = good_sections(text);
    let depth = sections.iter().map(Vec::len).max().unwrap_or(0);
    let mut kept = Vec::new();
    let mut spent = 0;
    for row in 0..depth {
        for example in sections.iter().filter_map(|section| section.get(row)) {
            let size = example.chars().count();
            if kept.len() >= MAX_EXAMPLES || spent + size > MAX_EXAMPLE_CHARS {
                return kept;
            }
            kept.push((*example).to_owned());
            spent += size;
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_lines_matches_python() {
        assert_eq!(
            split_lines("a\r\nb\rc\n\nd\u{2028}e\n"),
            ["a", "b", "c", "", "d", "e"]
        );
        assert_eq!(split_lines(""), Vec::<&str>::new());
        assert_eq!(split_lines("\n"), [""]);
    }

    #[test]
    fn strip_includes_python_separators() {
        assert_eq!(strip("\u{1f} x \u{a0}\n"), "x");
    }

    #[test]
    fn headings_follow_the_v4_pattern() {
        for line in [
            "**Good**",
            "Good:",
            "  _good replies_ :",
            "GOOD",
            "**Good replies**",
        ] {
            assert!(is_good_heading(line), "{line}");
        }
        for line in [
            "**Goodbye**",
            "good_x",
            "Good ***",
            "Not good",
            "**Good** stuff*",
        ] {
            assert!(!is_good_heading(line), "{line}");
        }
    }

    #[test]
    fn section_ends_follow_the_v4_pattern() {
        for line in ["## Next", "---", " *** ", "****", "**Bad**", "###### x"] {
            assert!(is_section_end(line), "{line}");
        }
        for line in ["####### x", "#x", "--", "** **x", "** * **", "> `a`"] {
            assert!(!is_section_end(line), "{line}");
        }
    }

    #[test]
    fn example_quotes_are_greedy() {
        assert_eq!(example_quote(" > `a `b` c`  "), Some("a `b` c"));
        assert_eq!(example_quote("> ``"), None);
        assert_eq!(example_quote("> `x` y"), None);
    }

    #[test]
    fn persona_heading_name() {
        assert_eq!(identity_name("x\n# Persona:  Name  \n"), "Name");
        assert_eq!(identity_name("#Persona: Name"), "The assistant");
        assert_eq!(identity_name("# persona: <Name>\n# Persona: Real"), "Real");
    }
}
