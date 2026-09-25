//! Identity codec seam (pseudonymization stage 1): extraction and chat route
//! every identity-bearing prompt fragment and model output through a session.
//! Only `Passthrough` ships; an `external` route fails closed while it is active.

mod codec;
mod passthrough;
mod route;
#[cfg(any(test, feature = "test-support"))]
mod tagging;

pub use codec::{CodecMode, DecodeError, IdentityCodec, IdentitySession, Member};
pub use passthrough::{Passthrough, PassthroughSession};
pub use route::{RouteRefused, check_routes, guard, open_session};
#[cfg(any(test, feature = "test-support"))]
pub use tagging::{TaggingCodec, TaggingSession, find_leaks, find_request_leaks};
