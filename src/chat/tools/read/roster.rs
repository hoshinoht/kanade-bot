//! Free-text party fields → roster ids (v4 `agent/util.py`
//! `resolve_participant_text` and `match_roster`).

use std::sync::LazyLock;

use regex::Regex;

use crate::domain::members::Member;
use crate::domain::pytext::strip;

static MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@!?(\d+)>").expect("pattern"));
static BARE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{15,25})\b").expect("pattern"));
static TOKEN_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[,\s\x1C-\x1F]+").expect("pattern"));

/// A participants field turned into ids, preserving order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameResolution {
    pub ids: Vec<String>,
    /// Tokens that matched nobody.
    pub unknown: Vec<String>,
    /// Token → the display names it could have meant, in first-seen order.
    pub ambiguous: Vec<(String, Vec<String>)>,
}

/// The roster names are matched against: role holders, by display name
/// case-insensitively (v4 `list_members()` ordering, ASCII `NOCASE`).
pub fn bossers(members: &[Member]) -> Vec<&Member> {
    let mut out: Vec<&Member> = members.iter().filter(|m| m.has_role).collect();
    out.sort_by_key(|m| {
        m.display_name
            .clone()
            .unwrap_or_default()
            .to_ascii_lowercase()
    });
    out
}

fn candidate_names(member: &Member) -> Vec<&str> {
    [member.display_name.as_deref(), member.nickname.as_deref()]
        .into_iter()
        .flatten()
        .filter(|name| !name.is_empty())
        .collect()
}

/// Members whose name or nickname matches `token`: exact, else prefix, else
/// substring, first tier with any hit wins.
pub fn match_roster<'a>(token: &str, roster: &[&'a Member]) -> Vec<&'a Member> {
    let low = strip(token).to_lowercase();
    if low.is_empty() {
        return Vec::new();
    }
    let tests: [&dyn Fn(&str) -> bool; 3] = [
        &|name| name.to_lowercase() == low,
        &|name| name.to_lowercase().starts_with(&low),
        &|name| name.to_lowercase().contains(&low),
    ];
    for test in tests {
        let mut hits: Vec<&Member> = Vec::new();
        for member in roster {
            if candidate_names(member).into_iter().any(test)
                && !hits.iter().any(|hit| hit.user_id == member.user_id)
            {
                hits.push(member);
            }
        }
        if !hits.is_empty() {
            return hits;
        }
    }
    Vec::new()
}

/// Mentions and bare snowflakes first, then names for whatever is left.
pub fn resolve_participant_text(text: &str, roster: &[&Member]) -> NameResolution {
    let mut result = NameResolution::default();
    if text.is_empty() {
        return result;
    }
    let add = |ids: &mut Vec<String>, uid: &str| {
        if !ids.iter().any(|seen| seen == uid) {
            ids.push(uid.to_owned());
        }
    };
    for found in MENTION.captures_iter(text) {
        add(&mut result.ids, &found[1]);
    }
    let remaining = MENTION.replace_all(text, " ");
    for found in BARE_ID.captures_iter(&remaining) {
        add(&mut result.ids, &found[1]);
    }
    let remaining = BARE_ID.replace_all(&remaining, " ");
    for raw in TOKEN_SPLIT.split(&remaining) {
        let token = strip(strip(raw).trim_start_matches('@'));
        if token.is_empty() {
            continue;
        }
        let matches = match_roster(token, roster);
        match matches.as_slice() {
            [] => {
                if !result.unknown.iter().any(|seen| seen == token) {
                    result.unknown.push(token.to_owned());
                }
            }
            [one] => add(&mut result.ids, &one.user_id),
            many => {
                let names = many
                    .iter()
                    .map(|m| {
                        m.display_name
                            .clone()
                            .filter(|name| !name.is_empty())
                            .unwrap_or_else(|| m.user_id.clone())
                    })
                    .collect();
                match result.ambiguous.iter_mut().find(|(key, _)| key == token) {
                    Some(entry) => entry.1 = names,
                    None => result.ambiguous.push((token.to_owned(), names)),
                }
            }
        }
    }
    result
}
