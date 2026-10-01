//! Extraction proposal cards (slice E5): v4's card text (`format`), posting
//! and refreshing through the delivery journal (`desk`), ✅/❌ approval
//! (`react`) and the extract `Outbox` (`outbox`).

mod desk;
pub mod format;
mod outbox;
mod react;

pub use desk::{Authority, CardDesk, CardSettings, DeskDeps};
pub use format::{
    Audience, CardKind, CardView, SUPERSEDED_NOTICE, TBD, applied_notice, boss_label, boss_labels,
    card_kind, confirm_hint, format_participants, proposal_card, proposal_line, rejected_notice,
    unanswered, when_text,
};
pub use outbox::CardOutbox;
pub use react::{CardFollowUp, CardReaction, FollowUpCard};
