//! Config section saves beside the journal on a History page. They are
//! not in the hash chain, so they page by time: each page takes the saves
//! between its oldest record and the oldest record of the page before it.
//! Both come from the page's one snapshot ([`HistoryView`]).

use chrono::{DateTime, Duration, Utc};

use crate::{
    api::{
        dto::history::{SettingRowDiff, SettingsActor, SettingsChangeRow},
        error::ApiError,
        state::ApiState,
    },
    infrastructure::store::snapshots::HistoryView,
};
use kanade_domain::{
    history::{Actor, ChangeFilter},
    schedule::utc_instant,
    settings::{SettingsChange, SettingsChangeQuery},
    time::to_iso,
};

/// The filter's actor and boss week; `None` for a run's log, which settings
/// never touch.
fn scope(filter: &ChangeFilter) -> Option<(Option<Actor>, Option<DateTime<Utc>>)> {
    match filter {
        ChangeFilter::All => Some((None, None)),
        ChangeFilter::Week(week) => Some((None, Some(*week))),
        ChangeFilter::Actor(actor) => Some((Some(actor.clone()), None)),
        ChangeFilter::ActorInWeek(actor, week) => Some((Some(actor.clone()), Some(*week))),
        _ => None,
    }
}

/// The saves to read with the page; `None` reads none.
pub(super) fn query(filter: &ChangeFilter) -> Option<SettingsChangeQuery> {
    let (actor, week) = scope(filter)?;
    // A boss week is seven days (a DST shift moves it by hours at most); the
    // exact week of each save is checked by `page`.
    Some(SettingsChangeQuery {
        actor,
        from: week,
        until: week.map(|start| start + Duration::days(8)),
    })
}

/// The page's saves (newest first) and how many match the filter overall.
pub(super) fn page(
    state: &ApiState,
    filter: &ChangeFilter,
    before: Option<u64>,
    view: &HistoryView,
) -> Result<(Vec<SettingsChangeRow>, u64), ApiError> {
    let Some((_, week)) = scope(filter) else {
        return Ok((Vec::new(), 0));
    };
    let mut matching = Vec::new();
    for change in &view.settings {
        let start = week_of(state, &change.at)?;
        if week.is_none_or(|week| week == start) {
            matching.push((change, start));
        }
    }
    let total = matching.len() as u64;
    let upper = match (before, view.cursor_at) {
        (None, _) => None,
        (Some(_), Some(at)) => Some(at),
        // A cursor naming no record places no save, rather than repeating them.
        (Some(_), None) => return Ok((Vec::new(), total)),
    };
    let lower = view
        .next_before
        .and_then(|_| view.records.last().map(|record| record.at));
    let rows = matching
        .into_iter()
        .filter(|(change, _)| {
            lower.is_none_or(|lower| change.at >= lower)
                && upper.is_none_or(|upper| change.at < upper)
        })
        .map(|(change, start)| row(change, &start))
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
