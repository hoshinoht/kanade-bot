//! An optional hook told which runs each committed schedule change touched
//! (run rows and RSVPs), so posted cards can be refreshed from every write
//! path: reactions, commands, portal/admin API, chat and applied proposals.
//! Called after the commit, synchronously: it must be cheap and never block.

use std::fmt;
use std::sync::{Arc, OnceLock};

use crate::domain::schedule::{Change, ChangeSet};

pub type RunObserver = Arc<dyn Fn(&[String]) + Send + Sync>;

#[derive(Default)]
pub(crate) struct Observer(OnceLock<RunObserver>);

impl fmt::Debug for Observer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Observer")
            .field(&self.0.get().is_some())
            .finish()
    }
}

impl Observer {
    /// Install the hook; `false` if one is already set.
    pub(crate) fn set(&self, observer: RunObserver) -> bool {
        self.0.set(observer).is_ok()
    }

    /// Tell the hook about a written change; replays and no-ops are skipped
    /// by the caller.
    pub(crate) fn notify(&self, runs: &[String]) {
        if let Some(observer) = self.0.get()
            && !runs.is_empty()
        {
            observer(runs);
        }
    }
}

/// The runs a change set writes: run rows and their RSVPs.
pub(crate) fn touched_runs(changes: &ChangeSet) -> Vec<String> {
    let mut runs: Vec<String> = changes
        .changes
        .iter()
        .filter_map(|change| match change {
            Change::PutRun(run) => Some(run.id.clone()),
            Change::PutRsvp(rsvp) => Some(rsvp.run_id.clone()),
            Change::DeleteRsvp { run_id, .. } => Some(run_id.clone()),
            Change::PutFixedRun(_)
            | Change::DeleteFixedRun(_)
            | Change::PutReminder(_)
            | Change::DeleteReminder(_) => None,
        })
        .collect();
    runs.sort();
    runs.dedup();
    runs
}
