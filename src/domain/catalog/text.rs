use crate::domain::pytext::{is_space, strip};

/// Alias key: lowercase ASCII letters and digits only, so spacing and
/// punctuation never distinguish two names.
pub(super) fn normalise(token: &str) -> String {
    strip(token)
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        .collect()
}

/// Word separators inside a free-form reference.
pub(super) fn is_reference_separator(c: char) -> bool {
    matches!(c, ',' | '/' | '+' | '&') || is_space(c)
}

/// Hard separators between bosses in a scheduling list. Spaces are not: multi-word
/// names contain them and are matched greedily instead.
pub(super) fn is_list_separator(c: char) -> bool {
    matches!(c, ',' | '/' | '+' | '&')
}

/// First character and remainder of an ASCII alias key.
pub(super) fn split_letter(key: &str) -> Option<(&str, &str)> {
    let first = key.chars().next()?;
    Some(key.split_at(first.len_utf8()))
}
