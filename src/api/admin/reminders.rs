//! `GET /api/admin/reminders/{id}/preview`: one reminder's card as the bot
//! posts it (unsent: the tick's planner) or edits it (posted: the refresh
//! worker's content), through the delivery card builder
//! (`bot::delivery::preview`). A row retired without posting has no card. Read only: no record is written, no heading
//! is generated, no art is read and nothing goes to Discord. Difficulty marks are the ones serve listed at startup.
//!
//! `GET /api/admin/reminders/{id}/estimate.png`: the "Who carries" image a
//! new post of that card attaches (redesigned day-of and countdown), drawn
//! by the delivery chart code from the roster and the estimates stored now
//! (never a lookup), under the tick's rules: none while a lookup includes a
//! party member with no stored result, and "older than n days" on its "as
//! of" line and alt text when the oldest reading is stale; `404` wherever
//! the preview's `card.estimate` is null.

use std::sync::Arc;

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path as UrlPath, State, rejection::PathRejection},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};

use super::context::{context, frames, roster, state, unavailable, weeks};
use crate::{
    api::{
        auth::{AdminSession, wire::encode},
        dto::{
            iso_instant,
            reminders::{
                CardField, CardPreview, EmbedPreview, EstimateImage, ReminderPreview, ReminderRow,
                ReminderState, classified, row,
            },
            week::Context,
        },
        error::ApiError,
        listeners::Site,
        state::{ApiState, CharacterRoster},
    },
    bot::delivery::{
        CachedEstimates,
        cards::{
            Card, CardContext,
            chart::{self, Chart},
        },
        draw,
        preview::{posted, record_key, reminder_card, reminder_chart, reminder_intent},
    },
    domain::{notify::DeliverySettings, settings::RuntimeSettings},
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/reminders/{id}/preview", get(preview))
        .route("/api/admin/reminders/{id}/estimate.png", get(estimate))
}

async fn preview(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    path: Result<UrlPath<String>, PathRejection>,
) -> Result<Response, ApiError> {
    let Ok(UrlPath(id)) = path else {
        return Err(ApiError::NOT_FOUND);
    };
    let Read {
        ctx,
        row,
        run_started,
        card,
        chart,
    } = read(&site, &id).await?;
    let estimate = chart.map(|chart| {
        let drawn = chart::svg(&chart);
        EstimateImage {
            url: format!("/api/admin/reminders/{}/estimate.png", encode(&id)),
            alt: chart::alt(&chart),
            width: drawn.width,
            height: drawn.height,
        }
    });
    Ok(Json(ReminderPreview {
        reminder: row,
        card: card.map(|(card, heading_final)| view(&ctx, card, heading_final, estimate)),
        run_started,
        generated_at: iso_instant(ctx.now),
    })
    .into_response())
}

async fn estimate(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    path: Result<UrlPath<String>, PathRejection>,
) -> Result<Response, ApiError> {
    let Ok(UrlPath(id)) = path else {
        return Err(ApiError::NOT_FOUND);
    };
    let chart = read(&site, &id).await?.chart.ok_or(ApiError::NOT_FOUND)?;
    // The tick's own deadline and size cap; a card it leaves without the
    // image has none here either.
    let upload = draw(chart).await.map_err(|_| ApiError::UNAVAILABLE)?;
    Ok((
        [(header::CONTENT_TYPE, "image/png")],
        Body::from(Bytes::from_owner(upload.bytes)),
    )
        .into_response())
}

/// One reminder as previewed: its row, whether Discord's copy is frozen,
/// and its card (with whether the heading is stored) and chart.
struct Read<'a> {
    ctx: Context<'a>,
    row: ReminderRow,
    run_started: bool,
    card: Option<(Card, bool)>,
    chart: Option<Chart>,
}

/// The share chart's port as serve wires the tick's: the roster and the
/// estimates memory, while module `roster` and module `character_data`
/// with a scouter URL are on.
fn estimates(state: &ApiState) -> Option<CachedEstimates<dyn CharacterRoster>> {
    let roster = state.characters.as_ref()?;
    let module = state
        .character_data
        .as_ref()
        .filter(|module| module.estimates().is_some())?;
    Some(CachedEstimates::new(Arc::clone(roster), Arc::clone(module)))
}

async fn read<'a>(site: &'a Site, id: &str) -> Result<Read<'a>, ApiError> {
    let state = state(site)?;
    let now = state.now();
    let [this, next] = frames(state, now)?;
    let snapshot = weeks(state, vec![this.start, next.start]).await?;
    let profiles = state.store.members().await.map_err(unavailable)?;
    let ctx = context(site, state, roster(&profiles), now);
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
        style: settings.notifications.message_style,
        marks: &state.marks,
        // The preview keeps the embed rendering of a V2 digest.
        v2: None,
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
            id,
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
    let mut card = None;
    let mut chart = None;
    if let Some((content, mentions, heading)) = source
        && let Some(built) = reminder_card(&content, &mentions, &cards, heading.as_deref())
    {
        if let Some(port) = estimates(state) {
            chart = reminder_chart(&built, &content, &cards, &port, now).await;
        }
        card = Some((built, heading.is_some()));
    }
    Ok(Read {
        ctx,
        row: reminder_row,
        run_started,
        card,
        chart,
    })
}

/// The card's art is shown only where it would be attached: each embed's
/// lead boss portrait and (day-of) entry art, when the file exists.
fn view(
    ctx: &Context<'_>,
    card: Card,
    heading_final: bool,
    estimate: Option<EstimateImage>,
) -> CardPreview {
    let mut embeds = card.embeds.into_iter().map(|embed| {
        let boss = embed
            .lead
            .and_then(|token| ctx.bosses(&[token]).into_iter().next());
        EmbedPreview {
            color: format!("#{:06x}", embed.colour & 0x00ff_ffff),
            title: embed.title,
            thumbnail: embed
                .thumbnail
                .and(boss.as_ref().and_then(|boss| boss.portrait.clone())),
            image: embed.image.and(boss.and_then(|boss| boss.art)),
            description: embed.description,
            fields: embed
                .fields
                .into_iter()
                .map(|field| CardField {
                    name: field.name,
                    value: field.value,
                    inline: field.inline,
                })
                .collect(),
            footer: embed.footer,
        }
    });
    let first = embeds.next().unwrap_or_else(|| EmbedPreview {
        color: "#000000".to_owned(),
        title: None,
        description: None,
        fields: Vec::new(),
        footer: None,
        thumbnail: None,
        image: None,
    });
    CardPreview {
        content: card.content,
        estimate,
        color: first.color,
        title: first.title,
        description: first.description,
        fields: first.fields,
        footer: first.footer,
        thumbnail: first.thumbnail,
        image: first.image,
        more_embeds: embeds.collect(),
        heading_final,
    }
}
