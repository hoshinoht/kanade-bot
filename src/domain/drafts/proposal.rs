//! Proposals: drafts the extractor or chatbot stages for approval. They are
//! ordinary drafts (kind [`DraftKind::Proposal`](super::DraftKind::Proposal)) merged through
//! [`DraftStore::commit_merge`]; this port adds only what drafts lack — the
//! source, a supersede key and a TTL deadline — written with the draft.
//!
//! Approval authority and the TTL value are not decided here: the TTL is a
//! parameter of each proposal ([`DEFAULT_PROPOSAL_TTL`] until configured).

use std::future::Future;

use chrono::{DateTime, TimeDelta, Utc};

use super::port::{DraftStore, LoadedDraft, StagedOp, StoredDraft};
use crate::domain::history::{Actor, ChangeRef};
use crate::domain::scheduler::StoreError;

/// The TTL used until one is configured.
pub const DEFAULT_PROPOSAL_TTL: TimeDelta = TimeDelta::hours(24);

/// The `close_reason` of a proposal a newer one replaced.
pub const SUPERSEDED: &str = "superseded";

/// What staged a proposal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProposalSource {
    Extraction,
    Chat,
}

impl ProposalSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extraction => "extraction",
            Self::Chat => "chat",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Extraction, Self::Chat]
            .into_iter()
            .find(|source| source.as_str() == value)
    }
}

/// The proposal facts stored beside its draft; they never change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalInfo {
    pub source: ProposalSource,
    /// The extraction log or chat interaction id.
    pub source_id: String,
    /// Live proposals sharing a key replace each other (newest wins).
    pub supersede_key: Option<String>,
    /// `created_at + ttl`; a live proposal expires at or after it.
    pub expires_at: DateTime<Utc>,
}

/// A proposal to create, `submitted` with its operations. The author must be
/// a system actor (the extractor or chatbot component); it is also the actor
/// that closes superseded proposals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProposal {
    pub id: String,
    pub title: String,
    pub author: Actor,
    pub base: ChangeRef,
    pub base_revision: u64,
    pub subject: Option<String>,
    pub at: DateTime<Utc>,
    pub ops: Vec<StagedOp>,
    /// Derived as for drafts ([`expires_week`](super::expires_week)).
    pub expires_week: Option<DateTime<Utc>>,
    pub source: ProposalSource,
    pub source_id: String,
    pub supersede_key: Option<String>,
    /// Must be positive.
    pub ttl: TimeDelta,
}

/// The outcome of [`ProposalStore::create_proposal`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProposalCreated {
    /// Written, with the ids of the live proposals it superseded (closed
    /// `discarded`, reason [`SUPERSEDED`], in the same transaction).
    Created {
        draft: StoredDraft,
        superseded: Vec<String>,
    },
    /// The id already names this source's proposal; nothing written.
    Replayed(StoredDraft),
}

/// A proposal draft with its stored facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredProposal {
    pub draft: StoredDraft,
    pub info: ProposalInfo,
}

/// Proposal persistence on top of [`DraftStore`]; the plain
/// `create_draft` refuses the proposal kind.
pub trait ProposalStore: DraftStore {
    /// One write: create the proposal (status `submitted`, `created` and
    /// `submitted` events) and supersede the live proposals with the same
    /// key. An existing id is replayed when it is a proposal from the same
    /// source, else [`StoreError::Constraint`].
    fn create_proposal(
        &self,
        new: NewProposal,
    ) -> impl Future<Output = Result<ProposalCreated, StoreError>> + Send;

    fn load_proposal(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<(LoadedDraft, ProposalInfo)>, StoreError>> + Send;

    /// Proposals in creation order, all or only live ones.
    fn list_proposals(
        &self,
        live_only: bool,
    ) -> impl Future<Output = Result<Vec<StoredProposal>, StoreError>> + Send;

    /// Expire every live proposal whose `expires_at` is at or before `now`,
    /// as `actor`; returns their ids.
    fn expire_proposals(
        &self,
        now: DateTime<Utc>,
        actor: &Actor,
    ) -> impl Future<Output = Result<Vec<String>, StoreError>> + Send;
}

/// Refusals both stores apply before writing; returns `expires_at`.
pub fn check_new(new: &NewProposal) -> Result<DateTime<Utc>, StoreError> {
    if !matches!(new.author, Actor::System { .. }) {
        return Err(StoreError::Constraint(
            "a proposal's author is a system component".into(),
        ));
    }
    if new.ttl <= TimeDelta::zero() {
        return Err(StoreError::Constraint(
            "a proposal TTL must be positive".into(),
        ));
    }
    if new.title.is_empty() || new.title.chars().count() > 200 {
        return Err(StoreError::Constraint(
            "a proposal title is 1..=200 characters".into(),
        ));
    }
    new.at
        .checked_add_signed(new.ttl)
        .ok_or_else(|| StoreError::Constraint("a proposal TTL overflows".into()))
}
