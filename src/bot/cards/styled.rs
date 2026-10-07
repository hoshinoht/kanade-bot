//! The redesigned proposal card (`MessageStyle::Redesigned`): a verb-led
//! question in the content, one field group per change (From / To / Party
//! inline), who has not answered as subtext, and the run ids, confidence and
//! how to answer in the footer. An outcome recolours the card and adds a
//! subtext line. Classic cards stay in `format.rs`, byte-exact. Frozen in
//! `docs/v5/vectors/extract/cards_redesigned.json`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

use super::format::{
    Audience, card_kind, kind_verb, local_day, local_time, weekday_name, when_text,
};
use crate::bot::delivery::cards::CardField;
use crate::bot::delivery::cards::redesign::{
    DifficultyMarks, SETTLED_GREEN, boss_labels, full_time, subtext,
};
use crate::domain::catalog::BossTable;
use crate::domain::ids::tag;
use crate::domain::proposals::{CardDetails, ChangeKind};
use crate::domain::schedule::{EMOJI_NO, EMOJI_YES, Run};

/// A closed card: rejected, superseded or out of date.
pub const CLOSED_GREY: u32 = 0x4E5058;

/// What became of a change on the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Closure {
    Applied {
        by: String,
        at: DateTime<Utc>,
    },
    Rejected {
        by: String,
        at: DateTime<Utc>,
    },
    Superseded,
    /// A ✅ was refused because the schedule changed since the card.
    Stale,
}

/// How the card names things: the guild zone, the catalog and difficulty
/// marks for bosses, and the audience for people.
#[derive(Clone, Copy)]
pub struct Look<'a> {
    pub zone: Tz,
    pub catalog: Option<&'a BossTable>,
    pub marks: &'a DifficultyMarks,
    pub who: Option<&'a Audience>,
}

/// Where the card stands: its outcomes (distinct, in order), whether any
/// change still waits for an answer, and further lines shown as subtext.
#[derive(Clone, Copy, Debug)]
pub struct CardState<'a> {
    pub closures: &'a [Closure],
    pub open: bool,
    pub notes: &'a [String],
}

impl CardState<'static> {
    /// A fresh card: nothing decided yet.
    pub const OPEN: Self = Self {
        closures: &[],
        open: true,
        notes: &[],
    };
}

/// A rendered redesigned card (one embed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyledCard {
    pub content: String,
    pub description: Option<String>,
    pub fields: Vec<CardField>,
    pub footer: String,
    pub colour: u32,
    /// Who the post may notify (its allowed mentions).
    pub mention_users: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    Open,
    Applied,
    Closed,
}

fn tone(state: &CardState<'_>) -> Tone {
    let applied = state
        .closures
        .iter()
        .any(|closure| matches!(closure, Closure::Applied { .. }));
    if state.closures.contains(&Closure::Stale) {
        Tone::Closed
    } else if state.open || state.closures.is_empty() {
        Tone::Open
    } else if applied {
        Tone::Applied
    } else {
        Tone::Closed
    }
}

/// People by name (mentions without an audience), joined `, `.
fn names(ids: &[String], who: Option<&Audience>, nobody: &str) -> String {
    if ids.is_empty() {
        return nobody.to_owned();
    }
    let people: Vec<String> = ids
        .iter()
        .map(|id| who.map_or_else(|| format!("<@{id}>"), |who| who.name_for(id)))
        .collect();
    people.join(", ")
}

fn filled(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.is_empty())
}

/// The run's party, else the change's own (a new run has no run yet).
fn people<'a>(details: &'a CardDetails, run: Option<&'a Run>) -> &'a [String] {
    match run {
        Some(run) if !run.participants.is_empty() => &run.participants,
        _ => &details.participants,
    }
}

fn leaving(details: &CardDetails) -> &[String] {
    if details.payload.remove.is_empty() {
        &details.participants
    } else {
        &details.payload.remove
    }
}

/// `Wed 21:30` from a timing payload, if it names both.
fn weekly_slot(details: &CardDetails) -> Option<String> {
    let payload = &details.payload;
    match (payload.weekday, filled(payload.time.as_deref())) {
        (Some(weekday), Some(time)) => Some(format!("{} {time}", weekday_name(weekday))),
        _ => None,
    }
}

/// The new slot as a full Discord timestamp, else the words written.
fn to_text(details: &CardDetails, zone: Tz) -> String {
    match details.new_datetime {
        Some(at) => format!("**{}**", full_time(at)),
        None => when_text(details, zone),
    }
}

fn question(details: &CardDetails, bosses: &str, who: Option<&Audience>) -> String {
    let payload = &details.payload;
    match details.kind {
        ChangeKind::Move => format!("Move {bosses}?"),
        ChangeKind::Add | ChangeKind::Split => format!("Add {bosses}?"),
        ChangeKind::Cancel => format!("Cancel {bosses} this week?"),
        ChangeKind::Otot => format!("Put {bosses} on own time?"),
        ChangeKind::Sub => {
            let out = names(leaving(details), who, "someone");
            if payload.add.is_empty() {
                format!("Count {out} out of {bosses}?")
            } else {
                let standing_in = names(&payload.add, who, "someone");
                format!("Swap {out} for {standing_in} on {bosses}?")
            }
        }
        ChangeKind::Rsvp => {
            let people = names(&details.participants, who, "someone");
            match details.rsvp.as_deref() {
                Some("yes") => format!("Count {people} in for {bosses}?"),
                Some("no") => format!("Count {people} out of {bosses}?"),
                Some("maybe") => format!("Mark {people} unsure for {bosses}?"),
                _ => format!("Note {people}'s answer for {bosses}?"),
            }
        }
        ChangeKind::Fix => match payload.op.as_deref() {
            Some("remove") => format!("Stop scheduling {bosses} every week?"),
            Some("edit") => format!("Change the weekly timing of {bosses}?"),
            _ => format!("Make {bosses} weekly?"),
        },
    }
}

/// One change's inline fields.
fn group(details: &CardDetails, run: Option<&Run>, look: Look<'_>) -> Vec<CardField> {
    let (zone, who) = (look.zone, look.who);
    let payload = &details.payload;
    let party = || CardField::inline("Party", names(people(details, run), who, "—"));
    let when = || {
        CardField::inline(
            "When",
            run.map_or_else(|| "—".to_owned(), |run| full_time(run.datetime)),
        )
    };
    let was = filled(payload.weekly_when.as_deref());
    match details.kind {
        ChangeKind::Move => {
            let from = run.map_or_else(
                || "—".to_owned(),
                |run| {
                    format!(
                        "~~{} {}~~",
                        local_day(run.datetime, zone),
                        local_time(run.datetime, zone)
                    )
                },
            );
            vec![
                CardField::inline("From", from),
                CardField::inline("To", to_text(details, zone)),
                party(),
            ]
        }
        ChangeKind::Add | ChangeKind::Split => {
            vec![CardField::inline("To", to_text(details, zone)), party()]
        }
        ChangeKind::Cancel => vec![
            when(),
            CardField::inline("Change", "**Off this week**"),
            party(),
        ],
        ChangeKind::Otot => vec![
            when(),
            CardField::inline("Change", "**Own time**\n-# no countdown pings"),
            party(),
        ],
        ChangeKind::Sub => vec![
            when(),
            CardField::inline("Out", names(leaving(details), who, "—")),
            CardField::inline("In", names(&payload.add, who, "—")),
        ],
        ChangeKind::Rsvp => {
            let answer = match details.rsvp.as_deref() {
                Some("yes") => "**Can make it**",
                Some("no") => "**Can't make it**",
                Some("maybe") => "**Not sure yet**",
                _ => "**Answered**",
            };
            vec![
                when(),
                CardField::inline("Answer", answer),
                CardField::inline("Who", names(&details.participants, who, "—")),
            ]
        }
        ChangeKind::Fix => match payload.op.as_deref() {
            Some("remove") => vec![
                CardField::inline(
                    "Every",
                    was.map_or_else(|| "—".to_owned(), |was| format!("~~{was}~~")),
                ),
                CardField::inline(
                    "Change",
                    "**Stop scheduling weekly**\n-# this week's run is cancelled",
                ),
                party(),
            ],
            Some("edit") => {
                let from = was.map_or_else(|| "—".to_owned(), |was| format!("~~every {was}~~"));
                let to = weekly_slot(details).map_or_else(
                    || "unchanged".to_owned(),
                    |slot| {
                        format!(
                            "**every {slot}**\n-# every week from now on; this week's run moves with it"
                        )
                    },
                );
                let party = if payload.participants.is_empty() {
                    party()
                } else {
                    CardField::inline(
                        "Party",
                        format!(
                            "{} → {}",
                            names(&details.participants, who, "—"),
                            names(&payload.participants, who, "—")
                        ),
                    )
                };
                vec![
                    CardField::inline("From", from),
                    CardField::inline("To", to),
                    party,
                ]
            }
            _ => {
                let every = match weekly_slot(details) {
                    Some(slot) => format!("**every {slot}**"),
                    None => format!("{} and every week after", to_text(details, zone)),
                };
                vec![
                    CardField::inline(
                        "Every",
                        format!(
                            "{every}\n-# recurring, not a one-off; this week's run is added \
                             if that night is still ahead"
                        ),
                    ),
                    party(),
                ]
            }
        },
    }
}

/// `unchanged` ("run unchanged") is `None` once anything on the card applied.
fn closure_line(closure: &Closure, zone: Tz, unchanged: Option<&str>) -> String {
    let tail = unchanged
        .map(|text| format!(" · {text}"))
        .unwrap_or_default();
    match closure {
        Closure::Applied { by, at } => format!("✅ Applied by {by} at {}", local_time(*at, zone)),
        Closure::Rejected { by, at } => {
            format!("❌ Rejected by {by} at {}{tail}", local_time(*at, zone))
        }
        Closure::Superseded => format!("↪ Superseded by a newer card{tail}"),
        Closure::Stale => format!("⚠️ Out of date{tail}"),
    }
}

/// The footer word of a closed card: its first non-applied outcome.
fn closed_word(closures: &[Closure]) -> &'static str {
    closures
        .iter()
        .find_map(|closure| match closure {
            Closure::Applied { .. } => None,
            Closure::Rejected { .. } => Some("rejected"),
            Closure::Superseded => Some("superseded"),
            Closure::Stale => Some("out of date"),
        })
        .unwrap_or("closed")
}

/// One redesigned card for a burst. `confidence` is the lowest of the
/// changes'; `unanswered` is shown only while the card is open.
pub fn styled_card(
    details: &[&CardDetails],
    runs: &BTreeMap<String, &Run>,
    look: Look<'_>,
    unanswered: Option<&[String]>,
    confidence: Option<f64>,
    state: CardState<'_>,
) -> StyledCard {
    let run_of =
        |details: &CardDetails| details.run_id.as_ref().and_then(|id| runs.get(id).copied());
    let labels = |details: &CardDetails| {
        boss_labels(
            run_of(details).map_or(&details.bosses, |run| &run.bosses),
            look.catalog,
            look.marks,
        )
    };
    let (emoji, open_colour) = card_kind(details).mark();
    let tone = tone(&state);
    let mut ran: Vec<&Run> = Vec::new();
    for change in details {
        if let Some(run) = run_of(change)
            && !ran.iter().any(|seen| seen.id == run.id)
        {
            ran.push(run);
        }
    }

    let heading = match details {
        [one] => question(one, &labels(one), look.who),
        _ => format!("Apply {} changes?", details.len()),
    };
    let mut content = if tone == Tone::Closed {
        format!("{emoji} ~~**{heading}**~~")
    } else {
        format!("{emoji} **{heading}**")
    };
    if let [one] = details
        && let Some(summary) = filled(one.summary.as_deref())
    {
        content.push(' ');
        content.push_str(summary);
    }
    let applied = state
        .closures
        .iter()
        .any(|closure| matches!(closure, Closure::Applied { .. }));
    let unchanged = (!applied).then_some(match ran.len() {
        0 => "schedule unchanged",
        1 => "run unchanged",
        _ => "runs unchanged",
    });
    let lines = state
        .closures
        .iter()
        .map(|closure| closure_line(closure, look.zone, unchanged))
        .chain(state.notes.iter().cloned());
    for line in lines {
        content.push('\n');
        content.push_str(&subtext(&line));
    }

    let mut fields = Vec::new();
    for change in details {
        let run = run_of(change);
        if details.len() > 1 {
            let about: Vec<String> = filled(change.summary.as_deref())
                .map(str::to_owned)
                .into_iter()
                .chain(run.map(|run| tag(&run.id)))
                .collect();
            let value = if about.is_empty() {
                "\u{200b}".to_owned()
            } else {
                subtext(&about.join(" · "))
            };
            fields.push(CardField::wide(
                question(change, &labels(change), look.who),
                value,
            ));
        }
        fields.extend(group(change, run, look));
    }

    let mut description: Vec<String> = Vec::new();
    if tone == Tone::Open
        && let Some(waiting) = unanswered.filter(|waiting| !waiting.is_empty())
    {
        description.push(subtext(&format!(
            "Not yet answered: {}",
            names(waiting, look.who, "—")
        )));
    }
    let mut also: Vec<&str> = Vec::new();
    for kind in details.iter().flat_map(|change| &change.also_mentioned) {
        let spoken = kind_verb(kind);
        if !also.contains(&spoken) {
            also.push(spoken);
        }
    }
    if !also.is_empty() {
        description.push(subtext(&format!("Also mentioned: {}", also.join(", "))));
    }
    for line in details
        .iter()
        .filter_map(|change| change.self_service.as_ref())
    {
        if !description.contains(line) {
            description.push(line.clone());
        }
    }

    let mut footer: Vec<String> = ran.iter().map(|run| tag(&run.id)).collect();
    match tone {
        Tone::Open => {
            if let Some(confidence) = confidence {
                let percent = (confidence * 100.0).round().clamp(0.0, 100.0);
                footer.push(format!("{percent}% sure"));
            }
            footer.push(format!("{EMOJI_YES} apply"));
            footer.push(format!("{EMOJI_NO} reject"));
            footer.push("/amend to edit".to_owned());
        }
        Tone::Applied => footer.push("applied".to_owned()),
        Tone::Closed => footer.push(closed_word(state.closures).to_owned()),
    }

    StyledCard {
        content,
        description: (!description.is_empty()).then(|| description.join("\n")),
        fields,
        footer: footer.join(" · "),
        colour: match tone {
            Tone::Open => open_colour,
            Tone::Applied => SETTLED_GREEN,
            Tone::Closed => CLOSED_GREY,
        },
        mention_users: look
            .who
            .map(|who| who.mentioned.clone())
            .unwrap_or_default(),
    }
}
