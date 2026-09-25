//! In-memory `ProposalStore`: proposal facts beside the draft tables,
//! written in the same swap as the draft and its supersede closes.

use chrono::{DateTime, Utc};

use super::{MemoryScheduleStore, Tables, draft_event, micros};
use crate::domain::drafts::{
    DraftEventKind, DraftKind, DraftScope, DraftStatus, LoadedDraft, NewProposal, ProposalCreated,
    ProposalInfo, ProposalStore, SUPERSEDED, StoredDraft, StoredProposal, check_new_proposal,
};
use crate::domain::history::Actor;
use crate::domain::scheduler::StoreError;

/// Close a live draft as `actor` (no version bump, as expiry).
fn close(
    tables: &mut Tables,
    id: &str,
    status: DraftStatus,
    reason: Option<&str>,
    actor: &Actor,
    at: DateTime<Utc>,
) {
    let Some(draft) = tables.drafts.drafts.get_mut(id) else {
        return;
    };
    draft.status = status;
    draft.closed_by = Some(actor.clone());
    draft.close_reason = reason.map(str::to_owned);
    draft.updated_at = at;
    let version = draft.version;
    let kind = if status == DraftStatus::Expired {
        DraftEventKind::Expired
    } else {
        DraftEventKind::Discarded
    };
    draft_event(
        tables,
        id,
        version,
        kind,
        actor,
        at,
        reason.map(str::to_owned),
    );
}

impl ProposalStore for MemoryScheduleStore {
    async fn create_proposal(&self, new: NewProposal) -> Result<ProposalCreated, StoreError> {
        let expires_at = micros(check_new_proposal(&new)?);
        let at = micros(new.at);
        let mut tables = self.tables();
        if let Some(existing) = tables.drafts.drafts.get(&new.id) {
            return match tables.drafts.proposals.get(&new.id) {
                Some(info) if info.source == new.source && info.source_id == new.source_id => {
                    Ok(ProposalCreated::Replayed(existing.clone()))
                }
                _ => Err(StoreError::Constraint(format!("draft {} exists", new.id))),
            };
        }
        let superseded: Vec<String> = match &new.supersede_key {
            None => Vec::new(),
            Some(key) => tables
                .drafts
                .order
                .iter()
                .filter(|id| {
                    tables
                        .drafts
                        .proposals
                        .get(*id)
                        .and_then(|info| info.supersede_key.as_ref())
                        == Some(key)
                        && tables
                            .drafts
                            .drafts
                            .get(*id)
                            .is_some_and(|draft| draft.status.is_live())
                })
                .cloned()
                .collect(),
        };
        let stored = StoredDraft {
            id: new.id.clone(),
            kind: DraftKind::Proposal,
            title: new.title.clone(),
            author: new.author.clone(),
            base: new.base.clone(),
            base_revision: new.base_revision,
            version: 1,
            status: DraftStatus::Submitted,
            request_type: None,
            subject: new.subject.clone(),
            merged_seq: None,
            closed_by: None,
            close_reason: None,
            created_at: at,
            updated_at: at,
            scope: match new.expires_week {
                None => DraftScope::Weekly,
                Some(week) => DraftScope::Week(micros(week)),
            },
        };
        let mut next = tables.clone();
        next.drafts.drafts.insert(new.id.clone(), stored.clone());
        next.drafts.order.push(new.id.clone());
        next.drafts.proposals.insert(
            new.id.clone(),
            ProposalInfo {
                source: new.source,
                source_id: new.source_id.clone(),
                supersede_key: new.supersede_key.clone(),
                expires_at,
            },
        );
        let mut ops = new.ops.clone();
        for (position, staged) in ops.iter_mut().enumerate() {
            staged.ord = position;
        }
        next.drafts.ops.insert(new.id.clone(), ops);
        for kind in [DraftEventKind::Created, DraftEventKind::Submitted] {
            draft_event(&mut next, &new.id, 1, kind, &new.author, at, None);
        }
        for id in &superseded {
            close(
                &mut next,
                id,
                DraftStatus::Discarded,
                Some(SUPERSEDED),
                &new.author,
                at,
            );
        }
        *tables = next;
        Ok(ProposalCreated::Created {
            draft: stored,
            superseded,
        })
    }

    async fn load_proposal(
        &self,
        id: &str,
    ) -> Result<Option<(LoadedDraft, ProposalInfo)>, StoreError> {
        let tables = self.tables();
        let (Some(info), Some(draft)) = (
            tables.drafts.proposals.get(id),
            tables.drafts.drafts.get(id),
        ) else {
            return Ok(None);
        };
        Ok(Some((
            LoadedDraft {
                draft: draft.clone(),
                ops: tables.drafts.ops.get(id).cloned().unwrap_or_default(),
            },
            info.clone(),
        )))
    }

    async fn list_proposals(&self, live_only: bool) -> Result<Vec<StoredProposal>, StoreError> {
        let tables = self.tables();
        Ok(tables
            .drafts
            .order
            .iter()
            .filter_map(|id| {
                let info = tables.drafts.proposals.get(id)?;
                let draft = tables.drafts.drafts.get(id)?;
                (!live_only || draft.status.is_live()).then(|| StoredProposal {
                    draft: draft.clone(),
                    info: info.clone(),
                })
            })
            .collect())
    }

    async fn expire_proposals(
        &self,
        now: DateTime<Utc>,
        actor: &Actor,
    ) -> Result<Vec<String>, StoreError> {
        let now = micros(now);
        let mut tables = self.tables();
        let due: Vec<String> = tables
            .drafts
            .proposals
            .iter()
            .filter(|(id, info)| {
                info.expires_at <= now
                    && tables
                        .drafts
                        .drafts
                        .get(*id)
                        .is_some_and(|draft| draft.status.is_live())
            })
            .map(|(id, _)| id.clone())
            .collect();
        let mut next = tables.clone();
        for id in &due {
            close(&mut next, id, DraftStatus::Expired, None, actor, now);
        }
        *tables = next;
        Ok(due)
    }
}
