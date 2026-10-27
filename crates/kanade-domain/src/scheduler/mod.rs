//! The scheduler application service over narrow store, id and clock ports.

mod drafts;
mod ports;
mod proposal_lookup;
mod proposals;
mod requests;
mod service;
mod store_ref;

pub use drafts::{DraftError, DraftExpiry, DraftResult, MergeOutcome, MergeWarning, SkipReason};
pub use ports::{Clock, Committed, IdSource, RecordedRequest, ScheduleStore, Scope, StoreError};
pub use proposal_lookup::{CARDLESS_CHAT_GRACE, same_run_proposal};
pub use proposals::{
    ChatProposed, ProposalApproved, ProposalError, ProposalPreview, ProposalRequest,
    ProposalResult, Proposed, Supersede, SupersedeScope,
};
pub use requests::{Approved, Rejected, RequestError, RequestPreview, RequestResult};
pub use service::{
    Attributed, COMMIT_ATTEMPTS, DeclineNoticeContext, DeclineRsvpResult, RollbackRequest,
    RollbackSelection, SchedulerError, SchedulerResult, SchedulerService,
};
pub use store_ref::StoreRef;
