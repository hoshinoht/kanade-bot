//! v4's informational card notes for weekly-timing proposals.

use chrono::{DateTime, Utc};

use crate::domain::schedule::{RunSource, SchedulePolicy, ScheduleSnapshot, utc_instant};

fn materialised(policy: &SchedulePolicy, now: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    policy
        .materialised_weeks(now)
        .ok()
        .and_then(|weeks| weeks.iter().map(utc_instant).collect::<Result<_, _>>().ok())
        .unwrap_or_default()
}

/// The live runs a weekly-timing edit pushes onto (v4 `apply_fixed_to_runs`'
/// count: one per materialised week, not done or cancelled).
pub fn live_timing_runs(
    snapshot: &ScheduleSnapshot,
    fixed_id: &str,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> usize {
    materialised(policy, now)
        .into_iter()
        .filter(|week| {
            snapshot.runs.iter().any(|run| {
                run.fixed_run_id.as_deref() == Some(fixed_id)
                    && run.week_start == *week
                    && !run.status.is_terminal()
            })
        })
        .count()
}

/// v4 `_note_adoption`: a materialised week whose run of the new timing was
/// not created by it (source other than `fixed`) adopted an existing run.
pub fn adoption_notes(
    snapshot: &ScheduleSnapshot,
    fixed_id: &str,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Vec<String> {
    ["this week", "next week", "the week after"]
        .into_iter()
        .zip(materialised(policy, now))
        .filter(|(_, week)| {
            snapshot.runs.iter().any(|run| {
                run.fixed_run_id.as_deref() == Some(fixed_id)
                    && run.week_start == *week
                    && run.source != RunSource::Fixed
            })
        })
        .map(|(label, _)| format!("adopted {label}'s run"))
        .collect()
}
