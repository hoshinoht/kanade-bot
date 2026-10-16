//! The harness routes on both origins, ahead of the real routers (which
//! refuse every `/__*` path):
//!
//! - `POST /__e2e/reset`: a restart over a fresh copy of the template, the
//!   clock back at its start (a live clock back to real time), the modules
//!   and Limits groups at their defaults; answers `{now}`.
//! - `POST /__e2e/clock` `{at}` (RFC 3339) or `{advance_s}`: the instant the
//!   writer, the API and module `character_data` read (a live clock keeps
//!   running from there); answers `{now}`.
//! - `GET /__e2e/whoami`: `{server, now, origin, admin_cookie: {name, value},
//!   break_glass: {cookie: {name, value}, token}, tailscale: {login,
//!   edge_secret}}`: Asahi's Discord admin session every template copy holds
//!   (the admin every spec browses as), the break-glass token with the token
//!   session the template holds, and what the admin's trusted edge sends for
//!   a Tailscale sign-in (the browser is that edge's loopback peer).
//! - `POST /__e2e/modules` `{roster?, character_data?, guides?}`: a restart
//!   over the live store with those modules (serve reads them once, at
//!   startup); `character_data` needs `roster`, and `roster: false` turns
//!   both off. Answers the modules now on.
//! - `POST /__e2e/limits` `{groups: "three" | "default"}`: the canned
//!   governor Limits shows: the mock's declared cloud, local and legacy
//!   groups, or the one gateway group (no real flow fills a governor).
//! - `POST /__e2e/public/end`: ends every member-portal session through the
//!   store's session API, as an expiry or a sign-out elsewhere would;
//!   answers `{ended}`.
//! - `POST /__e2e/chat-allowance` `{member, answers}`: spends that many
//!   answers from the member's live chat window (and the guild pool), as if
//!   the chatbot had answered them, so Limits has a window to reset;
//!   204, 409 when the window or pool fills first, 503 before chat starts.
//!   The chat driver counts in monotonic time, so the answers age with the
//!   wall clock, not `/__e2e/clock`.
//! - `POST`/`GET`/`DELETE /__e2e/reports`: the CSP report sink, shared by both
//!   origins; every enforced policy gets `report-uri /__e2e/reports`.
//! - `GET /__e2e/discord/oauth2/authorize`: the fake Discord page.
//! - Admin only: `/__showcase/*`, the core primitives showcase.

use std::sync::{
    Arc, Mutex, PoisonError, RwLock,
    atomic::{AtomicI64, Ordering},
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Request, State, rejection::JsonRejection},
    handler::Handler,
    http::{HeaderValue, StatusCode, header::CONTENT_SECURITY_POLICY},
    middleware::{from_fn_with_state, map_response_with_state},
    response::{IntoResponse, Response},
    routing::{MethodRouter, get, post},
};
use chrono::{DateTime, Datelike, TimeDelta, Timelike, Utc};
use kanade::{
    api::{
        auth::{Clock, system_now},
        guard::headers::CSP,
        listeners::Origin,
    },
    domain::members::MemberStore,
    infrastructure::store::web_sessions::{LoginMethod, SessionOrigin, WebSessionStore},
};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    EDGE_SECRET, Fail, TAILSCALE_LOGIN,
    fakes::discord,
    live::{AdminSessions, Generation, Modules, Routes, World},
};

/// Module `character_data`'s controls (`/__e2e/estimates`,
/// `/__e2e/character-data`, `/__e2e/public/characters`).
#[path = "controls_p5.rs"]
mod p5;

pub const SERVER: &str = "kanade-e2e-server";
const REPORTS: &str = "/__e2e/reports";
/// As the mock: enough for any one test's violations.
const MAX_REPORTS: usize = 500;

/// The one clock every generation reads: pinned (an instant that moves only
/// when set) or live (the wall clock, shifted by whatever `set` moved it).
#[derive(Clone)]
pub struct SharedClock {
    live: bool,
    /// Pinned: the instant; live: the shift from the wall clock. Microseconds.
    micros: Arc<AtomicI64>,
    /// `micros` at startup, which a reset restores.
    start: i64,
}

impl SharedClock {
    pub fn pinned(at: DateTime<Utc>) -> Self {
        Self::with(false, at.timestamp_micros())
    }

    pub fn live() -> Self {
        Self::with(true, 0)
    }

    fn with(live: bool, start: i64) -> Self {
        Self {
            live,
            micros: Arc::new(AtomicI64::new(start)),
            start,
        }
    }

    pub fn now(&self) -> DateTime<Utc> {
        let micros = self.micros.load(Ordering::SeqCst);
        if self.live {
            system_now() + TimeDelta::microseconds(micros)
        } else {
            DateTime::from_timestamp_micros(micros).unwrap_or(DateTime::UNIX_EPOCH)
        }
    }

    fn set(&self, at: DateTime<Utc>) {
        let micros = if self.live {
            (at - system_now()).num_microseconds().unwrap_or(0)
        } else {
            at.timestamp_micros()
        };
        self.micros.store(micros, Ordering::SeqCst);
    }

    fn reset(&self) {
        self.micros.store(self.start, Ordering::SeqCst);
    }

    pub fn clock(&self) -> Clock {
        let this = self.clone();
        Arc::new(move || this.now())
    }
}

/// `2026-09-29T04:00:00Z`, with microseconds only when there are any.
pub fn iso(at: DateTime<Utc>) -> String {
    let mut text = format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        at.year(),
        at.month(),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    );
    match at.timestamp_subsec_micros() {
        0 => text.push('Z'),
        micros => text.push_str(&format!(".{micros:06}Z")),
    }
    text
}

struct Slot {
    serial: u64,
    live: Option<Generation>,
    /// The modules the live generation composed with.
    modules: Modules,
}

pub struct Shared {
    world: World,
    db_dir: std::path::PathBuf,
    template: std::path::PathBuf,
    clock: SharedClock,
    admins: AdminSessions,
    /// The break-glass admin token.
    token: String,
    /// `None` once closed.
    routes: RwLock<Option<Routes>>,
    /// Held across a whole reset, so resets never interleave.
    generation: tokio::sync::Mutex<Slot>,
    reports: Mutex<Vec<Value>>,
    page: Arc<discord::Page>,
    showcase: Router,
    /// The production policy with the report sink.
    csp: HeaderValue,
}

pub struct Start {
    pub world: World,
    pub db_dir: std::path::PathBuf,
    pub template: std::path::PathBuf,
    pub clock: SharedClock,
    pub admins: AdminSessions,
    pub token: String,
    pub page: Arc<discord::Page>,
}

impl Shared {
    /// Serves the first copy of the template.
    pub async fn start(start: Start) -> Result<Self, Fail> {
        let Start {
            world,
            db_dir,
            template,
            clock,
            admins,
            token,
            page,
        } = start;
        let showcase = world.showcase();
        let modules = world.modules();
        let shared = Self {
            world,
            db_dir,
            template,
            clock,
            admins,
            token,
            routes: RwLock::new(None),
            generation: tokio::sync::Mutex::new(Slot {
                serial: 0,
                live: None,
                modules,
            }),
            reports: Mutex::default(),
            page,
            showcase,
            csp: HeaderValue::try_from(format!("{CSP}; report-uri {REPORTS}"))?,
        };
        shared.reset().await?;
        Ok(shared)
    }

    /// Composes over a fresh copy, swaps it in, then retires the old one.
    pub async fn reset(&self) -> Result<(), Fail> {
        let mut slot = self.generation.lock().await;
        self.clock.reset();
        self.world.three_groups.store(false, Ordering::SeqCst);
        self.world.offline.store(false, Ordering::SeqCst);
        p5::reset(&self.world);
        slot.serial += 1;
        let path = self.db_dir.join(format!("live-{}.sqlite3", slot.serial));
        self.world.copy_template(&self.template, &path)?;
        let modules = self.world.modules();
        let fresh = match self.world.open(path.clone(), true, modules).await {
            Ok(fresh) => fresh,
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                return Err(error);
            }
        };
        *self.routes.write().unwrap_or_else(PoisonError::into_inner) =
            Some(self.world.routes(fresh.live()));
        slot.modules = modules;
        if let Some(old) = slot.live.replace(fresh) {
            old.retire().await;
        }
        Ok(())
    }

    /// Restarts over the live store with `modules`, as serve restarts after
    /// a module change. Nothing is served in between: the old routes hold
    /// the store, which must close before it reopens.
    async fn restart(&self, modules: Modules) -> Result<Modules, Fail> {
        let mut slot = self.generation.lock().await;
        let old = slot.live.take().ok_or("nothing is being served")?;
        self.routes
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let path = old.close().await?;
        let fresh = self.world.open(path, true, modules).await?;
        *self.routes.write().unwrap_or_else(PoisonError::into_inner) =
            Some(self.world.routes(fresh.live()));
        slot.live = Some(fresh);
        slot.modules = modules;
        Ok(modules)
    }

    /// Ends every member-portal session of every guild member the store
    /// knows (the seeded roster and the fake Discord sign-in users).
    async fn end_member_sessions(&self) -> Result<u64, Fail> {
        let slot = self.generation.lock().await;
        let store = slot.live.as_ref().ok_or("nothing is being served")?.store();
        let mut ended = 0;
        for profile in store.list_members().await? {
            ended += store
                .delete_subject_sessions(
                    SessionOrigin::Public,
                    LoginMethod::Discord,
                    &profile.member.user_id,
                )
                .await?;
        }
        Ok(ended)
    }

    /// Ends the live generation's event streams (before a graceful drain).
    pub async fn end_streams(&self) {
        if let Some(live) = &self.generation.lock().await.live {
            live.live().state.events.close();
        }
    }

    /// Spends `answers` from the live chat driver's window for `member`;
    /// `None` while chat is not running, `Some(false)` once one did not fit.
    async fn spend_chat(&self, member: &str, answers: usize) -> Option<bool> {
        let slot = self.generation.lock().await;
        let chat = slot.live.as_ref()?.live().state.chat.clone()?;
        for _ in 0..answers {
            if !chat.spend_allowance(member)? {
                return Some(false);
            }
        }
        Some(true)
    }

    /// The live generation, held so no reset or restart swaps it meanwhile.
    pub async fn live(&self) -> Option<tokio::sync::MappedMutexGuard<'_, Generation>> {
        tokio::sync::MutexGuard::try_map(self.generation.lock().await, |slot| slot.live.as_mut())
            .ok()
    }

    /// Stops serving and retires the live generation.
    pub async fn close(&self) {
        self.routes
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(live) = self.generation.lock().await.live.take() {
            live.retire().await;
        }
    }

    fn route(&self, origin: Origin) -> Option<MethodRouter> {
        let routes = self.routes.read().unwrap_or_else(PoisonError::into_inner);
        routes.as_ref().map(|routes| match origin {
            Origin::Admin => routes.admin.clone(),
            Origin::Public => routes.public.clone(),
        })
    }

    fn reports(&self) -> std::sync::MutexGuard<'_, Vec<Value>> {
        self.reports.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Clone)]
struct Side {
    shared: Arc<Shared>,
    origin: Origin,
}

/// One origin's listener: the controls, then the live real router.
pub fn router(shared: &Arc<Shared>, origin: Origin) -> Router {
    let side = Side {
        shared: Arc::clone(shared),
        origin,
    };
    let mut app = Router::new()
        .route("/__e2e/whoami", get(whoami))
        .route("/__e2e/reset", post(reset))
        .route("/__e2e/clock", post(set_clock))
        .route("/__e2e/chat-allowance", post(spend_chat))
        .route("/__e2e/modules", post(set_modules))
        .route("/__e2e/limits", post(set_limits))
        .route("/__e2e/public/end", post(end_member_sessions))
        .route(REPORTS, get(list).post(receive).delete(clear))
        .merge(discord::routes(Arc::clone(&shared.page)))
        .merge(p5::routes())
        .merge(crate::controls_p6::routes(shared))
        .merge(crate::controls_guides::routes(shared))
        .fallback(forward);
    if origin == Origin::Admin {
        app = app.nest_service("/__showcase", shared.showcase.clone());
    }
    app.with_state(side.clone())
        .layer(from_fn_with_state(side.clone(), p5::observe))
        .layer(map_response_with_state(side, report_uri))
}

async fn forward(State(side): State<Side>, request: Request) -> Response {
    match side.shared.route(side.origin) {
        Some(route) => route.call(request, ()).await,
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn report_uri(State(side): State<Side>, mut response: Response) -> Response {
    let headers = response.headers_mut();
    let Some(policy) = headers.get(CONTENT_SECURITY_POLICY) else {
        return response;
    };
    let reporting = if policy == CSP {
        Some(side.shared.csp.clone())
    } else {
        policy.to_str().ok().and_then(|policy| {
            HeaderValue::try_from(format!("{policy}; report-uri {REPORTS}")).ok()
        })
    };
    if let Some(reporting) = reporting {
        headers.insert(CONTENT_SECURITY_POLICY, reporting);
    }
    response
}

fn now_body(at: DateTime<Utc>) -> Json<Value> {
    Json(json!({ "now": iso(at) }))
}

async fn whoami(State(side): State<Side>) -> Json<Value> {
    let shared = &side.shared;
    let cookie = |(name, value): &(String, String)| json!({ "name": name, "value": value });
    Json(json!({
        "server": SERVER,
        "now": iso(shared.clock.now()),
        "origin": match side.origin {
            Origin::Admin => "admin",
            Origin::Public => "public",
        },
        "admin_cookie": cookie(&shared.admins.discord),
        "break_glass": { "cookie": cookie(&shared.admins.token), "token": shared.token },
        "tailscale": { "login": TAILSCALE_LOGIN, "edge_secret": EDGE_SECRET },
    }))
}

async fn reset(State(side): State<Side>) -> Response {
    match side.shared.reset().await {
        Ok(()) => now_body(side.shared.clock.now()).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChatSpend {
    member: String,
    answers: usize,
}

async fn spend_chat(
    State(side): State<Side>,
    body: Result<Json<ChatSpend>, JsonRejection>,
) -> Response {
    let Ok(Json(spend)) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match side.shared.spend_chat(&spend.member, spend.answers).await {
        None => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Some(false) => (StatusCode::CONFLICT, "the window or the pool is full").into_response(),
        Some(true) => StatusCode::NO_CONTENT.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModulesChange {
    roster: Option<bool>,
    character_data: Option<bool>,
    guides: Option<bool>,
}

async fn set_modules(
    State(side): State<Side>,
    body: Result<Json<ModulesChange>, JsonRejection>,
) -> Response {
    let Ok(Json(change)) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let current = side.shared.generation.lock().await.modules;
    let roster = change.roster.unwrap_or(current.roster);
    let character_data = match change.character_data {
        Some(true) if !roster => return StatusCode::UNPROCESSABLE_ENTITY.into_response(),
        Some(on) => on,
        None => roster && current.character_data,
    };
    let modules = Modules {
        roster,
        character_data,
        guides: change.guides.unwrap_or(current.guides),
    };
    match side.shared.restart(modules).await {
        Ok(modules) => Json(json!({
            "roster": modules.roster,
            "character_data": modules.character_data,
            "guides": modules.guides,
        }))
        .into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsChange {
    groups: String,
}

async fn set_limits(
    State(side): State<Side>,
    body: Result<Json<LimitsChange>, JsonRejection>,
) -> StatusCode {
    let Ok(Json(change)) = body else {
        return StatusCode::BAD_REQUEST;
    };
    let three = match change.groups.as_str() {
        "three" => true,
        "default" => false,
        _ => return StatusCode::UNPROCESSABLE_ENTITY,
    };
    side.shared
        .world
        .three_groups
        .store(three, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

async fn end_member_sessions(State(side): State<Side>) -> Response {
    match side.shared.end_member_sessions().await {
        Ok(ended) => Json(json!({ "ended": ended })).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClockChange {
    at: Option<String>,
    advance_s: Option<i64>,
}

async fn set_clock(
    State(side): State<Side>,
    body: Result<Json<ClockChange>, JsonRejection>,
) -> Response {
    let Ok(Json(change)) = body else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let clock = &side.shared.clock;
    let at = match (change.at, change.advance_s) {
        (Some(at), None) => DateTime::parse_from_rfc3339(&at)
            .ok()
            .map(|at| at.with_timezone(&Utc)),
        (None, Some(seconds)) => {
            TimeDelta::try_seconds(seconds).and_then(|delta| clock.now().checked_add_signed(delta))
        }
        _ => None,
    };
    let Some(at) = at else {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    };
    clock.set(at);
    now_body(at).into_response()
}

/// `report-uri` bodies (`application/csp-report`) or Reporting API arrays.
async fn receive(State(side): State<Side>, body: Bytes) -> StatusCode {
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return StatusCode::BAD_REQUEST;
    };
    let items = match value {
        Value::Array(items) => items,
        other => vec![other],
    };
    let mut reports = side.shared.reports();
    for item in items {
        eprintln!("csp-report {item}");
        if reports.len() < MAX_REPORTS {
            reports.push(item);
        }
    }
    StatusCode::NO_CONTENT
}

async fn list(State(side): State<Side>) -> Json<Vec<Value>> {
    Json(side.shared.reports().clone())
}

async fn clear(State(side): State<Side>) -> StatusCode {
    side.shared.reports().clear();
    StatusCode::NO_CONTENT
}
