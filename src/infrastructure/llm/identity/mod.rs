//! Identity codec seam: extraction and chat route every identity-bearing
//! prompt fragment and model output through a session. `Passthrough` is what
//! runs; `PseudonymCodec` (fictional-name tokens) exists but is not wired yet.
//! An `external` route fails closed while passthrough is active unless the
//! operator override allows it.

mod codec;
mod passthrough;
mod pseudonym;
mod route;
#[cfg(any(test, feature = "test-support"))]
mod tagging;

pub use codec::{
    CodecMode, DecodeError, IdentityCodec, IdentitySession, Member, ScanName, ScanNeedles,
};
pub use passthrough::{Passthrough, PassthroughSession};
pub use pseudonym::{
    BotIdentity, CodeLexicon, NamePool, PseudonymCodec, PseudonymConfig, PseudonymSession,
    SystemRng,
};
pub use route::{RouteRefused, check_routes, guard, open_session, unmasked};
#[cfg(any(test, feature = "test-support"))]
pub use tagging::{TaggingCodec, TaggingSession, find_leaks, find_request_leaks};
