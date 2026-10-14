//! A shaped reply as the messages that post it: one message when it fits
//! the member bound (v4), otherwise follow-ups split at paragraph breaks
//! outside fenced code. An oversized fence is split at line boundaries into
//! fenced chunks that reopen it; the part count is capped.

use std::ops::Range;

use super::fence::fenced;
use crate::chat::tools::MAX_MEMBER_REPLY;
use crate::domain::pytext::strip;

/// Most messages one reply posts.
pub const MAX_REPLY_PARTS: usize = 4;
/// Ends the last message when the reply needed more than [`MAX_REPLY_PARTS`].
pub const TRIMMED: &str = "*(reply trimmed)*";
/// Discord's message limit in UTF-16 units.
const DISCORD_UNITS: usize = 2000;
const CLOSE: &str = "\n```";

fn fits(text: &str, limit: usize) -> bool {
    text.chars().count() <= limit
        && text.encode_utf16().count() <= limit + (DISCORD_UNITS - MAX_MEMBER_REPLY)
}

/// The messages posting `text`, in order; `[text]` when it fits one.
pub fn reply_parts(text: &str) -> Vec<String> {
    if fits(text, MAX_MEMBER_REPLY) {
        return vec![text.to_owned()];
    }
    let mut parts = pack(text, MAX_MEMBER_REPLY);
    if parts.len() > MAX_REPLY_PARTS {
        parts.truncate(MAX_REPLY_PARTS);
        let last = parts.pop().expect("parts");
        let room = MAX_MEMBER_REPLY - TRIMMED.chars().count() - 2;
        let head = pack(&last, room).swap_remove(0);
        parts.push(format!("{head}\n\n{TRIMMED}"));
    }
    parts
}

/// `(start, end)` byte offsets of each `\n`-separated line.
fn lines(text: &str, within: Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = within.start;
    for line in text[within.clone()].split('\n') {
        out.push(start..start + line.len());
        start += line.len() + 1;
    }
    out
}

/// Paragraphs: runs of lines between blank lines outside fences.
fn paragraphs(text: &str, fences: &[Range<usize>]) -> Vec<Range<usize>> {
    let fenced = |line: &Range<usize>| {
        fences
            .iter()
            .any(|block| line.start < block.end && block.start < line.end.max(line.start + 1))
    };
    let mut out: Vec<Range<usize>> = Vec::new();
    let mut open: Option<Range<usize>> = None;
    for line in lines(text, 0..text.len()) {
        let separator = strip(&text[line.clone()]).is_empty() && !fenced(&line);
        match (&mut open, separator) {
            (Some(paragraph), false) => paragraph.end = line.end,
            (None, false) => open = Some(line),
            (Some(_), true) => out.extend(open.take()),
            (None, true) => {}
        }
    }
    out.extend(open);
    out
}

/// Greedy packing of paragraphs into messages within `limit`.
fn pack(text: &str, limit: usize) -> Vec<String> {
    let fences = fenced(text);
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    for paragraph in paragraphs(text, &fences) {
        let body = &text[paragraph.clone()];
        let joined = if current.is_empty() {
            body.to_owned()
        } else {
            format!("{current}\n\n{body}")
        };
        if fits(&joined, limit) {
            current = joined;
            continue;
        }
        if !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        if fits(body, limit) {
            current = body.to_owned();
        } else {
            let mut pieces = cut(text, paragraph, &fences, limit);
            current = pieces.pop().unwrap_or_default();
            parts.extend(pieces);
        }
    }
    if !current.is_empty() || parts.is_empty() {
        parts.push(current);
    }
    parts
}

/// One oversized paragraph at line boundaries (a line too long for one
/// part at characters); a fence open at a cut is closed there and reopened
/// with its marker and language.
fn cut(text: &str, paragraph: Range<usize>, fences: &[Range<usize>], limit: usize) -> Vec<String> {
    let open_after = |at: usize| reopener(text, fences, at);
    let mut pieces: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut reopen: Option<String> = None;
    for line in lines(text, paragraph) {
        let mut at = line.start;
        loop {
            let body = &text[at..line.end];
            let after = open_after(line.end);
            let close = if after.is_some() { CLOSE } else { "" };
            let base = if current.is_empty() {
                reopen
                    .as_ref()
                    .map(|open| format!("{open}\n"))
                    .unwrap_or_default()
            } else if at == line.start {
                format!("{current}\n")
            } else {
                current.clone()
            };
            let joined = format!("{base}{body}");
            if fits(&format!("{joined}{close}"), limit) {
                current = joined;
                reopen = after;
                break;
            }
            if !current.is_empty() {
                if reopen.is_some() {
                    current.push_str(CLOSE);
                }
                pieces.push(std::mem::take(&mut current));
                // The cut already closed the fence this line closes.
                let indent = body.len() - body.trim_start().len();
                let closes = at == line.start
                    && reopen.is_some()
                    && fences.iter().any(|b| b.end == at + indent + FENCE_LEN);
                if closes {
                    // The rest of the line goes back through the size check.
                    let rest = &body[indent + FENCE_LEN..];
                    at += indent + FENCE_LEN + (rest.len() - rest.trim_start().len());
                    reopen = None;
                    continue;
                }
                continue;
            }
            let prefix = base;
            let room = limit
                .saturating_sub(prefix.chars().count() + CLOSE.len())
                .max(1);
            let mut end = at + head(body, room).len();
            // Never cut through a fence marker.
            for block in fences {
                for marker in [block.start, block.end - FENCE_LEN] {
                    if marker < end && end < marker + FENCE_LEN {
                        end = if marker > at {
                            marker
                        } else {
                            marker + FENCE_LEN
                        };
                    }
                }
            }
            let open = open_after(end);
            let wrap = if open.is_some() { CLOSE } else { "" };
            pieces.push(format!("{prefix}{}{wrap}", &text[at..end]));
            reopen = open;
            at = end;
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces
}

const FENCE_LEN: usize = 3;

/// The marker reopening the fence still open after byte `at`: ```` ``` ````
/// plus its language when the opening line carries only that.
fn reopener(text: &str, fences: &[Range<usize>], at: usize) -> Option<String> {
    let block = fences.iter().find(|b| b.start < at && b.end > at)?;
    let info_start = block.start + FENCE_LEN;
    // A language is a short bare word alone on the opening line.
    let info = text[info_start..block.end]
        .split_once('\n')
        .map_or("", |(first, _)| first.trim());
    let language = info.len() <= 24
        && info
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+#._-".contains(c));
    Some(if language {
        format!("```{info}")
    } else {
        "```".to_owned()
    })
}

/// Characters that attach to the one before them (ZWJ sequences, variation
/// selectors, skin tones, combining marks, tag characters).
fn attaches(c: char) -> bool {
    matches!(c,
        '\u{200d}' | '\u{fe00}'..='\u{fe0f}' | '\u{1f3fb}'..='\u{1f3ff}'
        | '\u{0300}'..='\u{036f}' | '\u{20d0}'..='\u{20ff}' | '\u{e0020}'..='\u{e007f}')
}

/// The longest prefix of `text` that fits `room` without splitting a joined
/// emoji; at least one character sequence.
fn head(text: &str, room: usize) -> &str {
    let mut end = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let next = at + c.len_utf8();
        if end > 0 && !fits(&text[..next], room) {
            break;
        }
        let joined = c == '\u{200d}' || chars.peek().is_some_and(|&(_, n)| attaches(n));
        if !joined || chars.peek().is_none() {
            end = next;
        }
    }
    if end == 0 {
        // One sequence longer than the room: take it whole.
        let mut chars = text.char_indices().peekable();
        while let Some((at, c)) = chars.next() {
            end = at + c.len_utf8();
            let joined = c == '\u{200d}' || chars.peek().is_some_and(|&(_, n)| attaches(n));
            if !joined {
                break;
            }
        }
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fence(lines: usize) -> String {
        let body: Vec<String> = (0..lines)
            .map(|n| format!("    let value_{n} = compute({n}); // keeps  spacing"))
            .collect();
        format!("```rust\n{}\n```", body.join("\n"))
    }

    fn all_fit(parts: &[String]) {
        for part in parts {
            assert!(fits(part, MAX_MEMBER_REPLY), "{}", part.chars().count());
            assert_eq!(part.matches("```").count() % 2, 0, "{part}");
        }
    }

    #[test]
    fn a_reply_within_the_bound_is_one_unchanged_message() {
        let text = "Hello  there\n\n\n```py\n x\n```";
        assert_eq!(reply_parts(text), vec![text.to_owned()]);
        let edge = "a".repeat(MAX_MEMBER_REPLY);
        assert_eq!(reply_parts(&edge), vec![edge]);
    }

    #[test]
    fn a_long_reply_splits_at_paragraphs_and_keeps_code_whole() {
        let code = fence(12);
        let intro = "Intro line. ".repeat(40);
        let text = format!("{}\n\n{code}\n\nBye!", strip(&intro));
        let parts = reply_parts(&text);
        all_fit(&parts);
        assert_eq!(parts.join("\n\n"), text);
        assert!(parts.iter().any(|part| part.contains(&code)));
    }

    #[test]
    fn an_oversized_fence_splits_into_reopened_fences() {
        let code = fence(60);
        let parts = reply_parts(&format!("Here:\n{code}"));
        assert!(parts.len() > 1);
        all_fit(&parts);
        let lines: Vec<&str> = parts
            .iter()
            .flat_map(|part| part.lines())
            .filter(|line| !line.starts_with("```") && *line != "Here:")
            .collect();
        let original: Vec<&str> = code.lines().filter(|l| !l.starts_with("```")).collect();
        assert_eq!(lines, original);
        for part in &parts[1..] {
            assert!(part.starts_with("```rust\n"), "{part}");
        }
    }

    #[test]
    fn a_huge_reply_is_capped_with_a_note() {
        let text = (0..40)
            .map(|n| format!("Paragraph {n}: {}", "words ".repeat(30)))
            .collect::<Vec<_>>()
            .join("\n\n");
        let parts = reply_parts(&text);
        assert_eq!(parts.len(), MAX_REPLY_PARTS);
        all_fit(&parts);
        assert!(parts[3].ends_with(TRIMMED));
        assert!(parts[0].starts_with("Paragraph 0:"));
    }

    #[test]
    fn a_single_giant_line_is_cut_by_characters() {
        let text = "é".repeat(3000);
        let parts = reply_parts(&text);
        all_fit(&parts);
        assert_eq!(parts.concat(), text);
    }

    /// No part is a fence with nothing in it.
    fn no_empty_fences(parts: &[String]) {
        for part in parts {
            assert!(!part.contains("```\n```"), "{part}");
            assert!(!part.contains("```rust\n```"), "{part}");
        }
    }

    #[test]
    fn a_cut_at_the_closing_fence_leaves_no_empty_fence() {
        // Fill a part exactly up to the closing fence line.
        let mut cut_at_closer = false;
        for lines in 20..40 {
            for closer in ["```", "``` and that's it, a closing line with words."] {
                let code = fence(lines);
                let code = format!("{}{closer}", &code[..code.len() - 3]);
                let text = format!("{code}\n\nAfter.");
                let parts = reply_parts(&text);
                all_fit(&parts);
                no_empty_fences(&parts);
                assert!(parts.last().unwrap().ends_with("After."));
                cut_at_closer |= parts.iter().any(|part| part.starts_with("and that's it"));
            }
        }
        assert!(cut_at_closer, "no case cut at the closing line");
    }

    #[test]
    fn a_long_line_after_a_closing_fence_still_fits() {
        let tail = "words after the fence ".repeat(80);
        for lines in 20..40 {
            let code = fence(lines);
            let text = format!("{}``` {tail}\n\nAfter.", &code[..code.len() - 3]);
            let parts = reply_parts(&text);
            all_fit(&parts);
            no_empty_fences(&parts);
        }
    }

    #[test]
    fn a_reopened_fence_carries_only_the_marker_and_language() {
        let opener = "```rust";
        let code = fence(40);
        let parts = reply_parts(&code);
        for part in &parts[1..] {
            assert!(part.starts_with(&format!("{opener}\n    let")), "{part}");
        }
        // An opener with code on its line reopens as a bare fence.
        let inline = format!("```fn main() {{\n{}\n```", "    x();\n".repeat(200));
        let parts = reply_parts(&inline);
        assert!(parts.len() > 1);
        all_fit(&parts);
        for part in &parts[1..] {
            assert!(part.starts_with("```\n"), "{part}");
        }
    }

    #[test]
    fn an_oversized_line_opening_a_fence_is_reopened() {
        let text = format!("Look: ```{}```", "x".repeat(3000));
        let parts = reply_parts(&text);
        assert!(parts.len() > 1);
        all_fit(&parts);
        for part in &parts[1..] {
            assert!(part.starts_with("```"), "{part}");
        }
    }

    #[test]
    fn a_character_cut_keeps_joined_emoji_whole() {
        let family = "👩\u{200d}👩\u{200d}👧";
        let text = format!("{}{}", "a".repeat(1199), family.repeat(10));
        let parts = reply_parts(&text);
        all_fit(&parts);
        for part in &parts {
            assert!(
                !part.starts_with('\u{200d}') && !part.ends_with('\u{200d}'),
                "{part:?}"
            );
            let first = part.chars().next().unwrap();
            assert!(first == 'a' || first == '👩', "{part:?}");
        }
        assert_eq!(parts.concat(), text);
    }
}
