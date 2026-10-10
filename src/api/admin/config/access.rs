//! The bot's permissions in the channels it uses (v4 `access_report`): every
//! watched text channel plus the digest channel, read live from the gateway
//! cache. `recheck` answers the same fresh reading (the cache follows the
//! gateway, so there is nothing to refetch).

use std::sync::Arc;

use axum::{Json, extract::State, response::IntoResponse};
use chrono::{DateTime, Datelike, TimeZone, Timelike, Weekday};
use serde::Serialize;

use super::Reply;
use crate::api::{
    admin::write::state,
    auth::AdminSession,
    listeners::Site,
    state::{ApiState, ChannelGrants},
};

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct AccessReport {
    connected: bool,
    /// Guild-local, e.g. `Tue 29 Sep 12:00`.
    checked_at: String,
    rows: Vec<AccessRow>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub(crate) struct AccessRow {
    id: String,
    name: String,
    watched: bool,
    digest: bool,
    view: bool,
    send: bool,
    history: bool,
    embed: bool,
    react: bool,
    manage_messages: bool,
}

pub(super) async fn read(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    Ok(Json(report(state(&site)?).await).into_response())
}

/// Safe to repeat; the session guard still applies CSRF to the POST.
pub(super) async fn recheck(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    Ok(Json(report(state(&site)?).await).into_response())
}

/// `Tue 29 Sep 12:00` (chrono's formatter is not compiled in).
fn local_label<Z: TimeZone>(at: DateTime<Z>) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let weekday = match at.weekday() {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    };
    let month = MONTHS[at.month0() as usize];
    format!(
        "{weekday} {:02} {month} {:02}:{:02}",
        at.day(),
        at.hour(),
        at.minute()
    )
}

async fn report(state: &ApiState) -> AccessReport {
    let checked_at = local_label(state.now().with_timezone(&state.policy.zone()));
    let channels = &state.channels;
    if !channels.connected() {
        return AccessReport {
            connected: false,
            checked_at,
            rows: Vec::new(),
        };
    }
    let digest = match &state.config {
        Some(desk) => desk.settings().await.posting.channel_id,
        None => None,
    };
    let rows = channels
        .channels()
        .into_iter()
        .filter(|channel| channel.watched || digest.as_deref() == Some(channel.id.as_str()))
        .map(|channel| {
            let grants = channels
                .grants(&channel.id)
                .unwrap_or(ChannelGrants::UNKNOWN);
            AccessRow {
                digest: digest.as_deref() == Some(channel.id.as_str()),
                id: channel.id,
                name: channel.name,
                watched: channel.watched,
                view: grants.view,
                send: grants.send,
                history: grants.history,
                embed: grants.embed,
                react: grants.react,
                manage_messages: grants.manage_messages,
            }
        })
        .collect();
    AccessReport {
        connected: true,
        checked_at,
        rows,
    }
}
