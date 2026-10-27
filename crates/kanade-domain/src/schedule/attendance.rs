//! v5 attendance on the write path: the status recount
//! (`domain::attendance`).

use chrono::{DateTime, Utc};

use super::draft::Draft;
use super::rsvp::derive_run_status;

/// Re-derive every live, not-yet-started run's status under the draft's
/// attendance rules at `now` (the tick runs this so unknown answers make a
/// run at risk once its window opens, and assumed ones confirm it). Returns
/// the runs whose status changed.
pub fn recount_attendance(draft: &mut Draft, now: DateTime<Utc>) -> Vec<String> {
    let mut changed = Vec::new();
    for run in draft.runs(None) {
        if !run.status.is_live() || run.datetime <= now {
            continue;
        }
        let status = derive_run_status(draft, &run, run.status, now).status;
        if status != run.status {
            draft.set_run_status(&run.id, status);
            changed.push(run.id);
        }
    }
    changed
}
