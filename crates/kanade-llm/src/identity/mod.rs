//! Roster data used to render raw model prompts. External routes are governed,
//! but caller-supplied names, identifiers, messages and URLs are not rewritten.

mod codec;
mod passthrough;

pub use codec::Member;
pub use passthrough::{Passthrough, PassthroughSession};
