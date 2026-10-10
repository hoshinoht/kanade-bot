//! A request as the form sends it, checked into the domain's `RequestSpec`.
//! Fields a kind does not take are `field_not_allowed`; anything malformed,
//! missing or outside the form's choices is `invalid_body`.

use std::collections::BTreeSet;

use axum::http::StatusCode;
use serde::Deserialize;

use super::super::write::invalid_body;
use crate::{
    api::admin::write::{Refusal, strict_time},
    domain::{
        catalog::BossTable,
        requests::{RequestSpec, RequestType, Subject, is_hidden_char},
        schedule::{FixedEdit, NewFixedRun},
        weeks::weekday_from_index,
    },
};

/// The longest note, in characters: it is stored as the request's title.
pub const MAX_NOTE: usize = 200;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api::public) struct RequestBody {
    kind: String,
    run_id: Option<String>,
    fixed_id: Option<String>,
    with: Option<String>,
    day: Option<u8>,
    time: Option<String>,
    channel_id: Option<String>,
    party: Option<Vec<String>>,
    bosses: Option<Vec<String>>,
    note: Option<String>,
}

/// What the form may name: the option lists' ids and the boss catalog.
pub(super) struct Choices<'a> {
    pub channels: BTreeSet<&'a str>,
    pub members: BTreeSet<&'a str>,
    pub catalog: &'a BossTable,
    /// A retry of a stored request: any id is taken, so the request digest
    /// alone decides replay or mismatch even after the options changed.
    pub lenient: bool,
}

pub(super) struct Checked {
    pub spec: RequestSpec,
    pub note: Option<String>,
}

fn not_allowed() -> Refusal {
    Refusal::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "field_not_allowed",
        "That field does not apply to this kind of request.",
    )
}

fn refuse_if(extra: bool) -> Result<(), Refusal> {
    if extra { Err(not_allowed()) } else { Ok(()) }
}

/// Trimmed; empty is none.
fn note(text: Option<String>) -> Result<Option<String>, Refusal> {
    let Some(text) = text else { return Ok(None) };
    let text = text.trim();
    if text.chars().count() > MAX_NOTE || text.chars().any(is_hidden_char) {
        return Err(invalid_body());
    }
    Ok((!text.is_empty()).then(|| text.to_owned()))
}

impl RequestBody {
    fn has_slot(&self) -> bool {
        self.day.is_some()
            || self.time.is_some()
            || self.channel_id.is_some()
            || self.party.is_some()
    }

    /// A run xor a weekly timing.
    fn subject(&mut self) -> Result<Subject, Refusal> {
        match (self.run_id.take(), self.fixed_id.take()) {
            (Some(run), None) => Ok(Subject::Run(run)),
            (None, Some(fixed)) => Ok(Subject::Fixed(fixed)),
            _ => Err(invalid_body()),
        }
    }
}

impl Choices<'_> {
    fn member(&self, id: String) -> Result<String, Refusal> {
        if self.lenient || self.members.contains(id.as_str()) {
            Ok(id)
        } else {
            Err(invalid_body())
        }
    }

    fn channel(&self, id: String) -> Result<String, Refusal> {
        if self.lenient || self.channels.contains(id.as_str()) {
            Ok(id)
        } else {
            Err(invalid_body())
        }
    }

    /// Roster members, in order, once each.
    fn party(&self, ids: Vec<String>) -> Result<Vec<String>, Refusal> {
        let mut party: Vec<String> = Vec::with_capacity(ids.len());
        for id in ids {
            let id = self.member(id)?;
            if !party.contains(&id) {
                party.push(id);
            }
        }
        Ok(party)
    }

    fn bosses(&self, tokens: Vec<String>) -> Result<Vec<String>, Refusal> {
        let mut bosses: Vec<String> = Vec::with_capacity(tokens.len());
        for token in tokens {
            let boss = self
                .catalog
                .parse_token(&token)
                .map_err(|_| invalid_body())?;
            if !bosses.contains(&boss) {
                bosses.push(boss);
            }
        }
        if bosses.is_empty() {
            return Err(invalid_body());
        }
        Ok(bosses)
    }
}

fn weekday(day: u8) -> Result<chrono::Weekday, Refusal> {
    weekday_from_index(i64::from(day)).map_err(|_| invalid_body())
}

fn time(text: &str) -> Result<chrono::NaiveTime, Refusal> {
    strict_time(text).ok_or_else(invalid_body)
}

/// `body` for `me`, against the form's choices.
pub(super) fn check(
    mut body: RequestBody,
    me: &str,
    choices: &Choices<'_>,
) -> Result<Checked, Refusal> {
    let kind = RequestType::parse(&body.kind).ok_or_else(invalid_body)?;
    let note = note(body.note.take())?;
    let spec = match kind {
        RequestType::Join | RequestType::Leave => {
            refuse_if(body.with.is_some() || body.has_slot() || body.bosses.is_some())?;
            let subject = body.subject()?;
            if kind == RequestType::Join {
                RequestSpec::Join(subject)
            } else {
                RequestSpec::Leave(subject)
            }
        }
        RequestType::Swap => {
            refuse_if(body.has_slot() || body.bosses.is_some())?;
            let subject = body.subject()?;
            let with = choices.member(body.with.ok_or_else(invalid_body)?)?;
            RequestSpec::Swap { subject, with }
        }
        RequestType::ChangeFixed => {
            refuse_if(body.run_id.is_some() || body.with.is_some() || body.bosses.is_some())?;
            if !body.has_slot() {
                return Err(invalid_body());
            }
            let fixed_id = body.fixed_id.ok_or_else(invalid_body)?;
            let participants = body.party.map(|ids| choices.party(ids)).transpose()?;
            if participants.as_ref().is_some_and(Vec::is_empty) {
                return Err(invalid_body());
            }
            RequestSpec::ChangeFixed {
                fixed_id,
                edit: FixedEdit {
                    weekday: body.day.map(weekday).transpose()?,
                    time: body.time.as_deref().map(time).transpose()?,
                    participants,
                    channel_id: body.channel_id.map(|id| choices.channel(id)).transpose()?,
                    ..FixedEdit::default()
                },
            }
        }
        RequestType::NewFixed => {
            refuse_if(body.run_id.is_some() || body.fixed_id.is_some() || body.with.is_some())?;
            let (Some(day), Some(at), Some(channel), Some(bosses)) =
                (body.day, body.time, body.channel_id, body.bosses)
            else {
                return Err(invalid_body());
            };
            // The requester owns it, so they lead the party.
            let mut party = vec![me.to_owned()];
            for id in choices.party(body.party.unwrap_or_default())? {
                if id != me {
                    party.push(id);
                }
            }
            RequestSpec::NewFixed(NewFixedRun {
                owner_id: me.to_owned(),
                channel_id: Some(choices.channel(channel)?),
                bosses: choices.bosses(bosses)?,
                weekday: weekday(day)?,
                time: time(&at)?,
                participants: party,
                note: None,
                owner_pinned: false,
            })
        }
    };
    Ok(Checked { spec, note })
}
