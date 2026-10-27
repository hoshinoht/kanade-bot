//! Pure write rules checked before SQLite writes; SQL and locking stay in
//! the store.

use chrono::{DateTime, Utc};

use kanade_domain::drafts::{DraftScope, DraftStatus, NewDraft, StoredDraft};
use kanade_domain::notify::DeclineNotice;
use kanade_domain::scheduler::StoreError;
use kanade_domain::settings::keys;

/// Nothing is written when any key is not an allowlisted setting.
pub(crate) fn refuse_unknown_settings(rows: &[(String, String)]) -> Result<(), StoreError> {
    match rows.iter().find(|(key, _)| !keys::is_setting(key)) {
        Some((key, _)) => Err(StoreError::Constraint(format!("{key:?} is not a setting"))),
        None => Ok(()),
    }
}

/// A decline-notice candidate needs a display name of at most 128 characters.
pub(crate) fn check_decline_candidate(candidate: &DeclineNotice) -> Result<(), StoreError> {
    let Some(display_name) = &candidate.display_name else {
        return Err(StoreError::Constraint(
            "decline notice candidate needs a display name".into(),
        ));
    };
    if display_name.chars().count() > 128 {
        return Err(StoreError::Constraint(
            "decline notice display name exceeds 128 characters".into(),
        ));
    }
    Ok(())
}

/// The stored row of a new draft, at version 1.
pub(crate) fn created_draft(
    new: &NewDraft,
    status: DraftStatus,
    expires_week: Option<DateTime<Utc>>,
) -> StoredDraft {
    StoredDraft {
        id: new.id.clone(),
        kind: new.kind,
        title: new.title.clone(),
        author: new.author.clone(),
        base: new.base.clone(),
        base_revision: new.base_revision,
        version: 1,
        status,
        request_type: new.request_type.clone(),
        subject: new.subject.clone(),
        merged_seq: None,
        closed_by: None,
        close_reason: None,
        created_at: new.at,
        updated_at: new.at,
        scope: match expires_week {
            None => DraftScope::Weekly,
            Some(week) => DraftScope::Week(week),
        },
    }
}
