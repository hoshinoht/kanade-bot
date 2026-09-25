//! `GET /api/admin/inbox`: live proposals and submitted member requests,
//! oldest first, each with the domain's preview against the current schedule.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::State,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};

use super::super::{
    context::{context, frames, roster},
    write::{Refusal, state, write_context},
};
use crate::{
    api::{
        auth::AdminSession,
        dto::{
            inbox::{self, Choice, Common, Evidence, message_url},
            iso_date,
            week::Context,
            when,
        },
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::{
        drafts::{DraftOp, LoadedDraft, Target},
        proposals::{ProposalSubject, StoredCard},
        schedule::{
            AmendedRunChoice, Draft, FixedEditChoices, ScheduleSnapshot, preview_fixed_edit,
        },
        scheduler::{DraftError, ProposalError, RequestError, Scope},
    },
};

fn unavailable<T>(_: T) -> Refusal {
    ApiError::UNAVAILABLE.into()
}

/// Discord's epoch in Unix milliseconds.
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

/// When Discord created the object with this snowflake id.
fn snowflake_time(id: &str) -> Option<DateTime<Utc>> {
    let id: u64 = id.parse().ok()?;
    DateTime::from_timestamp_millis(i64::try_from(id >> 22).ok()? + DISCORD_EPOCH_MS)
}

/// The card's evidence from the watched-message cache; a pruned or deleted
/// message is `missing`.
async fn evidence(
    state: &ApiState,
    ctx: &Context<'_>,
    card: Option<&StoredCard>,
) -> Result<Vec<Evidence>, Refusal> {
    let Some(card) = card else {
        return Ok(Vec::new());
    };
    let ids = &card.details.evidence_message_ids;
    let messages = match ids.iter().filter_map(|id| snowflake_time(id)).min() {
        Some(since) => state
            .store
            .messages(card.channel_id.clone(), since)
            .await
            .map_err(unavailable)?,
        None => Vec::new(),
    };
    Ok(ids
        .iter()
        .map(
            |id| match messages.iter().find(|message| &message.id == id) {
                Some(message) => Evidence {
                    id: id.clone(),
                    author: ctx.name(&message.author_id),
                    author_id: Some(message.author_id.clone()),
                    at: when(message.created_at, ctx.zone),
                    content: Some(message.content.clone()),
                    url: message_url(ctx, &message.channel_id, id),
                    missing: false,
                },
                None => Evidence {
                    id: id.clone(),
                    author: "someone".into(),
                    author_id: None,
                    at: snowflake_time(id).map_or_else(String::new, |at| when(at, ctx.zone)),
                    content: None,
                    url: None,
                    missing: true,
                },
            },
        )
        .collect())
}

/// A backend failure fails the list; anything else only blocks its item.
fn proposal_reason(error: &ProposalError) -> Result<String, Refusal> {
    match error {
        ProposalError::Draft(DraftError::Store(_) | DraftError::HistoryGap(_)) => {
            Err(ApiError::UNAVAILABLE.into())
        }
        other => Ok(other.to_string()),
    }
}

fn request_reason(error: &RequestError) -> Result<String, Refusal> {
    match error {
        RequestError::Draft(DraftError::Store(_) | DraftError::HistoryGap(_)) => {
            Err(ApiError::UNAVAILABLE.into())
        }
        other => Ok(other.to_string()),
    }
}

fn week_label(common: &Common<'_>, week: DateTime<Utc>) -> String {
    if week == common.frames[0].start {
        "This week".into()
    } else if week == common.frames[1].start {
        "Next week".into()
    } else {
        format!("Week of {}", iso_date(common.ctx.local_date(week)))
    }
}

/// A weekly-timing change's amended runs (each needs update or keep), and
/// the choices its listing preview assumes (update every one).
fn amended(
    common: &Common<'_>,
    loaded: &LoadedDraft,
    current: &ScheduleSnapshot,
) -> Option<(Vec<Choice>, FixedEditChoices)> {
    let (id, edit) = loaded.draft_ops().into_iter().find_map(|op| match op {
        DraftOp::ApplyFixedEdit {
            fixed: Target::Existing(id),
            edit,
            ..
        } => Some((id, edit)),
        _ => None,
    })?;
    let runs = preview_fixed_edit(
        &Draft::new(current.clone()),
        &id,
        &edit,
        common.policy,
        common.ctx.now,
    )
    .unwrap_or_default();
    let choices = runs
        .iter()
        .map(|run| Choice {
            run_id: run.run_id.clone(),
            label: week_label(common, run.week_start),
            when: when(run.datetime, common.ctx.zone),
            amended: true,
        })
        .collect();
    let assumed = runs
        .into_iter()
        .map(|run| (run.run_id, AmendedRunChoice::UpdateToFixed))
        .collect::<BTreeMap<_, _>>();
    Some((choices, FixedEditChoices::PerRun(assumed)))
}

pub async fn list(
    State(site): State<Arc<Site>>,
    session: AdminSession,
) -> Result<Response, Refusal> {
    let state = state(&site)?;
    let now = state.now();
    let frames = frames(state, now).map_err(Refusal::from)?;
    let (write_ctx, profiles) = write_context(state).await?;
    let current = state
        .store
        .snapshot(Scope::All)
        .await
        .map_err(unavailable)?;
    let ctx = context(&site, state, roster(&profiles), now);
    let common = Common {
        ctx: &ctx,
        policy: &state.policy,
        current: &current,
        frames: &frames,
    };
    let approver = session.discord_user();
    let mut items = Vec::new();

    let proposals = state.store.live_proposals().await.map_err(unavailable)?;
    let cards = state
        .store
        .cards(proposals.iter().map(|p| p.draft.id.clone()).collect())
        .await
        .map_err(unavailable)?;
    for stored in &proposals {
        let id = &stored.draft.id;
        let Some(loaded) = state.store.draft(id.clone()).await.map_err(unavailable)? else {
            continue;
        };
        let Some(subject) = loaded
            .draft
            .subject
            .as_deref()
            .and_then(ProposalSubject::parse)
        else {
            continue;
        };
        let card = cards.iter().find(|card| &card.proposal_id == id);
        let preview = state
            .writer
            .preview_proposal(id, approver, &write_ctx, now)
            .await;
        let preview = match &preview {
            Ok(preview) => Ok(preview),
            Err(error) => Err(proposal_reason(error)?),
        };
        let evidence = evidence(state, &ctx, card).await?;
        items.push(inbox::proposal(
            &common,
            &loaded,
            &stored.info,
            &subject,
            card,
            preview,
            evidence,
        ));
    }

    for loaded in state
        .store
        .submitted_requests()
        .await
        .map_err(unavailable)?
    {
        let (choices, assumed) = match amended(&common, &loaded, &current) {
            Some((choices, assumed)) => (Some(choices), Some(assumed)),
            None => (None, None),
        };
        let preview = state
            .writer
            .preview_request(&session.actor, &loaded.draft.id, assumed, &write_ctx, now)
            .await;
        let preview = match &preview {
            Ok(preview) => Ok(preview),
            Err(error) => Err(request_reason(error)?),
        };
        items.push(inbox::request(&common, &loaded, preview, choices));
    }
    items.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    Ok(Json(items).into_response())
}
