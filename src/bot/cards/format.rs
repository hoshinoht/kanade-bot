//! Proposal card text (v4 `bot.agent.formatting`: `when_text`,
//! `proposal_line`, `card_kind`, `proposal_card`, the card notices, and
//! `Pipeline._unanswered`). Pure; frozen in `docs/v5/vectors/extract/cards.json`.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;

use crate::domain::ids::short_id;
use crate::domain::proposals::{CardDetails, ChangeKind};
use crate::domain::schedule::{EMOJI_NO, EMOJI_YES, Run};

pub const COLOUR_PROPOSAL: u32 = 0xEB459E;
pub const COLOUR_SUGGESTION: u32 = 0xFAA61A;
pub const COLOUR_FIXED: u32 = 0x9B59B6;
pub const TBD: &str = "**TBD**";
/// Put on a card a newer card (or an approved sibling) retired.
pub const SUPERSEDED_NOTICE: &str = "↪ superseded by a newer card";

const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const DIFFICULTY_WORDS: [(&str, &str); 5] = [
    ("e", "Easy"),
    ("n", "Normal"),
    ("h", "Hard"),
    ("c", "Chaos"),
    ("x", "Extreme"),
];

pub fn confirm_hint() -> String {
    format!("React {EMOJI_YES} to confirm, {EMOJI_NO} to reject, or use `/amend` to edit.")
}

pub fn applied_notice(name: &str) -> String {
    format!("✅ applied by {name}")
}

pub fn rejected_notice(name: &str) -> String {
    format!("❌ rejected by {name}")
}

/// Who a card names, and which of them it may notify (v4 `Audience`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Audience {
    pub names: BTreeMap<String, String>,
    pub mentioned: Vec<String>,
}

impl Audience {
    fn name_for(&self, user_id: &str) -> String {
        if self.mentioned.iter().any(|id| id == user_id) {
            return mention(user_id);
        }
        match self.names.get(user_id) {
            Some(name) if !name.is_empty() => name.clone(),
            _ => mention(user_id),
        }
    }
}

fn mention(user_id: &str) -> String {
    format!("<@{user_id}>")
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

pub fn boss_labels(bosses: &[String]) -> String {
    if bosses.is_empty() {
        return "(no bosses)".to_owned();
    }
    let labels: Vec<String> = bosses.iter().map(|token| boss_label(token)).collect();
    labels.join(" + ")
}

fn local_day(at: DateTime<Utc>, zone: Tz) -> String {
    let local = at.with_timezone(&zone);
    format!(
        "{} {:02} {}",
        WEEKDAY_NAMES[local.weekday().num_days_from_monday() as usize],
        local.day(),
        MONTH_NAMES[local.month0() as usize]
    )
}

fn local_time(at: DateTime<Utc>, zone: Tz) -> String {
    let local = at.with_timezone(&zone);
    format!("{:02}:{:02}", local.hour(), local.minute())
}

fn kind_verb(kind: &str) -> &str {
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

fn weekday_name(weekday: u8) -> &'static str {
    WEEKDAY_NAMES[usize::from(weekday) % 7]
}

fn weekly_text(details: &CardDetails, zone: Tz) -> String {
    let payload = &details.payload;
    let when = match (payload.weekday, payload.time.as_deref()) {
        (Some(weekday), Some(time)) if !time.is_empty() => {
            format!("**every {} {time}**", weekday_name(weekday))
        }
        _ => format!("{} — **and every week after**", when_text(details, zone)),
    };
    format!(
        "{when} — recurring from now on, not a one-off; \
         this week's run is added if that night is still ahead"
    )
}

fn weekly_change_text(details: &CardDetails, who: Option<&Audience>) -> Vec<String> {
    let payload = &details.payload;
    let was = payload
        .weekly_when
        .as_deref()
        .filter(|text| !text.is_empty());
    let mut lines = Vec::new();
    match (payload.weekday, payload.time.as_deref()) {
        (Some(weekday), Some(time)) if !time.is_empty() => {
            let moved = format!("**every {} {time}**", weekday_name(weekday));
            let head = match was {
                Some(was) => format!("~~every {was}~~ → {moved}"),
                None => moved,
            };
            lines.push(format!(
                "{head} — the weekly timing itself, so every week from now on; \
                 this week's run moves with it"
            ));
        }
        _ => {
            if let Some(was) = was {
                lines.push(format!("**every {was}** — the night is unchanged"));
            }
        }
    }
    if !payload.participants.is_empty() {
        lines.push(format!(
            "{} → {}",
            format_participants(&details.participants, who),
            format_participants(&payload.participants, who)
        ));
    }
    lines
}

/// `(field name, field value)` for one change on a card. A matched run's
/// bosses head the line, beside its `#id`.
pub fn proposal_line(
    details: &CardDetails,
    run: Option<&Run>,
    zone: Tz,
    who: Option<&Audience>,
) -> (String, String) {
    let kind = details.kind;
    let bosses = boss_labels(run.map_or(&details.bosses, |run| &run.bosses));
    let payload = &details.payload;
    let op = payload.op.as_deref();
    let removes_baseline = kind == ChangeKind::Fix && op == Some("remove");
    let changes_baseline = kind == ChangeKind::Fix && op == Some("edit");
    let verb = match (kind, op) {
        (ChangeKind::Fix, Some("remove")) => "remove weekly",
        (ChangeKind::Fix, Some("edit")) => "change weekly",
        _ => kind_verb(kind.as_str()),
    };
    let mut name = format!("{verb} · {bosses}");
    if let Some(run) = run {
        name.push_str(&format!(" · `#{}`", short_id(&run.id)));
    }

    let mut lines: Vec<String> = Vec::new();
    if removes_baseline {
        let when = payload
            .weekly_when
            .as_deref()
            .filter(|text| !text.is_empty())
            .map(|when| format!(" ({when})"))
            .unwrap_or_default();
        lines.push(format!(
            "**stop scheduling this every week**{when} — future weeks will not be \
             scheduled, and this week's run is cancelled"
        ));
    } else if changes_baseline {
        lines.extend(weekly_change_text(details, who));
    } else {
        match kind {
            ChangeKind::Fix => lines.push(weekly_text(details, zone)),
            ChangeKind::Move | ChangeKind::Add | ChangeKind::Split => {
                let old = match run {
                    Some(run) if kind == ChangeKind::Move => format!(
                        "~~{} {}~~ → ",
                        local_day(run.datetime, zone),
                        local_time(run.datetime, zone)
                    ),
                    _ => String::new(),
                };
                lines.push(old + &when_text(details, zone));
            }
            ChangeKind::Cancel => lines.push("**off this week**".to_owned()),
            ChangeKind::Otot => {
                lines.push("**own time** — stays on the schedule, no countdown pings".to_owned())
            }
            ChangeKind::Sub => {
                let out = if payload.remove.is_empty() {
                    &details.participants
                } else {
                    &payload.remove
                };
                if payload.add.is_empty() {
                    lines.push(format!(
                        "{} **out this week**",
                        format_participants(out, who)
                    ));
                } else {
                    lines.push(format!(
                        "{} out · {} in",
                        format_participants(out, who),
                        format_participants(&payload.add, who)
                    ));
                }
            }
            ChangeKind::Rsvp => lines.push(
                match details.rsvp.as_deref() {
                    Some("yes") => "**can make it**",
                    Some("no") => "**can't make it**",
                    Some("maybe") => "**not sure yet**",
                    _ => "**answered**",
                }
                .to_owned(),
            ),
        }
    }

    let people: &[String] = if details.participants.is_empty() {
        run.map_or(&[], |run| run.participants.as_slice())
    } else {
        &details.participants
    };
    // An edit that changes the party already showed old → new.
    let party_shown = changes_baseline && !payload.participants.is_empty();
    if !people.is_empty() && kind != ChangeKind::Sub && !party_shown {
        lines.push(format_participants(people, who));
    }
    if let Some(summary) = details.summary.as_deref().filter(|text| !text.is_empty()) {
        lines.push(format!("_{summary}_"));
    }
    if !details.also_mentioned.is_empty() {
        let spoken: Vec<&str> = details
            .also_mentioned
            .iter()
            .map(|kind| kind_verb(kind))
            .collect();
        lines.push(format!("(also mentioned: {})", spoken.join(", ")));
    }
    // v5: the self-service link rides on the card (N1 cards-and-link).
    if let Some(line) = &details.self_service {
        lines.push(line.clone());
    }
    (name, lines.join("\n"))
}

/// Which header a card gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    Proposal,
    Suggestion,
    Fix,
}

impl CardKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposal => "proposal",
            Self::Suggestion => "suggestion",
            Self::Fix => "fix",
        }
    }

    fn header(self) -> (&'static str, u32) {
        match self {
            Self::Proposal => ("📋 Proposed change", COLOUR_PROPOSAL),
            Self::Suggestion => ("💡 Suggested amendment", COLOUR_SUGGESTION),
            Self::Fix => ("📌 New fixed timing", COLOUR_FIXED),
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

/// A rendered card (v4 `Card`, without artwork).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardView {
    pub content: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub fields: Vec<(String, String)>,
    pub footer: Option<String>,
    pub colour: u32,
    /// Who the post may notify (its allowed mentions).
    pub mention_users: Vec<String>,
}

/// One card for a burst: one field per change, headed by its kind.
pub fn proposal_card(
    details: &[&CardDetails],
    runs: &BTreeMap<String, &Run>,
    zone: Tz,
    unanswered: Option<&[String]>,
    confidence: Option<f64>,
    who: Option<&Audience>,
) -> CardView {
    let run_of =
        |details: &CardDetails| details.run_id.as_ref().and_then(|id| runs.get(id).copied());
    let (title, colour) = card_kind(details).header();
    let mut mentioned: Vec<String> = Vec::new();
    for change in details {
        let people = if change.participants.is_empty() {
            run_of(change).map_or(&[][..], |run| run.participants.as_slice())
        } else {
            change.participants.as_slice()
        };
        for uid in people {
            if !mentioned.contains(uid) {
                mentioned.push(uid.clone());
            }
        }
    }
    let content = if mentioned.is_empty() {
        title.to_owned()
    } else {
        format!("{title}\n{}", format_participants(&mentioned, who))
    };
    let fields = details
        .iter()
        .map(|change| proposal_line(change, run_of(change), zone, who))
        .collect();
    let mut footer = confirm_hint();
    if let Some(confidence) = confidence {
        footer.push_str(&format!("  (confidence {confidence:.2})"));
    }
    let description = unanswered
        .filter(|waiting| !waiting.is_empty())
        .map(|waiting| format!("Not yet answered: {}", format_participants(waiting, who)));
    CardView {
        content,
        title: None,
        description,
        fields,
        footer: Some(footer),
        colour,
        mention_users: who.map(|who| who.mentioned.clone()).unwrap_or_default(),
    }
}
