//! Equality for the run-bound chat tools, using persisted approval operations
//! rather than card prose or extraction bookkeeping.

use std::collections::BTreeSet;

use chrono::{DateTime, TimeDelta, Utc};

use crate::domain::drafts::{DraftOp, DraftStatus, LoadedDraft, NewProposal, ProposalInfo};
use crate::domain::proposals::{ChangeKind, ProposalSubject};

/// A chat turn may still be saving its card immediately after proposal creation.
pub const CARDLESS_CHAT_GRACE: TimeDelta = TimeDelta::minutes(2);

/// Return the existing proposal's channel when it is a live duplicate.
pub fn same_run_proposal(
    new: &NewProposal,
    existing: &LoadedDraft,
    info: &ProposalInfo,
    current_week: DateTime<Utc>,
) -> Option<String> {
    if existing.draft.status != DraftStatus::Submitted
        || info.expires_at <= new.at
        || existing
            .draft
            .scope
            .expires_week()
            .is_some_and(|week| week < current_week)
    {
        return None;
    }
    let subject = ProposalSubject::parse(new.subject.as_deref()?)?;
    let other = ProposalSubject::parse(existing.draft.subject.as_deref()?)?;
    if subject.run_id.is_none()
        || subject.run_id != other.run_id
        || subject.kind != other.kind
        || subject.channel_id.is_none()
        || subject.channel_id == other.channel_id
    {
        return None;
    }
    let ops: Vec<_> = new.ops.iter().map(|staged| &staged.op).collect();
    let old: Vec<_> = existing.ops.iter().map(|staged| &staged.op).collect();
    let same = match subject.kind {
        ChangeKind::Move => {
            let destination = |ops: &[&DraftOp]| {
                ops.iter().find_map(|op| match op {
                    DraftOp::AmendRun { to, .. } => Some(*to),
                    _ => None,
                })
            };
            destination(&ops).is_some() && destination(&ops) == destination(&old)
        }
        ChangeKind::Cancel => true,
        ChangeKind::Rsvp => {
            let answers = |ops: &[&DraftOp]| {
                ops.iter()
                    .filter_map(|op| match op {
                        DraftOp::SetRsvp { user_id, state, .. } => {
                            Some((user_id.clone(), state.as_str()))
                        }
                        _ => None,
                    })
                    .collect::<BTreeSet<_>>()
            };
            !answers(&ops).is_empty() && answers(&ops) == answers(&old)
        }
        // No other kind has a run-bound chat tool.
        _ => false,
    };
    same.then_some(other.channel_id).flatten()
}
