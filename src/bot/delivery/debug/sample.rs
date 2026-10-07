//! `/debug ping` without a run: an in-memory schedule holding one sample
//! run and its answers. Nothing here is ever written to a store.

use chrono::{DateTime, Utc};

use crate::bot::commands::SampleRun;
use crate::domain::notify::WeekReset;
use crate::domain::schedule::{
    Rsvp, RsvpSource, RsvpState, Run, RunSource, SchedulePolicy, ScheduleSnapshot,
};

/// The sample run's id (journalled with sandbox test cards only).
pub const SAMPLE_RUN_ID: &str = "sample";

/// The schedule a sample card renders from; `in`/`out` members join the
/// party, and a member in both counts as ✅.
pub fn sample_schedule(
    spec: &SampleRun,
    channel_id: &str,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<ScheduleSnapshot, String> {
    let reset = WeekReset {
        zone: policy.zone(),
        weekday: policy.reset_weekday,
        time: policy.reset_time,
    };
    let week_start = reset
        .current_week(spec.at)
        .map_err(|error| format!("sample week: {error}"))?;
    let mut party = spec.party.clone();
    for user in spec.yes.iter().chain(&spec.no) {
        if !party.contains(user) {
            party.push(user.clone());
        }
    }
    let answer = |user: &String, state| Rsvp {
        run_id: SAMPLE_RUN_ID.to_owned(),
        user_id: user.clone(),
        state,
        source: RsvpSource::Slash,
        at: now,
    };
    let mut rsvps: Vec<Rsvp> = spec
        .yes
        .iter()
        .map(|user| answer(user, RsvpState::Yes))
        .collect();
    rsvps.extend(
        spec.no
            .iter()
            .filter(|user| !spec.yes.contains(user))
            .map(|user| answer(user, RsvpState::No)),
    );
    Ok(ScheduleSnapshot {
        runs: vec![Run {
            id: SAMPLE_RUN_ID.to_owned(),
            fixed_run_id: None,
            channel_id: Some(channel_id.to_owned()),
            week_start,
            datetime: spec.at,
            bosses: spec.bosses.clone(),
            participants: party,
            status: spec.status,
            source: RunSource::Amend,
            attendance: Vec::new(),
            status_pin: None,
        }],
        rsvps,
        ..ScheduleSnapshot::default()
    })
}
