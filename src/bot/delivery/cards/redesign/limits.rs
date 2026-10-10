//! Discord's embed limits, and clipping text to them.

/// Embeds per message.
pub const MAX_EMBEDS: usize = 10;
/// Characters across every embed of one message.
pub const MAX_TOTAL_CHARS: usize = 6000;
pub const MAX_TITLE: usize = 256;
pub const MAX_DESCRIPTION: usize = 4096;
pub const MAX_FIELD_VALUE: usize = 1024;

use super::super::{CardEmbed, CardField};

/// `text` cut to `max` characters, ending in `…` when cut.
pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Lines joined with `\n` up to `max` characters; lines that do not fit
/// become `… and n more`.
pub fn clip_lines(lines: &[String], max: usize) -> String {
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        let rest = lines.len() - index - 1;
        let candidate = if out.is_empty() {
            line.clone()
        } else {
            format!("{out}\n{line}")
        };
        let tail = if rest == 0 {
            String::new()
        } else {
            format!("\n… and {rest} more")
        };
        if (candidate.clone() + &tail).chars().count() <= max {
            out = candidate;
            continue;
        }
        let more = format!("… and {} more", lines.len() - index);
        return if out.is_empty() {
            more
        } else {
            format!("{out}\n{more}")
        };
    }
    out
}

/// Characters Discord counts towards [`MAX_TOTAL_CHARS`].
pub fn embed_chars(embed: &CardEmbed) -> usize {
    let count = |text: &Option<String>| text.as_deref().map_or(0, |text| text.chars().count());
    count(&embed.title)
        + count(&embed.description)
        + count(&embed.footer)
        + embed
            .fields
            .iter()
            .map(|CardField { name, value, .. }| name.chars().count() + value.chars().count())
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_keeps_whole_lines_and_counts_the_rest() {
        assert_eq!(clip("abcdef", 6), "abcdef");
        assert_eq!(clip("abcdefg", 6), "abcde…");
        let lines: Vec<String> = ["one", "two", "three"].map(str::to_owned).to_vec();
        assert_eq!(clip_lines(&lines, 100), "one\ntwo\nthree");
        assert_eq!(clip_lines(&lines, 18), "one\n… and 2 more");
        assert_eq!(clip_lines(&lines, 5), "… and 3 more");
    }
}
