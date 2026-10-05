//! `GET /api/admin/reminders/{id}/preview`: one reminder's card as the bot
//! posts it (unsent: the tick's planner) or edits it (posted: the refresh
//! worker's content), through the delivery card builder
//! (`bot::delivery::preview`). A row retired without posting has no card. Read only: no record is written, no heading
//! is generated, no art is read and nothing goes to Discord.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path as UrlPath, State, rejection::PathRejection},
    response::{IntoResponse, Response},
    routing::get,
};

use super::context::{context, frames, roster, state, unavailable};
use crate::{
    api::{
        auth::AdminSession,
        dto::{
            iso_instant,
            reminders::{CardField, CardPreview, ReminderPreview, ReminderState, classified, row},
        },
        error::ApiError,
        listeners::Site,
    },
    bot::delivery::{
        cards::{Card, CardContext, card_runs},
        preview::{posted, record_key, reminder_card, reminder_intent},
    },
    domain::{
        notify::{DeliverySettings, IntentContent},
        scheduler::Scope,
        settings::RuntimeSettings,
    },
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new().route("/api/admin/reminders/{id}/preview", get(preview))
}

async fn preview(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    path: Result<UrlPath<String>, PathRejection>,
) -> Result<Response, ApiError> {
    let Ok(UrlPath(id)) = path else {
        return Err(ApiError::NOT_FOUND);
    };
    let state = state(&site)?;
    let now = state.now();
    let [this, next] = frames(state, now)?;
    let snapshot = state
        .store
        .snapshot(Scope::Weeks(vec![this.start, next.start]))
        .await
        .map_err(unavailable)?;
    let profiles = state.store.members().await.map_err(unavailable)?;
    let ctx = context(&site, state, roster(&profiles), now);
    let (reminder, run, kind, row_state) = classified(&ctx, &snapshot)
        .find(|(reminder, ..)| reminder.id == id)
        .ok_or(ApiError::NOT_FOUND)?;
    let reminder_row = row(&ctx, reminder, run, kind, row_state);
    // The settings the tick posts with (quiet mode, post channel).
    let settings = match &state.config {
        Some(desk) => desk.settings().await,
        None => RuntimeSettings::default(),
    };
    let cards = CardContext {
        schedule: &snapshot,
        attendance: state.policy.attendance,
        zone: state.policy.zone(),
        quiet: settings.notifications.quiet_mode,
        members: &ctx.roster,
        catalog: Some(&state.catalog),
    };
    // Whether Discord's copy is frozen: refresh edits a posted card while any
    // of its runs is still ahead (`refresh.rs`); otherwise the reminder's run.
    let ahead = |run_id: &str| {
        snapshot
            .runs
            .iter()
            .any(|run| run.id == run_id && run.datetime > now)
    };
    let mut run_started = !ahead(&run.id);
    // What the card is built from: content, mentions and the heading line.
    let source = match (&reminder.sent_at, &reminder.message_id) {
        // Past its grace (unsent or retired): the tick retires it without posting.
        _ if row_state == ReminderState::Stale => None,
        // Posted: as the refresh worker edits it, over the runs it was posted for.
        (Some(_), Some(message_id)) => {
            let posted_card = state
                .store
                .posted_cards(run.id.clone())
                .await
                .map_err(unavailable)?
                .into_iter()
                .find(|card| !card.test && &card.message_id == message_id);
            if let Some(card) = &posted_card {
                run_started = !card.run_ids.iter().any(|id| ahead(id));
            }
            posted_card.and_then(|card| {
                let (content, mentions) = posted(&card, &cards)?;
                Some((content, mentions, card.record.heading))
            })
        }
        // Retired without posting: no card was sent and none will be.
        (Some(_), None) => None,
        // Unsent: as the tick will post it, grouped with the other unsent rows.
        (None, _) => match reminder_intent(
            &snapshot,
            &id,
            &ctx.roster,
            DeliverySettings {
                post_channel_id: settings.posting.channel_id.as_deref(),
                quiet_mode: settings.notifications.quiet_mode,
                attendance: state.policy.attendance,
            },
        ) {
            None => None,
            Some(intent) => {
                let heading = match record_key(&intent) {
                    Some(key) => state
                        .store
                        .card_record(key)
                        .await
                        .map_err(unavailable)?
                        .and_then(|record| record.heading),
                    None => None,
                };
                Some((intent.content, intent.mentions, heading))
            }
        },
    };
    let card = source.and_then(|(content, mentions, heading)| {
        let lead = match &content {
            IntentContent::DayOf { run_ids } => card_runs(&cards, run_ids)
                .first()
                .map(|run| run.bosses.clone()),
            IntentContent::Countdown { run_id, .. } => {
                cards.run(run_id).map(|run| run.bosses.clone())
            }
            _ => None,
        }
        .unwrap_or_default();
        let lead = ctx.bosses(&lead).into_iter().next();
        reminder_card(&content, &mentions, &cards, heading.as_deref()).map(|card| {
            view(
                card,
                heading.is_some(),
                lead.as_ref().and_then(|boss| boss.portrait.clone()),
                lead.and_then(|boss| boss.art),
            )
        })
    });
    Ok(Json(ReminderPreview {
        reminder: reminder_row,
        card,
        run_started,
        generated_at: iso_instant(now),
    })
    .into_response())
}

/// The card's art is shown only where it would be attached: the lead boss's
/// portrait and (day-of) entry art, when the file exists.
fn view(
    card: Card,
    heading_final: bool,
    portrait: Option<String>,
    entry: Option<String>,
) -> CardPreview {
    CardPreview {
        color: format!("#{:06x}", card.colour & 0x00ff_ffff),
        thumbnail: card.thumbnail.and(portrait),
        image: card.image.and(entry),
        content: card.content,
        description: card.description,
        fields: card
            .fields
            .into_iter()
            .map(|(name, value)| CardField { name, value })
            .collect(),
        footer: card.footer,
        heading_final,
    }
}
