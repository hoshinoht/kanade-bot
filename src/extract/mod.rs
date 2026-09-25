//! Pure extraction rules ported from v4 `bot/extract`: no I/O, no clock reads,
//! no Discord, SQL or HTTP types.
//!
//! The keyword gate screens messages before any model call, windows cut
//! history into bursts, `resolve` turns the model's literal day/time text into
//! instants, `merge` folds a burst's amendments, and `matching` picks the run
//! each one is about. Frozen in `docs/v5/vectors/extract`.

mod amendment;
pub mod gate;
pub mod matching;
pub mod merge;
pub mod resolve;
mod text;
pub mod window;

pub use amendment::{Amendment, AmendmentKind};
