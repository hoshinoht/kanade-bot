//! Config section saves beside the journal on a History page. They are
//! not in the hash chain, so they page by time: each page takes the saves
//! between its oldest record and the oldest record of the page before it.

use chrono::{DateTime, Duration, Utc};

use super::super::context::unavailable;
use crate::{
    api::{
        dto::history::{SettingRowDiff, SettingsActor, SettingsChangeRow},
        error::ApiError,
        state::{ApiState, HistorySlice},
    },
    domain::{
        history::{Actor, ChangeFilter},
        schedule::utc_instant,
        settings::{SettingsChange, SettingsChangeQuery},
        time::to_iso,
    },
};

/// The page's saves (newest first) and how many match the filter overall.
pub(super) async fn page(
    state: &ApiState,
    filter: &ChangeFilter,
    before: Option<u64>,
    slice: &HistorySlice,
) -> Result<(Vec<SettingsChangeRow>, u64), ApiError> {
    let (actor, week): (Option<Actor>, Option<DateTime<Utc>>) = match filter {
        ChangeFilter::All => (None, None),
        ChangeFilter::Week(week) => (None, Some(*week)),
        ChangeFilter::Actor(actor) => (Some(actor.clone()), None),
        ChangeFilter::ActorInWeek(actor, week) => (Some(actor.clone()), Some(*week)),
        // A run's log is its own rows; settings never touch a run.
        _ => return Ok((Vec::new(), 0)),
    };
    // A boss week is seven days (a DST shift moves it by hours at most); the
    // exact week of each save is checked below.
    let query = SettingsChangeQuery {
        actor,
        from: week,
        until: week.map(|start| start + Duration::days(8)),
    };
    let mut matching = Vec::new();
    for change in state
        .store
        .settings_changes(query)
        .await
        .map_err(unavailable)?
    {
        let start = week_of(state, &change.at)?;
        if week.is_none_or(|week| week == start) {
            matching.push((change, start));
        }
    }
    let total = matching.len() as u64;
    let upper = match before {
        None => None,
        Some(seq) => match state.store.change(seq).await.map_err(unavailable)? {
            Some(record) => Some(record.at),
            // A cursor naming no record places no save, rather than repeating them.
            None => return Ok((Vec::new(), total)),
        },
    };
    let lower = slice
        .next_before
        .and_then(|_| slice.records.last().map(|record| record.at));
    let rows = matching
        .into_iter()
        .filter(|(change, _)| {
            lower.is_none_or(|lower| change.at >= lower)
                && upper.is_none_or(|upper| change.at < upper)
        })
        .map(|(change, start)| row(&change, &start))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((rows, total))
}

fn week_of(state: &ApiState, at: &DateTime<Utc>) -> Result<DateTime<Utc>, ApiError> {
    let week = state
        .policy
        .week_of(at)
        .map_err(|_| ApiError::UNAVAILABLE)?;
    utc_instant(&week).map_err(|_| ApiError::UNAVAILABLE)
}

fn iso(at: &DateTime<Utc>) -> Result<String, ApiError> {
    to_iso(at).map_err(|_| ApiError::UNAVAILABLE)
}

fn row(change: &SettingsChange, week: &DateTime<Utc>) -> Result<SettingsChangeRow, ApiError> {
    Ok(SettingsChangeRow {
        id: change.id,
        at: iso(&change.at)?,
        actor: SettingsActor {
            kind: change.actor.kind(),
            id: change.actor.id().to_owned(),
        },
        surface: change.surface.as_str(),
        section: change.section.clone(),
        revision: change.revision,
        week: iso(week)?,
        values: change
            .values
            .iter()
            .map(|(key, diff)| SettingRowDiff {
                key: key.clone(),
                from: diff.from.clone(),
                to: diff.to.clone(),
            })
            .collect(),
    })
}
