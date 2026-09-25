//! Edit preconditions: what a direct edit saw of each field it changes, so a
//! stale screen cannot silently overwrite a newer change.
//!
//! A field's version is the last change that set it (the blame index); the
//! store compares each declared `(target, field)` with that index inside the
//! commit's transaction. Only declared fields are checked, so another field
//! of the same row changing since is not a conflict (field-level merge).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{DateTime, Utc};

use super::blame::{BlameTarget, FIXED_RUN_FIELDS, RUN_FIELDS};
use super::origin::Actor;
use super::record::{ChangeRef, RowKey, RowValue};

/// Marker notice kind of an administrator's "apply mine anyway" edit: the
/// record's `refs` are the changes it overrides.
pub const EDIT_OVERRIDE: &str = "notice.edit.override";

const RSVP_PREFIX: &str = "rsvp:";

/// The caller saw `field` of `target` last set by change `seen` (`None`:
/// no change recorded for it).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Precondition {
    pub target: BlameTarget,
    pub field: String,
    pub seen: Option<u64>,
}

impl Precondition {
    pub fn new(target: BlameTarget, field: impl Into<String>, seen: Option<u64>) -> Self {
        Self {
            target,
            field: field.into(),
            seen,
        }
    }

    /// The row holding the field's value: the RSVP row for `rsvp:<user>`,
    /// otherwise the target's own row.
    pub fn value_key(&self) -> RowKey {
        match (&self.target, self.field.strip_prefix(RSVP_PREFIX)) {
            (BlameTarget::Run(run_id), Some(user_id)) => RowKey::Rsvp {
                run_id: run_id.clone(),
                user_id: user_id.to_owned(),
            },
            _ => self.target_key(),
        }
    }

    /// The target's own row, whose absence means it was deleted.
    pub fn target_key(&self) -> RowKey {
        target_key(&self.target)
    }
}

/// A blame target's own row.
pub fn target_key(target: &BlameTarget) -> RowKey {
    match target {
        BlameTarget::Run(id) => RowKey::Run(id.clone()),
        BlameTarget::FixedRun(id) => RowKey::FixedRun(id.clone()),
    }
}

/// What a direct edit expects, plus (administrators only) the changes it
/// knowingly overrides. Empty: no precondition, behaviour as before.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expect {
    pub fields: Vec<Precondition>,
    /// Changes the edit overrides ("apply mine anyway"); each must be the
    /// `seen` change of a declared field the edit changes. Recorded as the
    /// change's `refs`.
    pub overrides: Vec<ChangeRef>,
}

impl Expect {
    pub fn fields(fields: impl IntoIterator<Item = Precondition>) -> Self {
        Self {
            fields: fields.into_iter().collect(),
            overrides: Vec::new(),
        }
    }

    #[must_use]
    pub fn overriding(mut self, overrides: impl IntoIterator<Item = ChangeRef>) -> Self {
        self.overrides = overrides.into_iter().collect();
        self
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty() && self.overrides.is_empty()
    }

    /// The same expectations in canonical order, so a retry declaring them
    /// in another order digests the same.
    #[must_use]
    pub fn canonical(&self) -> Self {
        let mut canonical = self.clone();
        canonical.fields.sort();
        canonical.overrides.sort();
        canonical
    }

    /// Checks needing no store: known fields, no duplicates, and overrides
    /// only by administrators and only of changes the caller declared seeing.
    ///
    /// # Errors
    /// [`PreconditionError`].
    pub fn validate(&self, actor: &Actor) -> Result<(), PreconditionError> {
        validate_fields(&self.fields)?;
        self.check_override_actor(actor)?;
        for reference in &self.overrides {
            if !self
                .fields
                .iter()
                .any(|field| field.seen == Some(reference.seq))
            {
                return Err(PreconditionError::OverrideNotSeen { seq: reference.seq });
            }
        }
        Ok(())
    }

    /// Overrides only by administrators (also checked by the store).
    ///
    /// # Errors
    /// [`PreconditionError::OverrideNotAdmin`].
    pub fn check_override_actor(&self, actor: &Actor) -> Result<(), PreconditionError> {
        if self.overrides.is_empty() || matches!(actor, Actor::Admin { .. }) {
            Ok(())
        } else {
            Err(PreconditionError::OverrideNotAdmin)
        }
    }

    /// Every override must be the `seen` change of a declared field this
    /// change set actually changes (`changed`: the record's
    /// [`changed_fields`](super::changed_fields)).
    ///
    /// # Errors
    /// [`PreconditionError::OverrideUnchanged`].
    pub fn check_overrides_changed(
        &self,
        changed: &BTreeSet<(BlameTarget, String)>,
    ) -> Result<(), PreconditionError> {
        for reference in &self.overrides {
            let covered = self.fields.iter().any(|field| {
                field.seen == Some(reference.seq)
                    && changed.contains(&(field.target.clone(), field.field.clone()))
            });
            if !covered {
                return Err(PreconditionError::OverrideUnchanged { seq: reference.seq });
            }
        }
        Ok(())
    }
}

/// Every field is one blame names for its target, declared once.
///
/// # Errors
/// [`PreconditionError::UnknownField`] or [`PreconditionError::DuplicateField`].
pub fn validate_fields(fields: &[Precondition]) -> Result<(), PreconditionError> {
    let mut seen = BTreeSet::new();
    for precondition in fields {
        let known = match &precondition.target {
            BlameTarget::Run(_) => {
                RUN_FIELDS.contains(&precondition.field.as_str())
                    || precondition.field == super::blame::STATUS_PIN_FIELD
                    || [RSVP_PREFIX, "attended:"].iter().any(|prefix| {
                        precondition
                            .field
                            .strip_prefix(prefix)
                            .is_some_and(|user| !user.is_empty())
                    })
            }
            BlameTarget::FixedRun(_) => {
                FIXED_RUN_FIELDS.contains(&precondition.field.as_str())
                    || precondition.field == super::blame::ATTENDANCE_DEFAULT_FIELD
                    || precondition
                        .field
                        .strip_prefix("standing:")
                        .is_some_and(|user| !user.is_empty())
            }
        };
        if !known {
            return Err(PreconditionError::UnknownField {
                target: precondition.target.clone(),
                field: precondition.field.clone(),
            });
        }
        if !seen.insert((&precondition.target, &precondition.field)) {
            return Err(PreconditionError::DuplicateField {
                target: precondition.target.clone(),
                field: precondition.field.clone(),
            });
        }
    }
    Ok(())
}

/// A declared precondition the edit could not be applied under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreconditionError {
    UnknownField {
        target: BlameTarget,
        field: String,
    },
    DuplicateField {
        target: BlameTarget,
        field: String,
    },
    /// The target never existed (no row and no recorded change).
    UnknownTarget {
        target: BlameTarget,
    },
    /// Only administrators may override newer changes.
    OverrideNotAdmin,
    /// An override names a change the caller did not declare seeing.
    OverrideNotSeen {
        seq: u64,
    },
    /// An override names a change whose declared field the edit does not
    /// change.
    OverrideUnchanged {
        seq: u64,
    },
    /// An override names no recorded change (or the wrong hash).
    UnknownOverride {
        seq: u64,
    },
}

impl fmt::Display for PreconditionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownField { target, field } => {
                write!(f, "{} has no field `{field}`", target.kind())
            }
            Self::DuplicateField { target, field } => {
                write!(f, "{} field `{field}` is declared twice", target.kind())
            }
            Self::UnknownTarget { target } => write!(f, "no {} `{}`", target.kind(), target.id()),
            Self::OverrideNotAdmin => f.write_str("only administrators may override changes"),
            Self::OverrideNotSeen { seq } => {
                write!(f, "change {seq} is overridden but not declared as seen")
            }
            Self::OverrideUnchanged { seq } => {
                write!(f, "change {seq} is overridden but its field is not changed")
            }
            Self::UnknownOverride { seq } => write!(f, "no recorded change {seq} to override"),
        }
    }
}

impl std::error::Error for PreconditionError {}

/// The last change that set a field, as a store reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LastChange {
    pub change: ChangeRef,
    pub actor: Actor,
    pub at: DateTime<Utc>,
}

/// One declared field that changed since the caller saw it (or whose row is
/// gone); the edit was not applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaleField {
    pub target: BlameTarget,
    pub field: String,
    pub expected: Option<u64>,
    /// The change that set the field last, if any.
    pub current: Option<LastChange>,
    /// The target row no longer exists.
    pub target_deleted: bool,
    /// The row holding the field now (the RSVP row for `rsvp:<user>`).
    pub current_value: Option<RowValue>,
}

/// What a store holds now for one precondition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldNow {
    pub last: Option<LastChange>,
    pub target_exists: bool,
    /// Some change ever set a field of the target.
    pub ever_recorded: bool,
    pub value: Option<RowValue>,
}

/// Compare one precondition with what the store holds now. A missing target
/// some change recorded was deleted (stale); one no change ever recorded
/// never existed.
///
/// # Errors
/// [`PreconditionError::UnknownTarget`].
pub fn check_field(
    precondition: &Precondition,
    now: FieldNow,
) -> Result<Option<StaleField>, PreconditionError> {
    if !now.target_exists && !now.ever_recorded {
        return Err(PreconditionError::UnknownTarget {
            target: precondition.target.clone(),
        });
    }
    let current_seq = now.last.as_ref().map(|last| last.change.seq);
    Ok(
        (current_seq != precondition.seen || !now.target_exists).then(|| StaleField {
            target: precondition.target.clone(),
            field: precondition.field.clone(),
            expected: precondition.seen,
            current: now.last,
            target_deleted: !now.target_exists,
            current_value: now.value,
        }),
    )
}

/// A run or weekly timing as a screen reads it: its row (and, for a run,
/// its RSVP rows) together with every field's version, from ONE read
/// transaction. Screens must take versions from here, never from a later
/// separate read, or a commit in between is overwritten silently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Versioned {
    pub target: BlameTarget,
    /// `None`: no such row now.
    pub row: Option<RowValue>,
    /// A run's RSVP rows in user order; empty for a weekly timing.
    pub answers: Vec<RowValue>,
    /// Field → last change seq (blame's index).
    pub versions: BTreeMap<String, u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(field: &str) -> Precondition {
        Precondition::new(BlameTarget::Run("r".into()), field, Some(3))
    }

    #[test]
    fn fields_must_be_blame_fields() {
        assert!(validate_fields(&[run("slot"), run("rsvp:1")]).is_ok());
        for field in ["rsvp:", "note", "reminders"] {
            assert!(matches!(
                validate_fields(&[run(field)]),
                Err(PreconditionError::UnknownField { .. })
            ));
        }
        assert!(matches!(
            validate_fields(&[run("slot"), run("slot")]),
            Err(PreconditionError::DuplicateField { .. })
        ));
        let fixed = Precondition::new(BlameTarget::FixedRun("f".into()), "note", None);
        assert!(validate_fields(&[fixed]).is_ok());
    }

    #[test]
    fn overrides_are_admin_only_seen_and_changed() {
        let reference = |seq| ChangeRef {
            seq,
            hash: "0".repeat(64),
        };
        let expect = Expect::fields([run("slot")]).overriding([reference(3)]);
        assert!(expect.validate(&Actor::admin("a")).is_ok());
        assert_eq!(
            expect.validate(&Actor::member("1")),
            Err(PreconditionError::OverrideNotAdmin)
        );
        assert_eq!(
            expect.validate(&Actor::system("delivery")),
            Err(PreconditionError::OverrideNotAdmin)
        );
        assert_eq!(
            Expect::fields([run("slot")])
                .overriding([reference(4)])
                .validate(&Actor::admin("a")),
            Err(PreconditionError::OverrideNotSeen { seq: 4 })
        );
        let changed =
            |field: &str| BTreeSet::from([(BlameTarget::Run("r".into()), field.to_owned())]);
        assert!(expect.check_overrides_changed(&changed("slot")).is_ok());
        assert_eq!(
            expect.check_overrides_changed(&changed("status")),
            Err(PreconditionError::OverrideUnchanged { seq: 3 })
        );
    }

    #[test]
    fn canonical_order_ignores_declaration_order() {
        let reference = |seq| ChangeRef {
            seq,
            hash: "0".repeat(64),
        };
        let a =
            Expect::fields([run("status"), run("slot")]).overriding([reference(3), reference(1)]);
        let b =
            Expect::fields([run("slot"), run("status")]).overriding([reference(1), reference(3)]);
        assert_ne!(a, b);
        assert_eq!(a.canonical(), b.canonical());
    }

    #[test]
    fn a_field_is_stale_when_its_last_change_moved_or_its_row_is_gone() {
        let last = |seq| LastChange {
            change: ChangeRef {
                seq,
                hash: "0".repeat(64),
            },
            actor: Actor::admin("a"),
            at: DateTime::UNIX_EPOCH,
        };
        let now = |last, target_exists, ever_recorded| FieldNow {
            last,
            target_exists,
            ever_recorded,
            value: None,
        };
        let check = |p: &Precondition, state| check_field(p, state);
        assert!(
            check(&run("slot"), now(Some(last(3)), true, true))
                .unwrap()
                .is_none()
        );
        assert!(
            check(&run("slot"), now(Some(last(4)), true, true))
                .unwrap()
                .is_some()
        );
        assert!(
            check(&run("slot"), now(None, true, false))
                .unwrap()
                .is_some()
        );
        let gone = check(&run("slot"), now(Some(last(3)), false, true))
            .unwrap()
            .unwrap();
        assert!(gone.target_deleted);
        assert_eq!(
            check(&run("slot"), now(None, false, false)),
            Err(PreconditionError::UnknownTarget {
                target: BlameTarget::Run("r".into())
            })
        );
        let fresh = Precondition::new(BlameTarget::Run("r".into()), "rsvp:1", None);
        assert!(check(&fresh, now(None, true, true)).unwrap().is_none());
        assert_eq!(
            fresh.value_key(),
            RowKey::Rsvp {
                run_id: "r".into(),
                user_id: "1".into()
            }
        );
    }
}
