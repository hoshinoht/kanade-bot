//! Every name each member has been known by since serve started. Chat
//! history, anchors, focus cards, the bot's own replies and its notices and
//! cards (reached through reply chains) are text rendered with the names of
//! their time; every masked chat question registers all of them on its
//! identity session (`IdentitySession::former_name`), so a rename, a removed
//! alias or a member who left never goes out unmasked or unscanned. Fed by
//! every live-roster refresh (startup load, roster task, tick) and by each
//! question's roster read. In memory, like the history; `Debug` shows counts.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Mutex, PoisonError};

use serde_json::json;

use crate::domain::members::MemberProfile;
use crate::infrastructure::llm::identity::Member;
use crate::runtime::logging;

/// Former names kept per member (oldest dropped first); current names never
/// count against it.
pub const FORMER_PER_MEMBER: usize = 32;
/// Members remembered; beyond it the least recently seen departed member is
/// forgotten (current members never are).
pub const MEMBERS_REMEMBERED: usize = 4096;

#[derive(Default)]
struct Known {
    /// The names the latest roster gives them (empty once they left).
    current: Vec<String>,
    /// Names no longer current, oldest first.
    former: Vec<String>,
    /// The roster update that last listed them.
    seen: u64,
}

#[derive(Default)]
struct Book {
    members: HashMap<String, Known>,
    updates: u64,
    warned: bool,
}

#[derive(Default)]
pub struct NameHistory {
    book: Mutex<Book>,
}

fn names_of(member: &Member) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let all = std::iter::once(&member.display_name)
        .chain(&member.nickname)
        .chain(&member.aliases);
    for name in all {
        let name = name.trim();
        if !name.is_empty() && !names.iter().any(|known| known == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// The identity view of a stored roster row (bots included: harmless, and
/// the codec never masks the bot itself).
pub fn member_of(profile: &MemberProfile) -> Member {
    Member {
        user_id: profile.member.user_id.clone(),
        display_name: profile.member.display_name.clone().unwrap_or_default(),
        nickname: profile.member.nickname.clone(),
        aliases: profile.aliases.clone(),
    }
}

impl NameHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// A full roster as it stands now: its names become current, names they
    /// dropped (and every name of members missing from it) become former.
    pub fn observe(&self, roster: &[Member]) {
        let mut book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
        book.updates += 1;
        let update = book.updates;
        let mut listed: HashMap<&str, Vec<String>> = HashMap::new();
        for member in roster {
            listed
                .entry(member.user_id.as_str())
                .or_default()
                .extend(names_of(member));
        }
        for (user_id, known) in &mut book.members {
            if !listed.contains_key(user_id.as_str()) {
                retire(known, &[]);
            }
        }
        for (user_id, names) in listed {
            if !book.members.contains_key(user_id) {
                make_room(&mut book);
            }
            let known = book.members.entry(user_id.to_owned()).or_default();
            retire(known, &names);
            known.former.retain(|name| !names.contains(name));
            known.current = names;
            known.seen = update;
        }
    }

    pub fn observe_profiles(&self, profiles: &[MemberProfile]) {
        let roster: Vec<Member> = profiles.iter().map(member_of).collect();
        self.observe(&roster);
    }

    /// Every `(user id, name)` ever seen: per member former names oldest
    /// first, then current ones, so the most recent name is registered last
    /// (a departed member's replies show their latest name).
    pub fn pairs(&self) -> Vec<(String, String)> {
        let book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
        book.members
            .iter()
            .flat_map(|(user_id, known)| {
                known
                    .former
                    .iter()
                    .chain(&known.current)
                    .map(move |name| (user_id.clone(), name.clone()))
            })
            .collect()
    }
}

/// Current names missing from `now` become former (newest last, capped).
fn retire(known: &mut Known, now: &[String]) {
    let dropped: Vec<String> = known
        .current
        .drain(..)
        .filter(|name| !now.contains(name))
        .collect();
    for name in dropped {
        known.former.retain(|former| *former != name);
        known.former.push(name);
    }
    let excess = known.former.len().saturating_sub(FORMER_PER_MEMBER);
    known.former.drain(..excess);
}

/// Forget the least recently seen departed member when the book is full;
/// warn once. Current members are never forgotten.
fn make_room(book: &mut Book) {
    if book.members.len() < MEMBERS_REMEMBERED {
        return;
    }
    let oldest = book
        .members
        .iter()
        .filter(|(_, known)| known.current.is_empty())
        .min_by_key(|(_, known)| known.seen)
        .map(|(user_id, _)| user_id.clone());
    if let Some(user_id) = oldest {
        book.members.remove(&user_id);
        if !book.warned {
            book.warned = true;
            logging::event(
                "WARN",
                "former_names_evicted",
                json!({"remembered": MEMBERS_REMEMBERED}),
            );
        }
    }
}

impl fmt::Debug for NameHistory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("NameHistory")
            .field("members", &book.members.len())
            .field(
                "former",
                &book.members.values().map(|k| k.former.len()).sum::<usize>(),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(user_id: &str, name: &str, nickname: Option<&str>, aliases: &[&str]) -> Member {
        Member {
            user_id: user_id.into(),
            display_name: name.into(),
            nickname: nickname.map(str::to_owned),
            aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
        }
    }

    fn sorted(history: &NameHistory) -> Vec<(String, String)> {
        let mut pairs = history.pairs();
        pairs.sort();
        pairs
    }

    fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(id, name)| ((*id).to_owned(), (*name).to_owned()))
            .collect()
    }

    #[test]
    fn renames_removed_aliases_and_departures_become_former_names() {
        let history = NameHistory::new();
        history.observe(&[
            member("1", "Alicia", Some("Oldnick"), &["zorblax"]),
            member("2", "Ghost Rider", None, &[]),
        ]);
        history.observe(&[member("1", "Alicia", Some("Newnick"), &[])]);
        assert_eq!(
            sorted(&history),
            owned(&[
                ("1", "Alicia"),
                ("1", "Newnick"),
                ("1", "Oldnick"),
                ("1", "zorblax"),
                ("2", "Ghost Rider"),
            ])
        );
        assert_eq!(
            format!("{history:?}"),
            "NameHistory { members: 2, former: 3 }"
        );
    }

    #[test]
    fn current_names_never_count_against_the_cap() {
        let history = NameHistory::new();
        history.observe(&[member("1", "First", None, &[])]);
        let aliases: Vec<String> = (0..FORMER_PER_MEMBER + 5)
            .map(|n| format!("a{n}"))
            .collect();
        let aliases: Vec<&str> = aliases.iter().map(String::as_str).collect();
        history.observe(&[member("1", "Second", None, &aliases)]);
        let pairs = history.pairs();
        assert!(pairs.contains(&("1".into(), "First".into())), "former kept");
        assert_eq!(pairs.len(), FORMER_PER_MEMBER + 5 + 2);
        // Former names alone are capped, oldest dropped.
        for n in 0..FORMER_PER_MEMBER + 3 {
            history.observe(&[member("1", &format!("Name{n}"), None, &[])]);
        }
        let former = history.pairs().len() - 1;
        assert_eq!(former, FORMER_PER_MEMBER);
    }

    #[test]
    fn a_departed_members_latest_name_is_registered_last() {
        let history = NameHistory::new();
        history.observe(&[member("9", "Early", None, &[])]);
        history.observe(&[member("9", "Late", None, &[])]);
        history.observe(&[]);
        let pairs = history.pairs();
        assert_eq!(pairs, owned(&[("9", "Early"), ("9", "Late")]));
    }

    #[test]
    fn a_full_book_forgets_the_least_recently_seen_departed_member() {
        crate::runtime::logging::capture();
        let history = NameHistory::new();
        let everyone: Vec<Member> = (0..MEMBERS_REMEMBERED)
            .map(|n| member(&n.to_string(), &format!("M{n}"), None, &[]))
            .collect();
        history.observe(&everyone);
        // Only member 0 stays; the rest departed, member 1 seen longest ago.
        history.observe(&everyone[..1]);
        history.observe(&[everyone[0].clone(), member("new", "Newcomer", None, &[])]);
        let pairs = history.pairs();
        assert!(pairs.contains(&("new".into(), "Newcomer".into())));
        assert!(pairs.contains(&("0".into(), "M0".into())), "current kept");
        assert_eq!(pairs.len(), MEMBERS_REMEMBERED);
        let lines = crate::runtime::logging::captured();
        assert_eq!(
            lines
                .iter()
                .filter(|line| line["event"] == "former_names_evicted")
                .count(),
            1
        );
    }
}
