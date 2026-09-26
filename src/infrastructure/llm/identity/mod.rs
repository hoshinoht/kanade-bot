//! Identity codec seam: extraction and chat route every identity-bearing
//! prompt fragment and model output through a session. `Passthrough` is what
//! runs; `PseudonymCodec` (fictional-name tokens) exists but is not wired yet.
//! An `external` route fails closed while passthrough is active unless the
//! operator override allows it. `scan/` refuses pseudonymized requests that
//! still carry a raw identity.

mod codec;
mod passthrough;
mod protect;
mod pseudonym;
mod route;
mod scan;
#[cfg(any(test, feature = "test-support"))]
mod tagging;

pub use codec::{
    CodecMode, DecodeError, IdentityCodec, IdentitySession, IssuedName, Member, MentionNames,
    RosterSource, ScanName, ScanNeedles,
};
pub use passthrough::{Passthrough, PassthroughSession};
pub use protect::{Protected, encode_protected};
pub use pseudonym::{
    BotIdentity, CodeLexicon, NamePool, PseudonymCodec, PseudonymConfig, PseudonymSession,
    SystemRng,
};
pub use route::{RouteRefused, check_routes, guard, open_session, unmasked};
pub use scan::{
    IdentityGrant, IdentityLeakBlocked, LeakFound, LeakKind, LeakScanner, ScanExemptions,
};
#[cfg(any(test, feature = "test-support"))]
pub use tagging::{TaggingCodec, TaggingSession, find_leaks, find_request_leaks};
