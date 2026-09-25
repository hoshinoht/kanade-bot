//! The tamper-evident schedule change history: who changed what, through
//! which surface, with every touched row's before/after value, hash-chained
//! so an edited or removed record is detectable. Reverts are planned here as
//! new changes; history is never rewritten.

mod blame;
mod checkpoint;
mod cherry_pick;
mod origin;
mod port;
mod precondition;
mod record;
mod revert;
mod rewind;

pub use blame::{
    ATTENDANCE_DEFAULT_FIELD, Attribution, Blame, BlameIndex, BlameLine, BlameTarget,
    FIXED_RUN_FIELDS, RUN_FIELDS, STATUS_PIN_FIELD, Via, attended_field, blame, changed_fields,
    rsvp_field, standing_field,
};
pub use checkpoint::{
    Checkpoint, CheckpointCreated, CheckpointKind, Checkpoints, MAX_CHECKPOINT_NAME, NewCheckpoint,
    auto_checkpoint_name, check_checkpoint,
};
pub use cherry_pick::{
    PickConflict, PickMode, PickPlan, PickRefusal, PickStep, PickValue, plan_pick,
};
pub use origin::{Actor, ChangeMeta, Origin, Surface};
pub use port::{
    BrokenLink, ChangeFilter, ChangeHistory, ChangePage, ChangeQuery, CheckedChange,
    HistoryRefusal, HistoryVerification, MAX_PAGE, StoredRecord, verify_chain,
};
pub use precondition::{
    EDIT_OVERRIDE, Expect, FieldNow, LastChange, Precondition, PreconditionError, StaleField,
    Versioned, check_field, target_key, validate_fields,
};
pub use record::{
    CHANGE_FORMAT, ChangeRecord, ChangeRef, GENESIS_PREV_HASH, RecordError, RowChange, RowKey,
    RowValue, sha256_hex,
};
pub use revert::{
    HeldReminders, JournalHeld, RevertMode, RevertOutcome, RevertScope, RowConflict, SkippedRow,
    apply_revert, changed_rows, changes_by_actor, changes_for_week,
};
pub use rewind::{HistoryGap, check_records_after, rewind};
