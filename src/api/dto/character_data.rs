//! `character_data.json`: one character's link and Nexon data (module
//! `character_data`) and the module's Config health block. The ocid never
//! appears here.

use serde::Serialize;

use super::{history::SettingsActor, iso_instant};
use crate::domain::chardata::{
    ATTRIBUTION, ConsentSurface, Link, LinkStatus, NOTICE_VERSION, SNAPSHOT_TTL, Snapshot,
    date_text,
};
use crate::infrastructure::character_data::{Found, Health, LinkView};

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
    /// Who made the link: the admin who linked it (at once, no member
    /// confirmation), else the member.
    pub requested_by: Option<SettingsActor>,
    pub requested_at: Option<String>,
    /// The notice version the member confirmed; `null` for an admin's link
    /// (the member confirmed none).
    pub consent_version: Option<u32>,
    pub consented_at: Option<String>,
    pub linked_at: Option<String>,
    /// Where the member consented: `discord` (`/character link`) or
    /// `portal`; `null` while not consented and for consents given before
    /// it was recorded.
    #[cfg_attr(test, ts(type = "'discord' | 'portal' | null"))]
    pub consent_surface: Option<&'static str>,
    /// The current notice version.
    pub notice_version: u32,
    /// Fresh Nexon data; `null` without any.
    pub data: Option<CharacterFacts>,
    pub refresh: RefreshInfo,
    /// Shown with the data: `Data based on NEXON Open API`.
    pub attribution: &'static str,
    /// The public `/privacy` page; `null` without a public listener.
    pub privacy_url: Option<String>,
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
    pub next_attempt_at: Option<String>,
    /// The Open API error code (or reason) that stopped the last refresh.
    pub last_error: Option<String>,
}

pub fn facts(snapshot: &Snapshot) -> CharacterFacts {
    CharacterFacts {
        name: snapshot.character_name.clone(),
        world: snapshot.world_name.clone(),
        class: snapshot.character_class.clone(),
        level: snapshot.level,
        combat_power: snapshot.combat_power,
        image_url: snapshot.image_url.clone(),
        as_of: date_text(snapshot.as_of()),
        fetched_at: iso_instant(snapshot.fetched_at),
        expires_at: iso_instant(snapshot.expires_at),
    }
}

/// The strip's state for a link view (`CharacterLink.state`).
pub fn link_state(view: &LinkView) -> &'static str {
    let link = view.link.as_ref();
    match link.map(|link| link.status) {
        None if view.connected => "not_linked",
        None => "not_connected",
        Some(LinkStatus::Pending) => "pending",
        Some(LinkStatus::Stale) => "stale",
        Some(LinkStatus::Active) => match (&view.snapshot, link.and_then(|link| link.linked_at)) {
            (None, Some(linked)) if linked + SNAPSHOT_TTL <= view.now => "expired",
            _ => "linked",
        },
    }
}

pub fn link(character_id: &str, view: &LinkView, privacy_url: Option<&str>) -> CharacterLink {
    let link = view.link.as_ref();
    let state = link_state(view);
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
        consent_version: consent(link).and_then(|link| link.consent_version),
        consented_at: consent(link)
            .and_then(|link| link.consented_at)
            .map(iso_instant),
        linked_at: link.and_then(|link| link.linked_at).map(iso_instant),
        consent_surface: link
            .and_then(|link| link.consent_surface)
            .map(ConsentSurface::as_str),
        notice_version: NOTICE_VERSION,
        data: view
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.has_data())
            .map(facts),
        refresh: RefreshInfo {
            next_attempt_at: refresh
                .and_then(|state| state.next_attempt_at)
                .map(iso_instant),
            last_error: refresh.and_then(|state| state.last_error_code.clone()),
        },
        attribution: ATTRIBUTION,
        privacy_url: privacy_url.map(str::to_owned),
    }
}

/// `link` when its member consented to it (not an admin's link).
pub fn consent(link: Option<&Link>) -> Option<&Link> {
    link.filter(|link| !link.admin_linked())
}

/// `GET /api/admin/characters/lookup`: whether a lookup (and the link a
/// MapleSEA create asks for) can be made.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct LookupStatus {
    /// A Nexon key is configured.
    pub connected: bool,
}

/// `POST /api/admin/characters/lookup`: what NEXON says about a typed IGN,
/// shown in the Add character dialog only. Nothing of it is stored.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CharacterLookup {
    pub name: Option<String>,
    pub world: Option<String>,
    pub class: Option<String>,
    pub level: Option<u16>,
    /// Hotlinked from Nexon, never re-hosted.
    pub image_url: Option<String>,
    /// `YYYY-MM-DD` (Singapore): the response date, else the lookup day.
    pub as_of: String,
    /// Shown with the values: `Data based on NEXON Open API`.
    pub attribution: &'static str,
}

pub fn lookup(found: Found) -> CharacterLookup {
    CharacterLookup {
        name: found.name,
        world: found.world,
        class: found.class,
        level: found.level,
        image_url: found.image_url,
        as_of: date_text(found.as_of),
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
    pub scouter: ScouterHealthBlock,
}

/// The scouter (estimates): `off` without `KANADE_SCOUTER_URL`.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ScouterHealthBlock {
    #[cfg_attr(
        test,
        ts(type = "'off' | 'ok' | 'unreachable' | 'verification_required'")
    )]
    pub state: &'static str,
    /// The sidecar's `http://host[:port]` (no secret); `null` while off.
    pub url: Option<String>,
    pub last_lookup_at: Option<String>,
    /// The last error's code: the scouter's (`busy`,
    /// `verification_required`, `lookup_timeout`, …) or `timeout`,
    /// `unreachable`, `unreadable`.
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
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
    /// The posts behind `unknown`, `stuck` and `overdue`, oldest first.
    pub items: Vec<PostIssueItem>,
}

/// A share card an admin should look at. No Nexon data: ids and states.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PostIssueItem {
    /// The 6-hex ref in the card's footer.
    pub reference: String,
    #[cfg_attr(test, ts(type = "'unknown' | 'stuck' | 'overdue'"))]
    pub state: &'static str,
    pub channel_id: String,
    pub posted_at: String,
    /// A Discord link to the message for a manual removal; `null` when
    /// its id or the guild is not known.
    pub jump_url: Option<String>,
}

/// `guild_id` makes the items' jump links.
pub fn health(health: &Health, guild_id: Option<&str>) -> CharacterDataHealth {
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
            items: health
                .post_issues
                .iter()
                .map(|issue| PostIssueItem {
                    reference: issue.reference.clone(),
                    state: issue.state,
                    channel_id: issue.channel_id.clone(),
                    posted_at: iso_instant(issue.posted_at),
                    jump_url: guild_id
                        .zip(issue.message_id.as_deref())
                        .map(|(guild, message)| {
                            format!(
                                "https://discord.com/channels/{guild}/{}/{message}",
                                issue.channel_id
                            )
                        }),
                })
                .collect(),
        }),
        scouter: ScouterHealthBlock {
            state: health.scouter.state.as_str(),
            url: health.scouter.url.clone(),
            last_lookup_at: health.scouter.last_lookup_at.map(iso_instant),
            last_error: health
                .scouter
                .last_error
                .as_ref()
                .map(|(code, _)| code.clone()),
            last_error_at: health
                .scouter
                .last_error
                .as_ref()
                .map(|(_, at)| iso_instant(*at)),
        },
    }
}
