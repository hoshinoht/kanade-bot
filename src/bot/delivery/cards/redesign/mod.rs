//! The redesigned message style (`MessageStyle::Redesigned`): the shared
//! vocabulary (`vocab.rs`: boss labels, difficulty marks, Discord
//! timestamps, subtext, colour roles, the In/Waiting/Out roster) and the
//! day-of, countdown and digest cards built from it. Mentions and the
//! allow-list follow the same rules as the classic cards; only the layout
//! differs.

mod countdown;
mod day_of;
mod digest;
mod limits;
mod vocab;

pub use countdown::countdown_card;
pub use day_of::day_of_card;
pub use digest::{DIGEST_FOOTER, digest_card};
pub use limits::{MAX_EMBEDS, MAX_FIELD_VALUE, MAX_TITLE, MAX_TOTAL_CHARS};
pub use vocab::{
    AT_RISK_RED, DifficultyMarks, INK_BLUE, NO_MARKS, Roster, SETTLED_GREEN, StyleSource,
    WAITING_AMBER, boss_label, boss_labels, full_time, relative_time, roster, short_time, subtext,
};
