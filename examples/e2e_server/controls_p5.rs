//! Module `character_data`'s controls, each driving
//! the real module: the fake scouter and NEXON answer as asked
//! ([`crate::fakes::chardata`]), the real service calls them, and a change
//! of key or scouter URL is a restart, as serve reads both once at startup.
//! A reset puts all of it back.
//!
//! - `POST /__e2e/estimates` `{scenario, ign?}`: what later scouter lookups
//!   do ([`Scenario`]; `assumed` labels `ign`'s result), or `queued` (two
//!   other members' lookups start and wait; once a lookup is queued behind
//!   them they finish one by one) or `off` (a restart without
//!   `KANADE_SCOUTER_URL`). 204.
//! - `POST /__e2e/character-data` `{health}`: Config's health, each through
//!   real calls on a fresh restart: `ok`; `alert` (a refresh during NEXON
//!   maintenance holds calls for 30 minutes, a lookup the scouter answers
//!   `verification_required`, a stuck and an unknown share card); `refused`
//!   (a refresh NEXON answers with a refused key, the scouter's health
//!   probe fails); `setup` (no key and no scouter URL); `budget` (the
//!   24-hour call budget spent, no restart). 204.
//! - `POST /__e2e/public/characters` `{roster: "board" | "empty"}`: Asahi's
//!   own characters as the member boards draw them, written through the
//!   roster and character-data stores (`board`: AsahiKanna linked main with
//!   one share card, AsahiDK waiting for Asahi to confirm, AsahiPath stale,
//!   AsahiBlaze expired, AsahiMech retired, ids `ch-b1`…`ch-b5`), or none.
//!   204.
//!
//! The harness also watches every successful `POST …/estimates` (admin and
//! member): that is the lookup queued behind `queued`'s held ones.

use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Request, State, rejection::JsonRejection},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::{DateTime, Duration as Span, Utc};
use kanade::{
    api::state::ApiState,
    domain::{
        characters::{Character, CharacterChange, CharacterRecord, CharacterRole, CharacterStore},
        chardata::{
            CharDataStore, ConsentSurface, DAILY_CALLS, Link, LinkPut, LinkStatus, NOTICE_VERSION,
            Post, PostState, STRIP_AFTER, Snapshot,
        },
        history::{Actor, Surface},
    },
    infrastructure::{
        character_data::CharacterData,
        scouter::{JobRequest, JobState, RosterIgn, Start, Subject, Target},
        store::SqliteStore,
    },
};
use serde::Deserialize;

use super::{Shared, Side};
use crate::{
    Fail,
    fakes::chardata::{Nexon, Scenario, Setup},
    live::World,
    seed::ids::{ASAHI, HSTAR_PARTY, KAITO, SORA},
};

/// How long `alert` waits for its scouter lookup.
const LOOKUP_WAIT: Duration = Duration::from_secs(10);
/// The other members' lookups `queued` holds: SoraPF and KaitoLyn.
const OTHERS: [(&str, &str); 2] = [("ch-s11", SORA), ("ch-s13", KAITO)];

pub(super) fn routes() -> Router<Side> {
    Router::new()
        .route("/__e2e/estimates", post(set_estimates))
        .route("/__e2e/character-data", post(set_health))
        .route("/__e2e/public/characters", post(set_member_characters))
}

/// Before a reset composes: the fakes and the module's setup as at startup.
pub(super) fn reset(world: &World) {
    world.chardata.reset();
}

/// A lookup started on either origin may be the one `queued` waits for.
pub(super) async fn observe(State(side): State<Side>, request: Request, next: Next) -> Response {
    let lookup = request.method() == Method::POST && request.uri().path().ends_with("/estimates");
    let response = next.run(request).await;
    if lookup && response.status().is_success() {
        side.shared.world.chardata.queued_behind();
    }
    response
}

fn answer(done: Result<(), Fail>) -> Response {
    match done {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

impl Shared {
    /// The live generation's store and API state.
    async fn live_parts(&self) -> Result<(Arc<SqliteStore>, Arc<ApiState>), Fail> {
        let slot = self.generation.lock().await;
        let live = slot.live.as_ref().ok_or("nothing is being served")?;
        Ok((Arc::clone(live.store()), Arc::clone(&live.live().state)))
    }

    /// Module `character_data` of the live generation.
    async fn character_data(&self) -> Result<Arc<CharacterData>, Fail> {
        let (_, state) = self.live_parts().await?;
        state
            .character_data
            .clone()
            .ok_or_else(|| "module character_data is off".into())
    }

    /// Restarts with the modules on now (and the fakes' setup).
    async fn restart_same(&self) -> Result<(), Fail> {
        let modules = self.generation.lock().await.modules;
        self.restart(modules).await.map(|_| ())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EstimatesChange {
    scenario: String,
    ign: Option<String>,
}

async fn set_estimates(
    State(side): State<Side>,
    body: Result<Json<EstimatesChange>, JsonRejection>,
) -> Response {
    let Ok(Json(change)) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let shared = &side.shared;
    let modes = &shared.world.chardata;
    match change.scenario.as_str() {
        "off" => {
            modes.set_setup(Setup {
                no_scouter: true,
                ..modes.setup()
            });
            answer(shared.restart_same().await)
        }
        "queued" => answer(queue_others(shared).await),
        other => match serde_json::from_value::<Scenario>(serde_json::Value::from(other)) {
            Ok(scenario) => {
                modes.set_scenario(scenario, change.ign.as_deref());
                StatusCode::NO_CONTENT.into_response()
            }
            Err(_) => StatusCode::UNPROCESSABLE_ENTITY.into_response(),
        },
    }
}

/// A roster character as a scouter subject.
async fn subject(store: &SqliteStore, id: &str) -> Result<Subject, Fail> {
    let character = store
        .character(id)
        .await?
        .ok_or_else(|| format!("no character {id}"))?;
    let ign = RosterIgn::of(&character).ok_or_else(|| format!("{id} has no scouter IGN"))?;
    Ok(Subject {
        character_id: character.id,
        member_id: character.member_id,
        ign,
    })
}

/// Starts a refreshed lookup of one character as `actor`; its job id.
fn start_lookup(data: &CharacterData, subject: Subject, actor: Actor) -> Result<String, Fail> {
    let estimates = data.estimates().ok_or("the scouter is off")?;
    match estimates.start(JobRequest {
        target: Target::Characters(vec![subject.character_id.clone()]),
        subjects: vec![subject],
        missing: Vec::new(),
        started_by: actor,
        refresh: true,
    }) {
        Start::Started(job) | Start::Joined(job) => Ok(job.id),
        Start::Full => Err("the lookup queue is full".into()),
    }
}

/// Other members' lookups, held until one is queued behind them.
async fn queue_others(shared: &Shared) -> Result<(), Fail> {
    let (store, _) = shared.live_parts().await?;
    let data = shared.character_data().await?;
    let modes = &shared.world.chardata;
    modes.set_scenario(Scenario::Ok, None);
    let mut subjects = Vec::with_capacity(OTHERS.len());
    for (id, _) in OTHERS {
        subjects.push(subject(&store, id).await?);
    }
    let igns: Vec<&str> = subjects
        .iter()
        .map(|subject| subject.ign.as_str())
        .collect();
    modes.hold_igns(&igns);
    for (subject, (_, member)) in subjects.into_iter().zip(OTHERS) {
        start_lookup(&data, subject, Actor::member(member))?;
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HealthChange {
    health: String,
}

async fn set_health(
    State(side): State<Side>,
    body: Result<Json<HealthChange>, JsonRejection>,
) -> Response {
    let Ok(Json(change)) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let shared = &side.shared;
    let modes = &shared.world.chardata;
    let done = match change.health.as_str() {
        "ok" => {
            modes.reset();
            shared.restart_same().await
        }
        "setup" => {
            modes.set_setup(Setup {
                no_key: true,
                no_scouter: true,
            });
            shared.restart_same().await
        }
        "alert" => alert(shared).await,
        "refused" => refused(shared).await,
        "budget" => spend_budget(shared).await,
        _ => return StatusCode::UNPROCESSABLE_ENTITY.into_response(),
    };
    answer(done)
}

async fn alert(shared: &Shared) -> Result<(), Fail> {
    let modes = &shared.world.chardata;
    shared.restart_same().await?;
    let (store, state) = shared.live_parts().await?;
    let data = shared.character_data().await?;
    // A refresh during maintenance: every call held 30 minutes.
    modes.set_nexon(Nexon::Down);
    let refreshed = data.refresh_now("ch-s2").await;
    modes.set_nexon(Nexon::Ok);
    refreshed?;
    // The scouter asks for a verification.
    modes.set_scenario(Scenario::ManualCheck, None);
    let asahi = Actor::admin(format!("discord:{ASAHI}"));
    let job = start_lookup(&data, subject(&store, "ch-s3").await?, asahi)?;
    let estimates = data.estimates().ok_or("the scouter is off")?;
    let deadline = tokio::time::Instant::now() + LOOKUP_WAIT;
    while estimates
        .job(&job)
        .is_some_and(|job| matches!(job.state(), JobState::Queued | JobState::Running))
    {
        if tokio::time::Instant::now() > deadline {
            return Err("the scouter lookup did not finish".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    modes.set_scenario(Scenario::Ok, None);
    // Two share cards an admin should look at: one stuck in its channel,
    // one whose send never came back.
    let now = (state.clock)();
    for (reference, character, message, post_state, days) in [
        (
            "3fa91c",
            "ch-s3",
            Some("1424200000000000001"),
            PostState::Stuck,
            6,
        ),
        ("b71e02", "ch-s9", None, PostState::Unknown, 2),
    ] {
        let posted_at = now - Span::days(days);
        let post = Post {
            reference: reference.into(),
            channel_id: HSTAR_PARTY.into(),
            message_id: message.map(str::to_owned),
            character_id: character.into(),
            posted_at,
            strip_due_at: posted_at + STRIP_AFTER,
            state: post_state,
        };
        if !data.store().create_post(post).await? {
            return Err(format!("post {reference} was refused").into());
        }
    }
    Ok(())
}

async fn refused(shared: &Shared) -> Result<(), Fail> {
    let modes = &shared.world.chardata;
    shared.restart_same().await?;
    let data = shared.character_data().await?;
    modes.set_nexon(Nexon::Refused);
    // Down from now on: the fresh generation's next health read probes it.
    modes.set_scouter_down(true);
    data.refresh_now("ch-s2").await?;
    Ok(())
}

async fn spend_budget(shared: &Shared) -> Result<(), Fail> {
    let data = shared.character_data().await?;
    let now = data.now();
    while data.store().reserve_call(now, DAILY_CALLS).await? {}
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RosterChange {
    roster: String,
}

async fn set_member_characters(
    State(side): State<Side>,
    body: Result<Json<RosterChange>, JsonRejection>,
) -> Response {
    let Ok(Json(change)) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let board = match change.roster.as_str() {
        "board" => true,
        "empty" => false,
        _ => return StatusCode::UNPROCESSABLE_ENTITY.into_response(),
    };
    answer(member_characters(&side.shared, board).await)
}

/// `(id, IGN, class, level, role, retired)`, oldest first.
const BOARD: [(&str, &str, &str, u16, CharacterRole, bool); 5] = [
    (
        "ch-b1",
        "AsahiKanna",
        "Kanna",
        285,
        CharacterRole::Main,
        false,
    ),
    (
        "ch-b2",
        "AsahiDK",
        "Dark Knight",
        284,
        CharacterRole::Alt,
        false,
    ),
    (
        "ch-b3",
        "AsahiPath",
        "Pathfinder",
        279,
        CharacterRole::Alt,
        false,
    ),
    (
        "ch-b4",
        "AsahiBlaze",
        "Blaze Wizard",
        272,
        CharacterRole::Alt,
        false,
    ),
    (
        "ch-b5",
        "AsahiMech",
        "Mechanic",
        262,
        CharacterRole::Alt,
        true,
    ),
];

/// Replaces Asahi's characters (and their links) with the board's, or none.
async fn member_characters(shared: &Shared, board: bool) -> Result<(), Fail> {
    let (store, state) = shared.live_parts().await?;
    let now = (state.clock)();
    let admin = Actor::admin(format!("discord:{ASAHI}"));
    let record = |at: DateTime<Utc>| CharacterRecord {
        at,
        actor: admin.clone(),
        surface: Surface::AdminPortal,
    };
    let links = state
        .character_data
        .as_ref()
        .map(|data| Arc::clone(data.store()));
    // The main goes last: the roster keeps a main while alts remain.
    let mut rows = store.member_characters(ASAHI).await?;
    rows.sort_by_key(|row| row.role == CharacterRole::Main);
    for row in rows {
        if let Some(links) = &links {
            links.unlink(row.id.clone(), now).await?;
        }
        let change = CharacterChange::Delete {
            id: row.id.clone(),
            version: row.version,
        };
        let written = store.apply_character(change, record(now)).await?;
        if !written.applied() {
            return Err(format!("deleting {} was refused: {written:?}", row.ign).into());
        }
    }
    if !board {
        return Ok(());
    }
    let created = now - Span::days(60);
    for (n, (id, ign, class, level, role, retired)) in (0_i64..).zip(BOARD) {
        let at = created + Span::hours(n);
        let row = Character {
            id: id.into(),
            member_id: ASAHI.into(),
            ign: ign.into(),
            world: "msea".into(),
            class: Some(class.into()),
            level: Some(level),
            role,
            retired_at: retired.then_some(now - Span::days(3)),
            created_at: at,
            updated_at: at,
            updated_by: admin.clone(),
            version: 1,
        };
        let written = store
            .apply_character(CharacterChange::Create(row), record(at))
            .await?;
        if !written.applied() {
            return Err(format!("adding {ign} was refused: {written:?}").into());
        }
    }
    match links {
        Some(links) => board_links(links.as_ref(), now).await,
        None => Ok(()),
    }
}

async fn put(links: &dyn CharDataStore, link: Link, expected: u32) -> Result<(), Fail> {
    match links.put_link(link, expected).await? {
        LinkPut::Written => Ok(()),
        LinkPut::Stale(_) => Err("a board link was stale".into()),
    }
}

/// AsahiKanna linked with data and a share card, AsahiDK waiting for Asahi
/// since today (the sweeper's re-pend), AsahiPath stale, AsahiBlaze linked
/// past its data's 30 days.
async fn board_links(links: &dyn CharDataStore, now: DateTime<Utc>) -> Result<(), Fail> {
    let pending = |id: &str, ign: &str, by: Actor, at: DateTime<Utc>| Link {
        character_id: id.into(),
        member_id: ASAHI.into(),
        ign_key: ign.to_ascii_lowercase(),
        region: "msea".into(),
        status: LinkStatus::Pending,
        requested_by: by,
        requested_at: at,
        consent_version: None,
        consented_at: None,
        linked_at: None,
        consent_surface: None,
        version: 1,
    };
    let sweeper = Actor::system("chardata");
    put(links, pending("ch-b2", "AsahiDK", sweeper, now), 0).await?;
    for (id, ign, status, days) in [
        ("ch-b1", "AsahiKanna", LinkStatus::Active, 10),
        ("ch-b3", "AsahiPath", LinkStatus::Stale, 20),
        ("ch-b4", "AsahiBlaze", LinkStatus::Active, 40),
    ] {
        let linked_at = now - Span::days(days);
        let asked = pending(id, ign, Actor::member(ASAHI), linked_at - Span::minutes(10));
        put(links, asked.clone(), 0).await?;
        let confirmed = Link {
            consent_version: Some(NOTICE_VERSION),
            consented_at: Some(linked_at),
            linked_at: Some(linked_at),
            consent_surface: Some(ConsentSurface::Discord),
            ..asked.with_status(status)
        };
        put(links, confirmed, 1).await?;
    }
    // This morning's batch.
    let fetched_at = now - Span::hours(8);
    let stored = links
        .put_snapshot(Snapshot {
            character_id: "ch-b1".into(),
            ocid: "ocid-ch-b1".into(),
            ocid_fetched_at: fetched_at,
            character_name: Some("AsahiKanna".into()),
            world_name: Some("Aquila".into()),
            character_class: Some("Kanna".into()),
            level: Some(286),
            combat_power: Some(51_400_000),
            image_url: None,
            data_date: None,
            fetched_at,
            expires_at: Snapshot::expiry(fetched_at, fetched_at),
        })
        .await?;
    if !stored {
        return Err("AsahiKanna's data was refused".into());
    }
    // Her one card shared in #hstar-party two days ago, still live.
    let card = Post {
        message_id: Some("1424200000000000002".into()),
        ..Post::new(
            "a51c0e".into(),
            HSTAR_PARTY.into(),
            "ch-b1".into(),
            now - Span::days(2),
        )
    };
    if !links.create_post(card).await? {
        return Err("AsahiKanna's share card was refused".into());
    }
    Ok(())
}
