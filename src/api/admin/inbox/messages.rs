//! A proposal card's channel messages from the watched-message cache (never
//! Discord): the evidence it cites and the bounded thread around it. A
//! deleted or pruned message is gone from the cache, so it is `missing`
//! evidence and absent from the thread.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::super::write::Refusal;
use crate::{
    api::{
        dto::{
            inbox::{Evidence, Said, ThreadMessage, message_url},
            week::Context,
            when,
        },
        error::ApiError,
        state::ApiState,
    },
    domain::{model_log::WatchedMessage, proposals::StoredCard},
    extract::{
        pipeline::{CONTEXT_WINDOW, DEFAULT_CONTEXT_MESSAGES},
        window::MAX_BURST_MESSAGES,
    },
};

/// At most what one extraction call reads: its context before the burst
/// plus a whole burst.
pub const THREAD_LIMIT: usize = DEFAULT_CONTEXT_MESSAGES + MAX_BURST_MESSAGES;

/// Discord's epoch in Unix milliseconds.
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

/// When Discord created the object with this snowflake id.
fn snowflake_time(id: &str) -> Option<DateTime<Utc>> {
    let id: u64 = id.parse().ok()?;
    DateTime::from_timestamp_millis(i64::try_from(id >> 22).ok()? + DISCORD_EPOCH_MS)
}

fn present(ctx: &Context<'_>, message: &WatchedMessage) -> Evidence {
    Evidence {
        id: message.id.clone(),
        author: ctx.name(&message.author_id),
        author_id: Some(message.author_id.clone()),
        at: when(message.created_at, ctx.zone),
        content: Some(message.content.clone()),
        url: message_url(ctx, &message.channel_id, &message.id),
        missing: false,
    }
}

/// The card's evidence and thread; `read_at` (when the proposal was made)
/// caps the thread, so later chatter never shows as context.
pub async fn said(
    state: &ApiState,
    ctx: &Context<'_>,
    card: Option<&StoredCard>,
    read_at: DateTime<Utc>,
) -> Result<Said, Refusal> {
    let Some(card) = card else {
        return Ok(Said {
            evidence: Vec::new(),
            thread: None,
        });
    };
    let ids = &card.details.evidence_message_ids;
    let times: Vec<DateTime<Utc>> = ids.iter().filter_map(|id| snowflake_time(id)).collect();
    let (Some(&start), Some(&end)) = (times.iter().min(), times.iter().max()) else {
        return Ok(Said {
            evidence: ids.iter().map(|id| gone(ctx, id)).collect(),
            thread: None,
        });
    };
    let messages = state
        .store
        .messages(card.channel_id.clone(), start - CONTEXT_WINDOW)
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?;
    let evidence = ids
        .iter()
        .map(
            |id| match messages.iter().find(|message| &message.id == id) {
                Some(message) => present(ctx, message),
                None => gone(ctx, id),
            },
        )
        .collect();
    let used: BTreeSet<&str> = ids.iter().map(String::as_str).collect();
    let thread = thread(&messages, &used, (start, end), read_at)
        .into_iter()
        .map(|message| ThreadMessage {
            used: used.contains(message.id.as_str()),
            message: present(ctx, message),
        })
        .collect();
    Ok(Said {
        evidence,
        thread: Some(thread),
    })
}

fn gone(ctx: &Context<'_>, id: &str) -> Evidence {
    Evidence {
        id: id.to_owned(),
        author: "someone".into(),
        author_id: None,
        at: snowflake_time(id).map_or_else(String::new, |at| when(at, ctx.zone)),
        content: None,
        url: None,
        missing: true,
    }
}

/// Oldest first: up to [`DEFAULT_CONTEXT_MESSAGES`] before the first
/// evidence (as the extractor's context), everything between first and last
/// evidence, and up to [`MAX_BURST_MESSAGES`] after it until `read_at`. Past
/// [`THREAD_LIMIT`] the oldest unused messages go first.
fn thread<'a>(
    rows: &'a [WatchedMessage],
    used: &BTreeSet<&str>,
    (start, end): (DateTime<Utc>, DateTime<Utc>),
    read_at: DateTime<Utc>,
) -> Vec<&'a WatchedMessage> {
    let before: Vec<&WatchedMessage> = rows
        .iter()
        .filter(|row| row.created_at >= start - CONTEXT_WINDOW && row.created_at < start)
        .collect();
    let mut out: Vec<&WatchedMessage> =
        before[before.len().saturating_sub(DEFAULT_CONTEXT_MESSAGES)..].to_vec();
    out.extend(
        rows.iter()
            .filter(|row| row.created_at >= start && row.created_at <= end),
    );
    out.extend(
        rows.iter()
            .filter(|row| row.created_at > end && row.created_at <= read_at)
            .take(MAX_BURST_MESSAGES),
    );
    while out.len() > THREAD_LIMIT {
        let drop = out
            .iter()
            .position(|row| !used.contains(row.id.as_str()))
            .unwrap_or(0);
        out.remove(drop);
    }
    out
}
