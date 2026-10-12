//! `character_data.json`: one character's link and Nexon data (module
//! `character_data`) and the module's Config health block. The ocid never
//! appears here.

use chrono::{DateTime, Utc};
use chrono_tz::Asia::Singapore;
use serde::Serialize;

use super::{history::SettingsActor, iso_instant};
use crate::domain::chardata::{
    ATTRIBUTION, LinkStatus, NOTICE_VERSION, REFRESH_CAP, SNAPSHOT_TTL, Snapshot, date_text,
};
use crate::infrastructure::character_data::{Health, LinkView};

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CharacterLink {
    pub character_id: String,
    /// `not_connected`: no Nexon key is configured (and no link exists);
    /// `expired`: linked, but the data passed its 30 days.
    #[cfg_attr(
        test,
        ts(type = "'not_connected' | 'not_linked' | 'pending' | 'linked' | 'stale' | 'expired'")
    )]
    pub state: &'static str,
    /// Send it back with a link request; 0 while there is no link.
    pub version: u32,
    /// A Nexon key is configured: links can be made.
    pub connected: bool,
    pub requested_by: Option<SettingsActor>,
    pub requested_at: Option<String>,
    /// The notice version the member confirmed.
    pub consent_version: Option<u32>,
    pub consented_at: Option<String>,
    pub linked_at: Option<String>,
    /// The current notice version.
    pub notice_version: u32,
    /// Fresh Nexon data; `null` without any.
    pub data: Option<CharacterFacts>,
    pub refresh: RefreshInfo,
    /// Shown with the data: `Data based on NEXON Open API`.
    pub attribution: &'static str,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CharacterFacts {
    pub name: Option<String>,
    pub world: Option<String>,
    pub class: Option<String>,
    pub level: Option<u16>,
    #[cfg_attr(test, ts(type = "number | null"))]
    pub combat_power: Option<u64>,
    /// Hotlinked from Nexon, never re-hosted.
    pub image_url: Option<String>,
    /// `YYYY-MM-DD` (Singapore): the response date, else the fetch day.
    pub as_of: String,
    pub fetched_at: String,
    /// When the data is deleted unless refreshed.
    pub expires_at: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RefreshInfo {
    /// An on-demand refresh is refused until then (24 hours after the last).
    pub refreshable_at: Option<String>,
    pub next_attempt_at: Option<String>,
    /// The Open API error code (or reason) that stopped the last refresh.
    pub last_error: Option<String>,
}

fn singapore_day(at: DateTime<Utc>) -> String {
    date_text(at.with_timezone(&Singapore).date_naive())
}

fn facts(snapshot: &Snapshot) -> CharacterFacts {
    CharacterFacts {
        name: snapshot.character_name.clone(),
        world: snapshot.world_name.clone(),
        class: snapshot.character_class.clone(),
        level: snapshot.level,
        combat_power: snapshot.combat_power,
        image_url: snapshot.image_url.clone(),
        as_of: snapshot
            .data_date
            .map_or_else(|| singapore_day(snapshot.fetched_at), date_text),
        fetched_at: iso_instant(snapshot.fetched_at),
        expires_at: iso_instant(snapshot.expires_at),
    }
}

pub fn link(character_id: &str, view: &LinkView) -> CharacterLink {
    let link = view.link.as_ref();
    let state = match link.map(|link| link.status) {
        None if view.connected => "not_linked",
        None => "not_connected",
        Some(LinkStatus::Pending) => "pending",
        Some(LinkStatus::Stale) => "stale",
        Some(LinkStatus::Active) => match (&view.snapshot, link.and_then(|link| link.linked_at)) {
            (None, Some(linked)) if linked + SNAPSHOT_TTL <= view.now => "expired",
            _ => "linked",
        },
    };
    let refreshable_at = view
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.fetched_at + REFRESH_CAP)
        .filter(|at| *at > view.now && state == "linked")
        .map(iso_instant);
    let refresh = view.refresh.as_ref();
    CharacterLink {
        character_id: character_id.to_owned(),
        state,
        version: link.map_or(0, |link| link.version),
        connected: view.connected,
        requested_by: link.map(|link| SettingsActor {
            kind: link.requested_by.kind(),
            id: link.requested_by.id().to_owned(),
        }),
        requested_at: link.map(|link| iso_instant(link.requested_at)),
        consent_version: link.and_then(|link| link.consent_version),
        consented_at: link.and_then(|link| link.consented_at).map(iso_instant),
        linked_at: link.and_then(|link| link.linked_at).map(iso_instant),
        notice_version: NOTICE_VERSION,
        data: view
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.has_data())
            .map(facts),
        refresh: RefreshInfo {
            refreshable_at,
            next_attempt_at: refresh
                .and_then(|state| state.next_attempt_at)
                .map(iso_instant),
            last_error: refresh.and_then(|state| state.last_error_code.clone()),
        },
        attribution: ATTRIBUTION,
    }
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CharacterDataHealth {
    #[cfg_attr(test, ts(type = "'no_key' | 'ok' | 'backoff' | 'key_refused'"))]
    pub nexon: &'static str,
    /// Every call waits until then (maintenance).
    pub backoff_until: Option<String>,
    /// Calls in the last 24 hours; `null` when the file could not be read.
    pub calls_24h: Option<u32>,
    /// The hard stop on calls per 24 hours.
    pub budget: u32,
    pub last_batch: Option<BatchHealth>,
    pub links: Option<LinkHealth>,
    pub last_sweep: Option<String>,
    pub posts: Option<PostHealth>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct BatchHealth {
    pub at: String,
    pub refreshed: u32,
    pub failed: u32,
    /// Why it stopped early, if it did.
    pub stopped: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct LinkHealth {
    pub pending: u32,
    pub active: u32,
    pub stale: u32,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PostHealth {
    pub due: u32,
    pub unknown: u32,
    pub stuck: u32,
    pub overdue: u32,
}

pub fn health(health: &Health) -> CharacterDataHealth {
    CharacterDataHealth {
        nexon: health.nexon.as_str(),
        backoff_until: health.backoff_until.map(iso_instant),
        calls_24h: health.calls_24h,
        budget: health.budget,
        last_batch: health.last_batch.as_ref().map(|batch| BatchHealth {
            at: iso_instant(batch.at),
            refreshed: batch.refreshed,
            failed: batch.failed,
            stopped: batch.stopped.clone(),
        }),
        links: health.links.map(|links| LinkHealth {
            pending: links.pending,
            active: links.active,
            stale: links.stale,
        }),
        last_sweep: health.last_sweep.map(iso_instant),
        posts: health.posts.map(|posts| PostHealth {
            due: posts.due,
            unknown: posts.unknown,
            stuck: posts.stuck,
            overdue: posts.overdue,
        }),
    }
}
