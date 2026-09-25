//! API-1: the week `version` the PWA read becomes per-field preconditions.
//!
//! A field last changed at or before `version` is declared with that change
//! as `seen`, so the store re-checks it inside the commit (a change landing
//! between this read and the commit is still refused). A field changed after
//! `version` is a conflict, unless the request carries an Idempotency-Key:
//! then `seen` is what the client saw at `version` (history is append-only,
//! so a retry declares exactly what the first attempt did and replays).

use serde::Deserialize;

use super::refusal::Refusal;
use crate::{
    api::state::ReadStore,
    domain::history::{BlameTarget, ChangeRef, Expect, Precondition},
};

/// Optional explicit preconditions (admin-api "Edit preconditions"): used
/// instead of the version-derived ones when present.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Explicit {
    #[serde(default)]
    pub expect: Option<Vec<SeenField>>,
    #[serde(default, rename = "override")]
    pub overrides: Option<Vec<OverrideRef>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeenField {
    pub field: String,
    pub seen: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverrideRef {
    pub seq: u64,
    pub hash: String,
}

pub async fn expectations(
    store: &dyn ReadStore,
    target: BlameTarget,
    fields: &[String],
    version: Option<u64>,
    explicit: &Explicit,
    keyed: bool,
) -> Result<Expect, Refusal> {
    let overrides: Vec<ChangeRef> = explicit
        .overrides
        .iter()
        .flatten()
        .map(|reference| ChangeRef {
            seq: reference.seq,
            hash: reference.hash.clone(),
        })
        .collect();
    if let Some(seen) = &explicit.expect {
        let fields = seen
            .iter()
            .map(|field| Precondition::new(target.clone(), field.field.clone(), field.seen));
        return Ok(Expect::fields(fields).overriding(overrides));
    }
    let Some(version) = version else {
        return Ok(Expect::default().overriding(overrides));
    };
    let last = store
        .last_changes(target.clone())
        .await
        .map_err(|_| Refusal::from(crate::api::error::ApiError::UNAVAILABLE))?;
    let mut declared = Vec::new();
    for field in fields {
        let current = last.get(field).copied();
        let seen = match current {
            Some(seq) if seq > version => {
                if !keyed {
                    return Err(Refusal::stale());
                }
                store
                    .seen_at(target.clone(), field.clone(), version)
                    .await
                    .map_err(|_| Refusal::from(crate::api::error::ApiError::UNAVAILABLE))?
            }
            current => current,
        };
        declared.push(Precondition::new(target.clone(), field.clone(), seen));
    }
    Ok(Expect::fields(declared).overriding(overrides))
}
