//! The scheduler application service over narrow store, id and clock ports.

mod cherry_pick;
mod drafts;
mod ports;
mod proposal_lookup;
mod proposals;
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
pub use proposal_lookup::{CARDLESS_CHAT_GRACE, same_run_proposal};
pub use proposals::{
    ChatProposed, ProposalApproved, ProposalError, ProposalPreview, ProposalRequest,
    ProposalResult, Proposed, Supersede, SupersedeScope,
};
pub use requests::{Approved, Rejected, RequestError, RequestPreview, RequestResult};
pub use service::{
    Attributed, COMMIT_ATTEMPTS, DeclineNoticeContext, DeclineRsvpResult, SchedulerError,
    SchedulerResult, SchedulerService,
};
