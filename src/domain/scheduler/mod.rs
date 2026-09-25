//! The scheduler application service over narrow store, id and clock ports.

mod cherry_pick;
mod drafts;
mod ports;
mod requests;
mod service;

pub use cherry_pick::{PickError, PickPreview, PickResult, Picked};
pub use drafts::{
    DraftError, DraftExpiry, DraftResult, EditRefusal, MergeOutcome, MergeWarning, SkipReason,
};
pub use ports::{
    AttendanceHistory, Clock, Committed, IdSource, RecordedRequest, ScheduleStore, Scope,
    StoreError,
};
pub use requests::{Approved, Rejected, RequestError, RequestPreview, RequestResult};
pub use service::{Attributed, COMMIT_ATTEMPTS, SchedulerError, SchedulerResult, SchedulerService};
