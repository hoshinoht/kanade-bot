//! Extraction ported from v4 `bot/extract`: pure rules plus the stateful
//! orchestration around them.
//!
//! The keyword gate screens messages before any model call, windows cut
//! history into bursts, `resolve` turns the model's literal day/time text into
//! instants, `merge` folds a burst's amendments, and `matching` picks the run
//! each one is about. `prompt` builds the request, `schema` validates and
//! retries the answer, and `plan` turns it into kept and dropped changes.
//! Frozen in `docs/v5/vectors/extract`. Those modules do no I/O and read no
//! clock.
//!
//! `pipeline` (debounced live bursts), `backlog` (late messages, drained at a
//! fixed rate) and `rescan` (queued re-reads of a window) drive them through
//! governed model sessions, the scheduler's proposal API and the model-log
//! store, behind ports with no Discord types.

mod amendment;
pub mod backlog;
pub mod gate;
pub mod matching;
pub mod merge;
pub mod pipeline;
pub mod plan;
pub mod prompt;
pub mod rescan;
pub mod resolve;
pub mod schema;
mod text;
pub mod window;

pub use amendment::{Amendment, AmendmentKind};
