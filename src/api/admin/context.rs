//! Shared plumbing of the admin reads (and the member reads on the public
//! origin): the state, week frames, the projection context and the
//! `?week=` parameter.

use std::{collections::BTreeMap, sync::Arc};

use axum::http::Uri;
use chrono::{DateTime, Utc};

use crate::{
    api::{
        auth::wire,
        dto::{
            Art, dow, hhmm,
            week::{Context, WeekFrame},
        },
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::{
        members::{MemberProfile, Roster},
        schedule::utc_instant,
        scheduler::StoreError,
    },
};

pub fn state(site: &Site) -> Result<&Arc<ApiState>, ApiError> {
    site.state.as_ref().ok_or(ApiError::UNAVAILABLE)
}

/// Backend text never reaches the client.
pub fn unavailable(_: StoreError) -> ApiError {
    ApiError::UNAVAILABLE
}

/// This and next boss week, from one clock read.
pub fn frames(state: &ApiState, now: DateTime<Utc>) -> Result<[WeekFrame; 2], ApiError> {
    let weeks = state
        .policy
        .materialised_weeks(now)
        .map_err(|_| ApiError::UNAVAILABLE)?;
    let reset = format!(
        "{} {}",
        dow(state.policy.reset_weekday),
        hhmm(state.policy.reset_time)
    );
    let start = |index: usize| -> Result<WeekFrame, ApiError> {
        Ok(WeekFrame {
            start: utc_instant(&weeks[index]).map_err(|_| ApiError::UNAVAILABLE)?,
            reset: reset.clone(),
        })
    };
    Ok([start(0)?, start(1)?])
}

/// `?week=this|next` (absent is `this`); anything else is refused.
pub fn next_week(uri: &Uri) -> Result<bool, ApiError> {
    let pairs = wire::query_pairs(uri.query());
    match wire::query_value(&pairs, "week").as_deref() {
        None | Some("this") => Ok(false),
        Some("next") => Ok(true),
        Some(_) => Err(ApiError::INVALID_QUERY),
    }
}

pub fn roster(profiles: &[MemberProfile]) -> Roster {
    let mut roster = Roster::new();
    for profile in profiles {
        roster.upsert(profile.member.clone());
    }
    roster
}

pub fn context<'a>(
    site: &'a Site,
    state: &'a ApiState,
    roster: Roster,
    now: DateTime<Utc>,
) -> Context<'a> {
    Context {
        catalog: &state.catalog,
        art: Art {
            root: site.boss_dir.as_deref(),
        },
        zone: state.policy.zone(),
        now,
        roster,
        channels: state
            .channels
            .channels()
            .into_iter()
            .map(|channel| (channel.id.clone(), channel))
            .collect::<BTreeMap<_, _>>(),
        guild_id: state.guild_id.as_deref(),
    }
}
