//! Checkpoints (tags): named pointers at the history head, created
//! automatically at each boss-week rollover or by an administrator. They are
//! immutable: never renamed, moved or deleted.

use std::future::Future;

use chrono::{DateTime, NaiveDate, Utc};

use super::origin::Actor;
use super::record::ChangeRef;
use crate::domain::scheduler::StoreError;

/// Who made a checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CheckpointKind {
    /// At a boss-week rollover; one per week.
    Auto,
    /// Named by an administrator.
    Admin,
}

impl CheckpointKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Admin => "admin",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

/// A stored checkpoint: the head (and store revision) when it was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub name: String,
    pub kind: CheckpointKind,
    /// The history head at creation; also an external-anchor candidate.
    pub head: ChangeRef,
    /// The store revision of that head record.
    pub revision: u64,
    /// The boss week it belongs to.
    pub week: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub created_by: Actor,
}

/// A checkpoint to create at the current head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewCheckpoint {
    pub name: String,
    pub kind: CheckpointKind,
    pub week: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub created_by: Actor,
}

/// The result of creating a checkpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckpointCreated {
    Created(Checkpoint),
    /// The week already has its automatic checkpoint; nothing was written.
    Existing(Checkpoint),
}

impl CheckpointCreated {
    pub fn checkpoint(&self) -> &Checkpoint {
        match self {
            Self::Created(checkpoint) | Self::Existing(checkpoint) => checkpoint,
        }
    }
}

pub const MAX_CHECKPOINT_NAME: usize = 100;

/// The automatic checkpoint's name for the boss week starting on `date`
/// (the guild-local date).
pub fn auto_checkpoint_name(date: NaiveDate) -> String {
    format!("week {date} start")
}

fn reserved(name: &str) -> bool {
    name.strip_prefix("week ")
        .and_then(|rest| rest.strip_suffix(" start"))
        .is_some_and(|date| {
            let parts: Vec<&str> = date.split('-').collect();
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| part.chars().all(|c| c.is_ascii_digit()))
        })
}

/// Names are trimmed, non-empty, at most [`MAX_CHECKPOINT_NAME`]
/// characters; administrators cannot take the automatic names.
///
/// # Errors
/// [`StoreError::Constraint`] describing the problem.
pub fn check_checkpoint(new: &NewCheckpoint) -> Result<(), StoreError> {
    let name = new.name.as_str();
    if name.trim() != name
        || name.is_empty()
        || name.chars().count() > MAX_CHECKPOINT_NAME
        || name.chars().any(char::is_control)
    {
        return Err(StoreError::Constraint(format!(
            "checkpoint names are 1..={MAX_CHECKPOINT_NAME} characters without outer spaces or control characters"
        )));
    }
    if new.kind == CheckpointKind::Admin && reserved(name) {
        return Err(StoreError::Constraint(format!(
            "checkpoint name {name:?} is reserved for automatic checkpoints"
        )));
    }
    Ok(())
}

/// Checkpoint storage. There is deliberately no way to change or remove one.
pub trait Checkpoints {
    /// Record a checkpoint at the current history head. An automatic
    /// checkpoint for a week that has one, or whose name an automatic
    /// checkpoint already has (the same guild-local week under another reset
    /// instant), returns that one as [`CheckpointCreated::Existing`].
    ///
    /// # Errors
    /// [`StoreError::Constraint`] for an invalid or taken name.
    fn create_checkpoint(
        &self,
        new: NewCheckpoint,
    ) -> impl Future<Output = Result<CheckpointCreated, StoreError>> + Send;

    fn load_checkpoint(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<Checkpoint>, StoreError>> + Send;

    /// Checkpoints in creation order, optionally of one boss week.
    fn list_checkpoints(
        &self,
        week: Option<DateTime<Utc>>,
    ) -> impl Future<Output = Result<Vec<Checkpoint>, StoreError>> + Send;
}
