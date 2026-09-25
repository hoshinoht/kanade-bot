//! The member roster in serve: one sequential task applies every gateway
//! roster change and the startup reconciliation, and keeps an in-memory
//! snapshot the tick, cards and reactions read synchronously.

mod live;
mod reconcile;
mod task;

pub use live::LiveRoster;
pub use reconcile::{ReconcileError, ReconcileReport, diff, fetch_members};
pub use task::{RosterJob, RosterSink, RosterTask, prune_roles};
