//! Proposal card wording shared by the card and its V2 layout (v4
//! `bot.agent.formatting`: `when_text`, `card_kind`, the audience, and
//! `Pipeline._unanswered`). Pure.

use std::collections::BTreeMap;

use chrono_tz::Tz;

use crate::cards::text::{WEEKDAY_NAMES, local_day, local_time, mention};
use kanade_domain::proposals::{CardDetails, ChangeKind};
use kanade_domain::schedule::Run;

pub const COLOUR_PROPOSAL: u32 = 0xEB459E;
pub const COLOUR_SUGGESTION: u32 = 0xFAA61A;
pub const COLOUR_FIXED: u32 = 0x9B59B6;
pub const TBD: &str = "**TBD**";

const DIFFICULTY_WORDS: [(&str, &str); 5] = [
    ("e", "Easy"),
    ("n", "Normal"),
    ("h", "Hard"),
    ("c", "Chaos"),
    ("x", "Extreme"),
];

/// Who a card names, and which of them it may notify (v4 `Audience`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Audience {
    pub names: BTreeMap<String, String>,
    pub mentioned: Vec<String>,
}

impl Audience {
    pub(crate) fn name_for(&self, user_id: &str) -> String {
        if self.mentioned.iter().any(|id| id == user_id) {
            return mention(user_id);
        }
        match self.names.get(user_id) {
            Some(name) if !name.is_empty() => name.clone(),
            _ => mention(user_id),
        }
    }
}

/// v4 `format_participants`: everyone a mention without an audience.
pub fn format_participants(user_ids: &[String], who: Option<&Audience>) -> String {
    if user_ids.is_empty() {
        return "(nobody)".to_owned();
    }
    let people: Vec<String> = match who {
        Some(who) => user_ids.iter().map(|id| who.name_for(id)).collect(),
        None => user_ids.iter().map(|id| mention(id)).collect(),
    };
    people.join(" ")
}

/// `"XKalos"` → `"Extreme Kalos"`; anything else unchanged.
pub fn boss_label(token: &str) -> String {
    let mut chars = token.chars();
    let (Some(letter), short) = (chars.next(), chars.as_str()) else {
        return token.to_owned();
    };
    let letter = letter.to_lowercase().to_string();
    match DIFFICULTY_WORDS.iter().find(|(key, _)| *key == letter) {
        Some((_, word)) if !short.is_empty() => format!("{word} {short}"),
        _ => token.to_owned(),
    }
}

pub fn kind_verb(kind: &str) -> &str {
    match kind {
        "add" => "new run",
        "otot" => "own time",
        "sub" => "stand-in",
        "fix" => "new weekly",
        "rsvp" => "answer",
        other => other,
    }
}

/// The new day/time, else the words actually written; never invented.
pub fn when_text(details: &CardDetails, zone: Tz) -> String {
    if let Some(at) = details.new_datetime {
        return format!("**{} {}**", local_day(at, zone), local_time(at, zone));
    }
    let day = details.day_ref.as_deref().filter(|text| !text.is_empty());
    let time = details.time_ref.as_deref().filter(|text| !text.is_empty());
    match (day, time) {
        (Some(day), Some(time)) => format!("**{day} {time}** (couldn't read that as a date)"),
        (Some(day), None) => format!("**{day}** — time {TBD}"),
        (None, Some(time)) => format!("**{time}** — day {TBD}"),
        (None, None) => TBD.to_owned(),
    }
}

pub fn weekday_name(weekday: u8) -> &'static str {
    WEEKDAY_NAMES[usize::from(weekday) % 7]
}

kanade_domain::spelled! {
    /// Which mark a card gets.
    CardKind {
        Proposal => "proposal",
        Suggestion => "suggestion",
        Fix => "fix",
    }
}

impl CardKind {
    /// The card's emoji and colour.
    pub fn mark(self) -> (&'static str, u32) {
        match self {
            Self::Proposal => ("📋", COLOUR_PROPOSAL),
            Self::Suggestion => ("💡", COLOUR_SUGGESTION),
            Self::Fix => ("📌", COLOUR_FIXED),
        }
    }
}

pub fn card_kind(details: &[&CardDetails]) -> CardKind {
    if !details.is_empty() && details.iter().all(|d| d.kind == ChangeKind::Fix) {
        return CardKind::Fix;
    }
    if details
        .iter()
        .any(|d| d.is_question || d.new_datetime.is_none())
    {
        CardKind::Suggestion
    } else {
        CardKind::Proposal
    }
}

/// v4 `Pipeline._unanswered`: on a card still asking something, the affected
/// runs' members who have not spoken, in run order.
pub fn unanswered(details: &[&CardDetails], runs: &[&Run]) -> Vec<String> {
    if !details
        .iter()
        .any(|d| d.is_question || d.new_datetime.is_none())
    {
        return Vec::new();
    }
    let spoke: Vec<&String> = details.iter().flat_map(|d| &d.participants).collect();
    let mut waiting: Vec<String> = Vec::new();
    for run in runs {
        for uid in &run.participants {
            if !spoke.contains(&uid) && !waiting.contains(uid) {
                waiting.push(uid.clone());
            }
        }
    }
    waiting
}
