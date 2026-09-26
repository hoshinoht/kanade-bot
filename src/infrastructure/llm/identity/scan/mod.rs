//! Provider-boundary identity-leak scanner. When pseudonymization is on,
//! every request a governed session is about to send is scanned for the raw
//! names, ids and snowflakes its identity session masks; a hit refuses the
//! request before anything is sent. Passthrough sessions are not scanned.

mod blocked;
mod exemptions;
mod finder;
mod scanner;

pub use blocked::{IdentityLeakBlocked, LeakFound, LeakKind};
pub use exemptions::ScanExemptions;
pub use scanner::{IdentityGrant, LeakScanner};
