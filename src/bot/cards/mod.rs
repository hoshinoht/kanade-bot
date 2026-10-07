//! Extraction proposal cards (slice E5): v4's card text (`format`), the
//! redesigned card (`styled`), posting and refreshing through the delivery
//! journal (`desk`), ✅/❌ approval (`react`), the extract `Outbox`
//! (`outbox`) and the difficulty-pill application emojis (`emojis`).

mod desk;
pub mod emojis;
pub mod format;
mod outbox;
mod react;
mod replay;
mod styled;

pub use desk::{Authority, CardDesk, CardSettings, DeskDeps};
pub use format::{
    Audience, CardKind, CardView, SUPERSEDED_NOTICE, TBD, applied_notice, boss_label, boss_labels,
    card_kind, confirm_hint, format_participants, proposal_card, proposal_line, rejected_notice,
    unanswered, when_text,
};
pub use outbox::CardOutbox;
pub use react::{CardFollowUp, CardReaction, FollowUpCard};
pub use replay::ReplayLive;
pub use replay::{OFFLINE_CONFLICT_NOTICE, ReplayReport};
pub use styled::{CLOSED_GREY, CardState, Closure, Look, StyledCard, styled_card};
