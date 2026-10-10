//! Who a post names and who it may notify, resolved through one policy.

use std::collections::BTreeMap;

use super::policy::{PingKind, wants_mention};
use crate::domain::members::Directory;
use crate::domain::schedule::RsvpState;

fn push_unique(out: &mut Vec<String>, user_id: &str) -> bool {
    let fresh = !out.iter().any(|seen| seen == user_id);
    if fresh {
        out.push(user_id.to_owned());
    }
    fresh
}

/// The subset of `candidates` a `kind` of post may @mention, in order and
/// without duplicates.
pub fn resolve_mentions<S: AsRef<str>>(
    members: &dyn Directory,
    candidates: &[S],
    kind: &PingKind,
) -> Vec<String> {
    let mut out = Vec::new();
    for candidate in candidates {
        let user_id = candidate.as_ref();
        if !out.iter().any(|seen: &String| seen == user_id)
            && wants_mention(members.ping_level(user_id), kind)
        {
            out.push(user_id.to_owned());
        }
    }
    out
}

/// `user_id -> name` for everyone listed that has a name on file, first
/// occurrence order.
pub fn display_names<S: AsRef<str>>(
    members: &dyn Directory,
    user_ids: &[S],
) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    let mut names = Vec::new();
    for user_id in user_ids {
        let user_id = user_id.as_ref();
        if !push_unique(&mut seen, user_id) {
            continue;
        }
        if let Some(name) = members.display_name(user_id) {
            names.push((user_id.to_owned(), name));
        }
    }
    names
}

/// Everything a post needs to render its people: names, and who to notify.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Audience {
    pub names: Vec<(String, String)>,
    pub mentioned: Vec<String>,
}

/// `people` is everyone the post lists; `candidates` (default: all of them) is
/// the subset it would notify if they all wanted it.
pub fn audience<S: AsRef<str>>(
    members: &dyn Directory,
    people: &[S],
    kind: &PingKind,
    candidates: Option<&[S]>,
) -> Audience {
    Audience {
        names: display_names(members, people),
        mentioned: resolve_mentions(members, candidates.unwrap_or(people), kind),
    }
}

/// A swap notice lists the remaining line-up followed by those leaving.
pub fn swap_audience<S: AsRef<str>>(
    members: &dyn Directory,
    remaining: &[S],
    leaving: &[S],
) -> Audience {
    let people: Vec<&str> = remaining.iter().chain(leaving).map(AsRef::as_ref).collect();
    audience(members, &people, &PingKind::Swap, None)
}

/// Everyone on a run bar explicit decliners: who a countdown may notify.
pub fn not_declined(participants: &[String], rsvps: &BTreeMap<String, RsvpState>) -> Vec<String> {
    participants
        .iter()
        .filter(|user| rsvps.get(*user) != Some(&RsvpState::No))
        .cloned()
        .collect()
}

/// Every participant across `runs`, first occurrence order.
pub fn everyone_on<'a>(runs: impl IntoIterator<Item = &'a [String]>) -> Vec<String> {
    let mut seen = Vec::new();
    for participants in runs {
        for user_id in participants {
            push_unique(&mut seen, user_id);
        }
    }
    seen
}
