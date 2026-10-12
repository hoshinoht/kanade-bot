//! Draft request bodies and the staged operations they name. A `run` (or
//! `fixed`) is a stored id or `created:<n>`, the row the draft's operation
//! `n` creates.

use axum::http::Uri;
use chrono::Weekday;
use serde::Deserialize;
use std::collections::BTreeMap;

use super::super::write::{Refusal, SlotError, slot, strict_time};
use crate::{
    api::{auth::wire, dto::week::Context, error::ApiError},
    domain::{
        drafts::{DraftOp, Settle, Target},
        schedule::{
            FixedEdit, FixedEditChoices, RunStatus, SchedulePolicy, ScheduleSnapshot, StatusChange,
        },
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    pub title: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetitleBody {
    pub version: Option<u64>,
    pub title: String,
}

/// The query's only parameter `name`, if sent once with a value; any other
/// parameter (or a repeat) is `422 invalid_query`.
pub fn query_param(uri: &Uri, name: &str) -> Result<Option<String>, ApiError> {
    let pairs = wire::query_pairs(uri.query());
    let sent = uri.query().map_or(0, |query| {
        query.split('&').filter(|pair| !pair.is_empty()).count()
    });
    if pairs.len() != sent || pairs.len() > 1 || pairs.iter().any(|(key, _)| key != name) {
        return Err(ApiError::INVALID_QUERY);
    }
    match wire::query_value(&pairs, name) {
        Some(value) if !value.is_empty() => Ok(Some(value)),
        None => Ok(None),
        Some(_) => Err(ApiError::INVALID_QUERY),
    }
}

/// `?version=<n>` of a DELETE.
pub fn version_param(uri: &Uri) -> Result<Option<u64>, Refusal> {
    match query_param(uri, "version")? {
        None => Ok(None),
        Some(text) => text
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| text.parse().ok())
            .flatten()
            .map(Some)
            .ok_or_else(|| Refusal::from(ApiError::INVALID_QUERY)),
    }
}

/// `?from=<seq>&to=<seq>` of the graph read: both once, decimal, at most
/// the largest stored seq, `from <= to`; anything else is `422 invalid_query`.
pub fn seq_range(uri: &Uri) -> Result<(u64, u64), ApiError> {
    let pairs = wire::query_pairs(uri.query());
    let sent = uri.query().map_or(0, |query| {
        query.split('&').filter(|pair| !pair.is_empty()).count()
    });
    if pairs.len() != sent || pairs.iter().any(|(key, _)| key != "from" && key != "to") {
        return Err(ApiError::INVALID_QUERY);
    }
    let seq = |key: &str| {
        wire::query_value(&pairs, key)
            .filter(|text| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|text| text.parse::<u64>().ok())
            .filter(|seq| *seq <= super::super::history::MAX_SEQ)
            .ok_or(ApiError::INVALID_QUERY)
    };
    let (from, to) = (seq("from")?, seq("to")?);
    if from > to {
        return Err(ApiError::INVALID_QUERY);
    }
    Ok((from, to))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddOpBody {
    pub version: Option<u64>,
    pub op: OpBody,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RebaseBody {
    pub version: Option<u64>,
    /// Conflict key → `keep` | `take`.
    #[serde(default)]
    pub settle: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MergeBody {
    pub version: Option<u64>,
}

/// One change to stage, as the run sheet and the Fixed editor name them.
#[derive(Clone, Debug, Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpBody {
    /// `day` 0 = the run's reset day, inside the run's boss week.
    Move {
        run: String,
        day: u8,
        time: Option<String>,
    },
    Status {
        run: String,
        status: String,
    },
    Party {
        run: String,
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
    },
    Reset {
        run: String,
    },
    FixedParty {
        fixed: String,
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
    },
    /// A weekly timing's day (0 = Monday) and time; its live runs follow.
    FixedTime {
        fixed: String,
        day: u8,
        time: String,
    },
}

fn target(id: &str) -> Result<Target, Refusal> {
    match id.strip_prefix("created:") {
        Some(ord) => ord
            .parse()
            .map(Target::Created)
            .map_err(|_| Refusal::invalid("`created:<n>` names a staged change by number.")),
        None if id.is_empty() => Err(Refusal::invalid("Name the run or weekly timing.")),
        None => Ok(Target::Existing(id.to_owned())),
    }
}

const WEEKDAYS: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

impl OpBody {
    /// The operation to stage; a move is built in its run's boss week on
    /// `current`.
    pub fn op(
        &self,
        policy: &SchedulePolicy,
        ctx: &Context<'_>,
        current: &ScheduleSnapshot,
    ) -> Result<DraftOp, Refusal> {
        Ok(match self {
            Self::Move { run, day, time } => {
                if *day > 6 {
                    return Err(Refusal::invalid("A boss week has seven days."));
                }
                let Target::Existing(id) = target(run)? else {
                    return Err(Refusal::invalid(
                        "A run the draft adds moves by changing that change; remove and stage it again.",
                    ));
                };
                let found = current
                    .runs
                    .iter()
                    .find(|row| row.id == id)
                    .ok_or_else(|| Refusal::invalid("That run no longer exists."))?;
                let to = slot(policy, ctx, found, *day, time.as_deref()).map_err(|error| {
                    Refusal::invalid(match error {
                        SlotError::Time => "Times are HH:MM, 00:00 to 23:59.",
                        SlotError::NeedsTime => "A scheduled run needs a time.",
                        SlotError::Missing => "That time does not exist on that day.",
                        SlotError::Range => "That date is out of range.",
                        SlotError::OtherWeek => {
                            "That time falls in another boss week; runs stay in their week."
                        }
                    })
                })?;
                DraftOp::AmendRun {
                    run: Target::Existing(id),
                    to,
                }
            }
            Self::Status { run, status } => DraftOp::SetStatus {
                run: target(run)?,
                change: StatusChange {
                    status: RunStatus::parse(status).map_err(|_| {
                        Refusal::invalid(
                            "Pick one of planned, confirmed, own time, done or cancelled.",
                        )
                    })?,
                    announce: true,
                    via_portal: true,
                },
            },
            Self::Party { run, add, remove } => DraftOp::SwapParticipants {
                run: target(run)?,
                remove: remove.clone(),
                add: add.clone(),
                via_portal: true,
            },
            Self::Reset { run } => DraftOp::ResetToFixed { run: target(run)? },
            Self::FixedParty { fixed, add, remove } => DraftOp::FixedParticipants {
                fixed: target(fixed)?,
                add: add.clone(),
                remove: remove.clone(),
            },
            Self::FixedTime { fixed, day, time } => DraftOp::ApplyFixedEdit {
                fixed: target(fixed)?,
                edit: FixedEdit {
                    weekday: Some(
                        *WEEKDAYS.get(usize::from(*day)).ok_or_else(|| {
                            Refusal::invalid("A week has seven days (0 = Monday).")
                        })?,
                    ),
                    time: Some(
                        strict_time(time)
                            .ok_or_else(|| Refusal::invalid("Times are HH:MM, 00:00 to 23:59."))?,
                    ),
                    ..FixedEdit::default()
                },
                choices: FixedEditChoices::UpdateAll,
            },
        })
    }
}

/// `keep` | `take` per conflict key.
pub fn settle(given: &BTreeMap<String, String>) -> Result<BTreeMap<String, Settle>, Refusal> {
    given
        .iter()
        .map(|(key, answer)| {
            Settle::parse(answer)
                .map(|settle| (key.clone(), settle))
                .ok_or_else(|| Refusal::invalid("Settle each conflict with keep or take."))
        })
        .collect()
}
