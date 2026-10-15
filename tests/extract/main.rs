//! The extraction rules (`gate`, `window`, `merge`, `prompt`) and the
//! orchestration over pipeline-level fakes (`fakes`): `pipeline` and `rescan`
//! drive it, `claims` races them for one admission per message version.
//! `redirect` and `nudge` cover the v5 self-service redirect and its persona
//! nudges; `self_service` wires them through the governed rewrite and the
//! pipeline. `bounds` covers "after"/"before" moves and day-of-month dates.

mod bounds;
mod claims;
mod fakes;
mod gate;
mod merge;
mod nudge;
mod pipeline;
mod prompt;
mod redirect;
mod rescan;
mod self_service;
mod session;
mod stale_answers;
mod support;
mod usage;
mod window;

// The pinned test clock shared with the scheduler target.
#[path = "../common/mod.rs"]
mod common;
