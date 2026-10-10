//! Scheduling rules ported from v4 `bot/agent/{materialise,rsvp}` and the
//! schedule mutations of `bot/api/service`, as a pure core.
//!
//! Planners read a [`ScheduleSnapshot`] through a [`Draft`] that reproduces v4
//! table semantics, and the draft's diff is the [`ChangeSet`] a store commits.
//! Clocks and ids are always passed in; nothing here reads the wall clock.

mod attendance;
mod draft;
mod error;
mod fixed_edit;
mod lifecycle;
mod materialise;
mod mutate;
mod notice;
mod op;
mod policy;
mod reminders;
mod roster;
mod rsvp;
mod run;
mod state;

pub use attendance::{
    record_attendance, recount_attendance, set_attendance_default, set_standing_answer,
};
pub use draft::Draft;
pub use error::{MemberRunRefusal, RUN_ENDED, ScheduleError};
pub use fixed_edit::{
    AmendedRun, AmendedRunChoice, FixedEdit, FixedEditChoices, FixedEditRequest, PartyDelta,
    apply_fixed_edit, apply_party_delta, party_delta, preview_fixed_edit,
};
pub use lifecycle::{
    RUN_DONE_AFTER, SettleRun, apply_fixed_to_runs, finish_run, is_past, is_past_slot, mark_done,
    retire_fixed_run, settle_run,
};
pub use materialise::{adoptable_run, materialise_week, materialise_weeks};
pub use mutate::{
    StatusChange, amend_run, reset_to_fixed, set_status, settable_status, swap_participants,
    swap_run_slots,
};
pub use notice::{DRAFT_MERGED, Notice, NoticeChange, Outcome, ROLLBACK_RESTORED, RequestDecision};
pub use op::{Op, OpResult, WeekStart, apply_op};
pub use policy::{SchedulePolicy, utc_instant};
pub use reminders::{
    COUNTDOWN_GRACE, COUNTDOWN_PREFIX, DAY_OF, DAY_OF_CLAMP, DAY_OF_GRACE, ReminderPolicy,
    ReminderSpec, countdown_kind, ensure_reminders, is_stale, reconcile_day_of,
    refresh_run_reminders, reminder_specs,
};
pub use roster::{RosterChange, RunState, run_state, validate_channel, validate_participants};
pub use rsvp::{
    EMOJI_NO, EMOJI_YES, ReactionResult, answer_states, apply_reaction, compute_status,
    derive_run_status, is_frozen, recompute_after_roster_change, state_for_emoji,
};
pub use run::{
    FixedField, FixedRun, FixedRunPatch, NewFixedRun, NewRun, Reminder, Rsvp, RsvpSource,
    RsvpState, Run, RunSource, RunStatus,
};
pub use state::{Change, ChangeSet, ScheduleSnapshot};
