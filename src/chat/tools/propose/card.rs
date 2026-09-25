//! What a chatbot proposal says: the card facts and the model's reply
//! material (v4 `tools/proposals.py`).

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::chat::tools::read::ToolWorld;
use crate::chat::tools::read::format::{boss_labels, weekday_name, when_label};
use crate::domain::ids::short_id;
use crate::domain::members::member_name;
use crate::domain::proposals::ChangeKind;
use crate::domain::schedule::{RsvpState, Run};
use crate::domain::weeks::weekday_from_index;

/// v4 `FIX_EDIT` / `FIX_REMOVE` payload markers.
pub const FIX_EDIT: &str = "edit";
pub const FIX_REMOVE: &str = "remove";

/// A staged proposal and what its card shows; posting it is the caller's
/// (card delivery goes through the delivery journal, not the tool).
#[derive(Clone, Debug, PartialEq)]
pub struct ProposalCard {
    pub proposal_id: String,
    pub kind: ChangeKind,
    /// The inbox's name for it (`move`, `new weekly`, `remove weekly`, …).
    pub kind_label: &'static str,
    pub run_id: Option<String>,
    /// Where the card is posted: the asking channel.
    pub channel_id: String,
    pub bosses: Vec<String>,
    /// The people the change names (a weekly edit: the party it has now).
    pub participants: Vec<String>,
    pub new_datetime: Option<DateTime<Utc>>,
    pub rsvp: Option<RsvpState>,
    /// v4's card payload (`op`, `fixed_run_id`, `weekly_when`, `weekday`,
    /// `time`, `participants`), in v4 key names.
    pub payload: Map<String, Value>,
    pub summary: String,
    /// The boss week the card is filed under.
    pub week_start: DateTime<Utc>,
    /// The card's day and time line.
    pub when: String,
    /// Display names, never mentions.
    pub party: String,
    pub evidence_message_ids: Vec<String>,
    /// Older live cards this one retired.
    pub superseded: Vec<String>,
}

/// v4 `KIND_VERB` / `FIX_VERB`.
pub fn kind_label(kind: ChangeKind, payload: &Map<String, Value>) -> &'static str {
    let op = payload.get("op").and_then(Value::as_str);
    match (kind, op) {
        (ChangeKind::Fix, Some(FIX_REMOVE)) => "remove weekly",
        (ChangeKind::Fix, Some(FIX_EDIT)) => "change weekly",
        (ChangeKind::Fix, _) => "new weekly",
        (ChangeKind::Move, _) => "move",
        (ChangeKind::Add, _) => "new run",
        (ChangeKind::Cancel, _) => "cancel",
        (ChangeKind::Split, _) => "split",
        (ChangeKind::Otot, _) => "own time",
        (ChangeKind::Sub, _) => "stand-in",
        (ChangeKind::Rsvp, _) => "answer",
    }
}

/// A party by display name, so the model never learns to ping.
pub fn names(world: &ToolWorld<'_>, user_ids: &[String]) -> String {
    let names: Vec<String> = user_ids
        .iter()
        .map(|uid| member_name(world.directory, uid))
        .collect();
    names.join(", ")
}

fn payload_slot(payload: &Map<String, Value>) -> Option<String> {
    let weekday = payload.get("weekday")?.as_i64()?;
    let time = payload.get("time")?.as_str().filter(|t| !t.is_empty())?;
    Some(format!(
        "{} {time}",
        weekday_name(weekday_from_index(weekday).ok()?)
    ))
}

/// A weekly edit's night, `every was → every is`.
fn changed_when(payload: &Map<String, Value>) -> String {
    let was = payload
        .get("weekly_when")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match payload_slot(payload) {
        Some(slot) => format!("every {was} → every {slot}"),
        None => format!("every {was} (same night)"),
    }
}

/// The day and time a card shows, in the card's own words.
pub fn card_when(
    world: &ToolWorld<'_>,
    kind: ChangeKind,
    at: Option<DateTime<Utc>>,
    run: Option<&Run>,
    payload: &Map<String, Value>,
) -> String {
    if kind == ChangeKind::Fix {
        if payload.get("op").and_then(Value::as_str) == Some(FIX_EDIT) {
            return changed_when(payload);
        }
        if let Some(weekly) = payload
            .get("weekly_when")
            .and_then(Value::as_str)
            .filter(|w| !w.is_empty())
        {
            return format!("every {weekly}");
        }
        if let Some(slot) = payload_slot(payload) {
            return format!("every {slot}");
        }
    }
    at.or(run.map(|run| run.datetime))
        .map(|when| when_label(&when, world.zone))
        .unwrap_or_default()
}

/// The party line: who is on it, and for a weekly edit who it becomes.
pub fn card_party(
    world: &ToolWorld<'_>,
    participants: &[String],
    run: Option<&Run>,
    payload: &Map<String, Value>,
) -> String {
    let people = if participants.is_empty() {
        run.map(|run| run.participants.clone()).unwrap_or_default()
    } else {
        participants.to_vec()
    };
    let mut party = names(world, &people);
    if party.is_empty() {
        party = "nobody yet".to_owned();
    }
    if payload.get("op").and_then(Value::as_str) == Some(FIX_EDIT) {
        let joining: Vec<String> = payload
            .get("participants")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|uid| uid.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if !joining.is_empty() {
            party = format!("{party} → {}", names(world, &joining));
        }
    }
    party
}

/// The tool's reply: facts for the model's own sentence, never a claim that
/// anything changed.
pub fn card_ready(card: &ProposalCard) -> String {
    let mut facts = vec![
        "Card ready -- facts for your reply, not a sentence to copy:".to_owned(),
        format!("- bosses: {}", boss_labels(&card.bosses)),
        format!("- party: {}", card.party),
        format!("- cards: {}", short_id(&card.proposal_id)),
    ];
    if !card.when.is_empty() {
        facts.insert(2, format!("- when: {}", card.when));
    }
    facts.push(
        "NOT DONE - nothing has changed yet: it takes effect only when somebody reacts ✅ on it."
            .to_owned(),
    );
    facts.push(
        "Reply in your own voice saying the card is up and needs a ✅, keeping each fact exact. Do not copy the labels or formatting above into your reply, do not start with \"Card posted:\", and do not stick an emoji on a flat sentence to sound in-character. The people named above are the whole party on it -- name those and nobody else, and never say you are on a run: you are a bot and cannot go to one. Never say it is done, moved, or confirmed."
            .to_owned(),
    );
    facts.join("\n")
}
