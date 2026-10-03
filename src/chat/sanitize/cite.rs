//! `D-VOICED-CARD`: a line citing listed runs by id (`[id]`, `` `id` ``,
//! bare or `` `[id]` ``, any case) is kept, each id read as the run's bold
//! boss label (user decision 2026-10-03, "keep wording, card under").

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use super::{is_word, pattern};

static TOKEN: LazyLock<Regex> = LazyLock::new(|| pattern(r"`?\[?([0-9a-fA-F]{8})\]?`?"));

fn starts_with_label(text: &str, label: &str) -> bool {
    text.trim_start_matches('*')
        .to_lowercase()
        .starts_with(&label.to_lowercase())
}

fn ends_with_label(text: &str, label: &str) -> bool {
    text.trim_end_matches('*')
        .to_lowercase()
        .ends_with(&label.to_lowercase())
}

/// The line with one id swapped for `**label**`, or dropped where the label
/// already stands next to it or the line names it beside a parenthesised id.
fn swap(text: &mut String, start: usize, end: usize, label: &str) {
    let wrapped = text[..start].ends_with('(') && text[end..].starts_with(')');
    let (start, end) = if wrapped {
        (start - 1, end + 1)
    } else {
        (start, end)
    };
    let separator = |c: char| c.is_whitespace() || "—–-:,".contains(c);
    let before = text[..start].trim_end_matches(separator);
    let after = text[end..].trim_start_matches(separator);
    if !label.is_empty() && ends_with_label(before, label) {
        text.replace_range(before.len()..end, "");
    } else if !label.is_empty() && starts_with_label(after, label) {
        let resume = text.len() - after.len();
        text.replace_range(start..resume, "");
    } else if wrapped && !label.is_empty() && text.to_lowercase().contains(&label.to_lowercase()) {
        let trimmed = text[..start].trim_end().len();
        text.replace_range(trimmed..end, "");
    } else {
        text.replace_range(start..end, &format!("**{label}**"));
    }
}

/// The line with each standalone id of a listed run (`labels`, by
/// lowercase id) read as that run's label.
pub(super) fn voiced(line: &str, labels: &BTreeMap<String, String>) -> String {
    let cited: Vec<(usize, usize, &str)> = TOKEN
        .captures_iter(line)
        .filter_map(|token| {
            let whole = token.get(0)?;
            let alone = !line[..whole.start()]
                .chars()
                .next_back()
                .is_some_and(is_word)
                && !line[whole.end()..].chars().next().is_some_and(is_word);
            let label = labels.get(&token[1].to_lowercase())?;
            (alone && !label.is_empty()).then_some((whole.start(), whole.end(), label.as_str()))
        })
        .collect();
    let mut text = line.to_owned();
    for &(start, end, label) in cited.iter().rev() {
        swap(&mut text, start, end, label);
    }
    text
}
