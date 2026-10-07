//! The redesigned message style (`MessageStyle::Redesigned`): the shared
//! vocabulary (`vocab.rs`: boss labels, difficulty marks, Discord
//! timestamps, subtext, colour roles, the In/Waiting/Out roster), the
//! day-of, countdown and digest cards built from it, the change and
//! decline notices (`notice.rs`) and the `/schedule` reply
//! (`schedule.rs`). Mentions and the allow-list follow the same rules as
//! the classic style; only the layout differs. Messages that ping nobody
//! (digest, `/schedule`, proposal cards) also have a Components V2 layout
//! (`v2.rs` builders and budget, `live.rs` the kit and which posts are V2).

mod countdown;
mod day_of;
mod digest;
mod limits;
mod live;
mod notice;
mod schedule;
mod v2;
mod vocab;

pub use countdown::countdown_card;
pub use day_of::day_of_card;
pub use digest::{DIGEST_FOOTER, MY_RUNS, OPEN_PORTAL, digest_card};
pub use limits::{MAX_EMBEDS, MAX_FIELD_VALUE, MAX_TITLE, MAX_TOTAL_CHARS};
pub use live::{AvatarSource, LiveFormats, V2Kit, known_or_fetched, learn_after_refusal};
pub use notice::{NoticeLook, decline_text, notice_text};
pub use schedule::{
    SCHEDULE_FOOTER, SCHEDULE_FOOTER_HIDDEN, ScheduleScope, ScheduleWeek, schedule_components,
    schedule_embed,
};
pub use v2::{
    ButtonId, CARD_CLOSED, DIGEST_MINE, MAX_COMPONENTS, MAX_TEXT_CHARS, action_row, button,
    component_count, container, separator, text, text_chars, within_budget,
};
pub use vocab::{
    AT_RISK_RED, DifficultyMarks, INK_BLUE, NO_MARKS, Roster, SETTLED_GREEN, StyleSource,
    WAITING_AMBER, boss_label, boss_labels, full_time, relative_time, roster, short_time, subtext,
};
