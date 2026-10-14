//! Weekly fixed runs: validated against the catalog, then added through the
//! scheduler under their v4 ids, one attributed change per timing.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::read::V4Fixed;
use super::{Counts, ImportError, snowflake};
use crate::domain::catalog::BossTable;
use crate::domain::history::{Actor, Origin, Surface};
use crate::domain::ids::{IdGenerator, RandomIds};
use crate::domain::schedule::{NewFixedRun, SchedulePolicy};
use crate::domain::scheduler::{Clock, ScheduleStore, SchedulerError, SchedulerService, Scope};
use crate::domain::weeks::{parse_hhmm, weekday_from_index};
use crate::infrastructure::store::SqliteStore;

/// The history actor every imported change is attributed to.
pub fn actor() -> Actor {
    Actor::system("import")
}

fn request_id(id: &str) -> String {
    format!("v4-fixed:{id}")
}

pub struct Candidate {
    pub id: String,
    pub new: NewFixedRun,
}

/// A v4 row as a v5 timing, or the reason it is skipped.
pub fn validate(row: &V4Fixed, catalog: &BossTable) -> Result<Candidate, &'static str> {
    let id = row
        .id
        .as_deref()
        .filter(|id| {
            uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.hyphenated().to_string() == *id)
        })
        .ok_or("bad_id")?;
    let owner_id = row
        .owner_id
        .as_deref()
        .filter(|owner| snowflake(owner))
        .ok_or("bad_owner")?;
    let channel_id = match row.channel_id.as_deref().filter(|id| !id.is_empty()) {
        None => None,
        Some(channel) if snowflake(channel) => Some(channel.to_owned()),
        Some(_) => return Err("bad_channel"),
    };
    let bosses = strings(row.bosses.as_deref()).ok_or("bad_bosses")?;
    if bosses.is_empty() {
        return Err("bad_bosses");
    }
    let known = |token: &String| {
        catalog
            .split(token)
            .is_some_and(|(difficulty, boss)| &boss.canonical(difficulty.letter()) == token)
    };
    if !bosses.iter().all(known) {
        return Err("unknown_boss");
    }
    let weekday = row
        .weekday
        .filter(|day| (0..=6).contains(day))
        .and_then(|day| weekday_from_index(day).ok())
        .ok_or("bad_weekday")?;
    let time = row
        .time
        .as_deref()
        .and_then(|time| parse_hhmm(time).ok())
        .ok_or("bad_time")?;
    let listed = strings(row.participants.as_deref()).ok_or("bad_participants")?;
    let mut participants: Vec<String> = Vec::new();
    for member in listed {
        if !snowflake(&member) {
            return Err("bad_participants");
        }
        if !participants.contains(&member) {
            participants.push(member);
        }
    }
    if participants.is_empty() {
        return Err("no_participants");
    }
    Ok(Candidate {
        id: id.to_owned(),
        new: NewFixedRun {
            owner_id: owner_id.to_owned(),
            channel_id,
            bosses,
            weekday,
            time,
            participants,
            note: row
                .note
                .as_deref()
                .map(str::trim)
                .filter(|note| !note.is_empty())
                .map(str::to_owned),
            // v4 had no pin: its owner was always whoever created the run.
            owner_pinned: false,
        },
    })
}

/// A JSON array of strings (v4 `_dump`); integers are accepted as ids.
fn strings(text: Option<&str>) -> Option<Vec<String>> {
    let Value::Array(items) = serde_json::from_str(text?).ok()? else {
        return None;
    };
    items
        .into_iter()
        .map(|item| match item {
            Value::String(text) => Some(text),
            Value::Number(number) if number.is_u64() => Some(number.to_string()),
            _ => None,
        })
        .collect()
}

/// The import's single clock reading.
#[derive(Clone, Copy)]
pub struct Fixed(pub DateTime<Utc>);

impl Clock for Fixed {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// Hands out the v4 id for the timing being added (every time, so a
/// re-planned commit keeps it).
struct KeepId(String);

impl IdGenerator for KeepId {
    fn new_id(&mut self) -> String {
        self.0.clone()
    }
}

/// Count what is present and, when `apply`, add the rest. A timing already
/// in the store, or whose import request was recorded before (even if it
/// was since retired in v5), is never added again.
pub async fn import(
    store: Option<&Arc<SqliteStore>>,
    candidates: Vec<Candidate>,
    apply: bool,
    now: DateTime<Utc>,
    policy: &SchedulePolicy,
    counts: &mut Counts,
    skipped: &mut Vec<(String, &'static str)>,
) -> Result<(), ImportError> {
    let Some(store) = store else {
        counts.added += candidates.len() as u64;
        return Ok(());
    };
    let existing = store
        .load(&Scope::Weeks(Vec::new()))
        .await
        .map_err(stored)?;
    for candidate in candidates {
        let request = request_id(&candidate.id);
        let present = existing
            .fixed_runs
            .iter()
            .any(|fixed| fixed.id == candidate.id)
            || store
                .recorded_request(&actor(), &request)
                .await
                .map_err(stored)?
                .is_some();
        if present {
            counts.present += 1;
            continue;
        }
        if !apply {
            counts.added += 1;
            continue;
        }
        let mut service =
            SchedulerService::new(Arc::clone(store), KeepId(candidate.id.clone()), Fixed(now))
                .with_attendance(policy.attendance);
        let origin = Origin::new(actor(), Surface::Import).with_request_id(request);
        match service.as_origin(origin).add_fixed_run(candidate.new).await {
            Ok(_) => counts.added += 1,
            Err(SchedulerError::AlreadyApplied { .. }) => counts.present += 1,
            Err(SchedulerError::IdempotencyMismatch { .. }) => {
                counts.skip("changed_since_import");
                skipped.push((candidate.id, "changed_since_import"));
            }
            Err(SchedulerError::Schedule(_)) => {
                counts.skip("refused");
                skipped.push((candidate.id, "refused"));
            }
            Err(error) => return Err(ImportError::Store(error.to_string())),
        }
    }
    Ok(())
}

/// Materialise the current and next boss weeks as `serve` does; returns
/// how many runs were created (none when they already exist).
pub async fn materialise(
    store: &Arc<SqliteStore>,
    now: DateTime<Utc>,
    policy: &SchedulePolicy,
) -> Result<usize, ImportError> {
    let mut service = SchedulerService::new(Arc::clone(store), RandomIds, Fixed(now))
        .with_attendance(policy.attendance);
    service
        .as_origin(Origin::new(actor(), Surface::Import))
        .materialise_weeks(policy)
        .await
        .map(|created| created.len())
        .map_err(|error| ImportError::Store(error.to_string()))
}

fn stored(error: impl std::fmt::Display) -> ImportError {
    ImportError::Store(error.to_string())
}
