//! The snapshot order every store returns (see [`crate::domain::scheduler::ScheduleStore`]).

use crate::domain::schedule::ScheduleSnapshot;

pub(crate) fn sort_snapshot(snapshot: &mut ScheduleSnapshot) {
    snapshot.sort();
}
