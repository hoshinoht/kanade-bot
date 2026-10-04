//! `inbox.json#/$defs/PastPage`: closed proposals and member requests for
//! the Inbox's read-only Past tab (`docs/v5/admin-api.md` "Inbox (A6)").
//! Closed drafts never change, so their last update is when they closed.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{
    Named,
    inbox::{Evidence, kind_label, message_url},
    iso_instant,
    week::Context,
};
use crate::{
    api::state::ClosedItem,
    domain::{
        drafts::{DraftStatus, SUPERSEDED},
        history::Actor,
        ids::short_id,
        members::Directory,
        proposals::{ProposalSubject, StoredCard},
        requests::RequestType,
    },
};

/// Who closed it; `name` is ready to show (system actors read as Kanade).
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PastDecider"))]
pub struct Decider {
    #[cfg_attr(test, ts(type = "ActorKind"))]
    pub kind: &'static str,
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PastItem {
    pub id: String,
    pub short_id: String,
    #[cfg_attr(test, ts(type = "ProposalKind | 'change'"))]
    pub kind: &'static str,
    pub kind_label: &'static str,
    /// `extractor` for proposals, `self_service` for member requests.
    #[cfg_attr(test, ts(type = "InboxTab"))]
    pub tab: &'static str,
    #[cfg_attr(test, ts(type = "ProposalSource"))]
    pub source: &'static str,
    /// The extraction log or chat interaction that staged a proposal.
    pub source_id: Option<String>,
    pub summary: String,
    pub channel: Option<String>,
    /// The member who asked (requests only).
    pub requester: Option<Named>,
    #[cfg_attr(test, ts(type = "PastOutcome"))]
    pub outcome: &'static str,
    pub decided_by: Option<Decider>,
    pub decided_at: String,
    pub reason: Option<String>,
    pub created_at: String,
    /// The History record an approval wrote.
    pub history_seq: Option<u64>,
    pub evidence: Vec<Evidence>,
    pub card_url: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PastPage {
    pub items: Vec<PastItem>,
    /// The `before` cursor of the next page; `None` on the last.
    pub next_before: Option<String>,
}

/// The outcome shown for a closed status; live statuses have none.
pub fn outcome(status: DraftStatus, reason: Option<&str>) -> Option<&'static str> {
    Some(match status {
        DraftStatus::Open | DraftStatus::Submitted => return None,
        DraftStatus::Merged => "approved",
        DraftStatus::Rejected => "rejected",
        DraftStatus::Discarded if reason == Some(SUPERSEDED) => "superseded",
        DraftStatus::Discarded => "discarded",
        DraftStatus::Withdrawn => "withdrawn",
        DraftStatus::Expired => "expired",
    })
}

/// As the History page names actors: a Discord admin by their member name
/// when the roster has them.
pub fn decider(ctx: &Context<'_>, actor: &Actor) -> Decider {
    let name = match actor {
        Actor::Member { id } => ctx.name(id),
        Actor::Admin { id } if id == "token" => "Admin (token)".into(),
        Actor::Admin { id } => match id.split_once(':') {
            Some(("discord", user)) if ctx.roster.member(user).is_some() => ctx.name(user),
            Some(("discord", user)) => format!("Admin (Discord {user})"),
            Some(("tailscale", login)) => format!("Admin ({login})"),
            _ => format!("Admin ({id})"),
        },
        Actor::System { .. } => "Kanade".into(),
    };
    Decider {
        kind: actor.kind(),
        id: actor.id().to_owned(),
        name,
    }
}

/// One closed item; `evidence` is already resolved for a proposal's card.
pub fn item(
    ctx: &Context<'_>,
    closed: &ClosedItem,
    card: Option<&StoredCard>,
    evidence: Vec<Evidence>,
) -> Option<PastItem> {
    let draft = &closed.draft;
    let reason = draft.close_reason.as_deref();
    let outcome = outcome(draft.status, reason)?;
    let (kind, tab, source, source_id, summary, channel, requester) = match &closed.proposal {
        Some(info) => {
            let subject = draft.subject.as_deref().and_then(ProposalSubject::parse);
            let channel = card
                .map(|card| card.channel_id.clone())
                .or_else(|| subject.as_ref().and_then(|s| s.channel_id.clone()));
            (
                subject.map_or("change", |s| s.kind.as_str()),
                "extractor",
                info.source.as_str(),
                Some(info.source_id.clone()),
                card.and_then(|card| card.details.summary.clone())
                    .unwrap_or_else(|| draft.title.clone()),
                channel,
                None,
            )
        }
        None => (
            draft
                .request_type
                .as_deref()
                .and_then(RequestType::parse)
                .map_or("change", RequestType::as_str),
            "self_service",
            "self_service",
            None,
            draft.title.clone(),
            None,
            Some(ctx.named(draft.author.id())),
        ),
    };
    Some(PastItem {
        id: draft.id.clone(),
        short_id: short_id(&draft.id),
        kind,
        kind_label: kind_label(kind),
        tab,
        source,
        source_id,
        summary,
        channel: channel.map(|id| ctx.channel_name(&id)),
        requester,
        outcome,
        decided_by: draft.closed_by.as_ref().map(|actor| decider(ctx, actor)),
        decided_at: iso_instant(draft.updated_at),
        reason: reason
            .filter(|_| outcome != "superseded")
            .map(str::to_owned),
        created_at: iso_instant(draft.created_at),
        history_seq: draft.merged_seq,
        evidence,
        card_url: card
            .and_then(|card| message_url(ctx, &card.channel_id, card.message_id.as_deref()?)),
    })
}

/// Newest closed first, ties by id (descending), so the order is stable.
pub fn sort_key(closed: &ClosedItem) -> (DateTime<Utc>, &str) {
    (closed.draft.updated_at, closed.draft.id.as_str())
}
