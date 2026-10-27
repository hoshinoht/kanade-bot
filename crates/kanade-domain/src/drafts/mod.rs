//! Drafts: staged schedule operations, stored as versioned values, replayed
//! through [`apply_op`](crate::schedule::apply_op) for previews and
//! merged three-way into the current schedule. Pure; nothing here writes.

mod codec;
mod conflict;
mod expiry;
mod merge;
mod op;
mod port;
mod proposal;
mod replay;

pub use crate::schedule::party_delta;
pub use codec::{CodecError, DRAFT_OP_FORMAT, decode, encode};
pub use conflict::{Entity, Field, FieldValue, MergeConflict, Removal};
pub use expiry::expires_week;
#[cfg(any(test, feature = "test-support"))]
pub use merge::analyze_merge_since;
pub use merge::{MergeAnalysis, analyze_merge, analyze_merge_applying, replay_equivalent};
pub use op::{DraftOp, ReplayError, Target, resolve};
pub use port::{
    DraftChange, DraftCreated, DraftEvent, DraftEventKind, DraftKind, DraftRequest, DraftScope,
    DraftStale, DraftStatus, DraftStore, DraftUpdate, DraftWrite, LoadedDraft, MergeCommit,
    NewDraft, RequestLimit, RequestLimits, StagedOp, StoredDraft, Submission, closed_at,
};
pub use proposal::{
    DEFAULT_PROPOSAL_TTL, ExistingProposal, NewProposal, ProposalCreated, ProposalInfo,
    ProposalSource, ProposalStore, ProposalSubmission, SUPERSEDED, StoredProposal,
    check_new as check_new_proposal,
};
pub use replay::{
    PREVIEW_ID_PREFIX, PreviewIds, Rejected, Replay, StagedError, check_staged, replay,
    replay_real, resolve_staged,
};
