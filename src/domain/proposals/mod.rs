//! Proposals from the extractor and chatbot (v4 `bot/extract/commit.py`):
//! the change a proposal stages, who may approve it, and why it cannot
//! apply. Pure; the scheduler service proposes, approves and merges them.

mod authority;
mod card;
mod change;
mod notes;
mod refusal;
mod translate;

pub use authority::{Approver, may_commit};
pub use card::{CardDetails, CardPayload, ProposalCardStore, StoredCard};
pub use change::{ChangeKind, Payload, ProposalSubject, ProposedChange};
pub use notes::{adoption_notes, live_timing_runs};
pub use refusal::Refusal;
pub use translate::{CREATED_FROM_CHAT, Translation, fill_approver, translate};
