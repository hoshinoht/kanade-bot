//! The member roster in serve: one sequential task applies every gateway
//! roster change and the reconciliation of each fresh session, over the
//! shared store pass and snapshot in `crate::discord::roster`.

mod task;

pub use task::{RosterJob, RosterTask};
