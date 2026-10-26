//! The snapshot order every store returns (see [`kanade_domain::scheduler::ScheduleStore`]).

use kanade_domain::schedule::ScheduleSnapshot;

pub(crate) fn sort_snapshot(snapshot: &mut ScheduleSnapshot) {
    snapshot.sort();
}
