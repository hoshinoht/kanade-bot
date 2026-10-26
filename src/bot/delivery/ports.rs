//! Adapters that let one tick share its id source and clock reading with a
//! short-lived [`SchedulerService`](crate::domain::scheduler::SchedulerService);
//! the store is shared through [`StoreRef`](crate::domain::scheduler::StoreRef).

use chrono::{DateTime, Utc};

use crate::domain::scheduler::{Clock, IdSource};

/// A borrowed id source.
#[derive(Debug)]
pub struct IdsRef<'a, I>(pub &'a mut I);

impl<I: IdSource> IdSource for IdsRef<'_, I> {
    fn new_id(&mut self) -> String {
        self.0.new_id()
    }
}

/// The one clock reading a tick uses throughout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedClock(pub DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}
