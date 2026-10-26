//! Extraction proposal cards (slice E5): the card (`styled`, wording in
//! `crate::cards::format`) and its Components V2 layout (`v2`), posting and
//! refreshing through the delivery journal (`desk`), ✅/❌ and button
//! approval (`react`), the extract `Outbox` (`outbox`) and the
//! difficulty-pill application emojis (`emojis`).

mod desk;
pub mod emojis;
mod outbox;
mod react;
mod replay;
mod styled;
mod v2;

pub use desk::{Authority, CardDesk, CardSettings, DeskDeps};
pub use outbox::CardOutbox;
pub use react::{CardFollowUp, CardPress, CardReaction, FollowUpCard, Pressed};
pub use replay::ReplayLive;
pub use replay::{OFFLINE_CONFLICT_NOTICE, ReplayReport};
pub use styled::{CLOSED_GREY, CardState, Closure, Look, StyledCard, styled_card};
pub use v2::{APPLY, REJECT, card_components};
