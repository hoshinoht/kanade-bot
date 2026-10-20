//! Module `character_data`'s outside world: the scouter sidecar
//! (`KANADE_SCOUTER_URL`) and the NEXON Open API (`Overrides::nexon`), each
//! a loopback server whose answers follow [`Modes`], which the lane's
//! controls (`controls_p5`) set and a reset puts back.
//!
//! The scouter finds every character with the same four boss cards
//! (scaled per IGN) and totals a batch by adding the cards' shares. Its
//! [`Scenario`]s: `ok` (each lookup takes [`PACE`]), `paced` (each takes
//! [`SLOW_PACE`], longer than the page's poll, so a party fills in one
//! member at a time), `assumed` (one IGN's result assumes 100% critical
//! rate), `hold` (no lookup answers until the scenario changes), and the
//! error scenarios, where the next single lookup fails and the rest are
//! found: `busy` (429, `Retry-After: 40`), `critical_rate` (422),
//! `assumption_failed` (422 `critical_rate_assumption_failed`),
//! `manual_check` (503 `verification_required`), `failed` (502),
//! `timeout` (504). The lookups [`Modes::hold_igns`] names wait until a
//! lookup is queued behind them, then finish one by one.
//!
//! NEXON answers by IGN: `Nope…` is no such name (`400 OPENAPI00004`, as
//! live MapleSEA answers `/id`), `Busy…` is maintenance (503), `Later…`
//! resolves but its data is still being prepared (`OPENAPI00009`), any
//! other name is a level 270-ish character. An ocid NEXON never issued
//! (the seed's) is `OPENAPI00003`, so it is resolved again. [`Nexon`]
//! makes every call fail instead.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{RawQuery, State},
    http::{StatusCode, header::RETRY_AFTER},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;

/// How long an `ok` lookup takes.
const PACE: Duration = Duration::from_millis(300);
/// How long a `paced` lookup takes: past the page's 1.5 s poll.
const SLOW_PACE: Duration = Duration::from_millis(2_000);
/// Between a lookup queued behind the held ones and each one finishing.
const RELEASE_STEP: Duration = Duration::from_secs(3);
/// An assumed result's own critical rate.
const ACTUAL_CRIT: f64 = 89.0;
/// `(scouter key, %, share of a party, rating, party-only)`.
const CARDS: [(&str, f64, f64, &str, bool); 4] = [
    ("extreme_kalos", 54.0, 9.0, "6p Min Cut", true),
    ("hard_kaling", 18.0, 18.0, "Party-able", false),
    ("hard_maleficStar", 64.5, 21.5, "3p Min Cut", true),
    ("hard_limbo", 72.4, 72.4, "Party-able", false),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scenario {
    Ok,
    Paced,
    Assumed,
    Hold,
    Busy,
    CriticalRate,
    AssumptionFailed,
    ManualCheck,
    Failed,
    Timeout,
}

impl Scenario {
    /// The HTTP status, `Retry-After` and scouter code of an error scenario.
    fn error(self) -> Option<(StatusCode, Option<u32>, &'static str)> {
        Some(match self {
            // Not `busy`: kanade asks a `busy` again after its Retry-After
            // while the job's deadline allows, so it would never show.
            Self::Busy => (StatusCode::TOO_MANY_REQUESTS, Some(40), "rate_limited"),
            Self::CriticalRate => (
                StatusCode::UNPROCESSABLE_ENTITY,
                None,
                "critical_rate_setup_required",
            ),
            Self::AssumptionFailed => (
                StatusCode::UNPROCESSABLE_ENTITY,
                None,
                "critical_rate_assumption_failed",
            ),
            Self::ManualCheck => (
                StatusCode::SERVICE_UNAVAILABLE,
                None,
                "verification_required",
            ),
            Self::Failed => (StatusCode::BAD_GATEWAY, None, "lookup_failed"),
            Self::Timeout => (StatusCode::GATEWAY_TIMEOUT, None, "lookup_timeout"),
            Self::Ok | Self::Paced | Self::Assumed | Self::Hold => return None,
        })
    }
}

/// How every NEXON call answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nexon {
    /// By IGN (see the module docs).
    Ok,
    /// Maintenance: 503 with no error name.
    Down,
    /// The key is refused: `OPENAPI00002`.
    Refused,
}

/// What a restart composes module `character_data` with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Setup {
    /// No `nexon_api_key` in the secrets directory.
    pub no_key: bool,
    /// No `KANADE_SCOUTER_URL`.
    pub no_scouter: bool,
}

struct Inner {
    scenario: Scenario,
    /// The IGN key an `assumed` result is for.
    assumed: Option<String>,
    /// An error scenario still owes its one failed lookup.
    fail_next: bool,
    /// IGN keys whose lookups wait; the first goes [`RELEASE_STEP`] after a
    /// lookup is queued behind them, each next one a step later.
    held: Vec<String>,
    nexon: Nexon,
    /// `/healthz` answers 503 (the scouter is unreachable to kanade).
    scouter_down: bool,
    setup: Setup,
    /// Ocids issued by `/id`, and the name each is for.
    ocids: HashMap<String, String>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            scenario: Scenario::Ok,
            assumed: None,
            fail_next: false,
            held: Vec::new(),
            nexon: Nexon::Ok,
            scouter_down: false,
            setup: Setup::default(),
            ocids: HashMap::new(),
        }
    }
}

/// The fakes' shared state; each change wakes the lookups waiting on it.
pub struct Modes {
    state: Mutex<Inner>,
    changed: watch::Sender<u64>,
}

impl Default for Modes {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            changed: watch::Sender::new(0),
        }
    }
}

impl Modes {
    fn state(&self) -> MutexGuard<'_, Inner> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn change(&self, edit: impl FnOnce(&mut Inner)) {
        edit(&mut self.state());
        self.changed.send_modify(|count| *count += 1);
    }

    /// Resolves once `ready` holds.
    async fn until(&self, ready: impl Fn(&Inner) -> bool) {
        let mut changed = self.changed.subscribe();
        loop {
            changed.borrow_and_update();
            if ready(&self.state()) {
                return;
            }
            if changed.changed().await.is_err() {
                return;
            }
        }
    }

    /// Everything back as after startup (a reset), held lookups let go.
    pub fn reset(&self) {
        self.change(|state| *state = Inner::default());
    }

    /// Later lookups follow `scenario`; `assumed` names the IGN an
    /// `assumed` result is for.
    pub fn set_scenario(&self, scenario: Scenario, assumed: Option<&str>) {
        self.change(|state| {
            state.scenario = scenario;
            state.assumed = assumed.map(str::to_ascii_lowercase);
            state.fail_next = scenario.error().is_some();
        });
    }

    /// Lookups of these IGNs wait for [`Self::queued_behind`].
    pub fn hold_igns(&self, igns: &[&str]) {
        self.change(|state| state.held = igns.iter().map(|ign| ign.to_ascii_lowercase()).collect());
    }

    /// A lookup was queued: the held ones finish one by one.
    pub fn queued_behind(self: &Arc<Self>) {
        if self.state().held.is_empty() {
            return;
        }
        let modes = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(RELEASE_STEP).await;
                let mut left = true;
                modes.change(|state| {
                    if !state.held.is_empty() {
                        state.held.remove(0);
                    }
                    left = !state.held.is_empty();
                });
                if !left {
                    return;
                }
            }
        });
    }

    pub fn set_nexon(&self, nexon: Nexon) {
        self.change(|state| state.nexon = nexon);
    }

    pub fn set_scouter_down(&self, down: bool) {
        self.change(|state| state.scouter_down = down);
    }

    pub fn set_setup(&self, setup: Setup) {
        self.change(|state| state.setup = setup);
    }

    pub fn setup(&self) -> Setup {
        self.state().setup
    }
}

pub fn scouter(modes: Arc<Modes>) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/bosses", get(bosses))
        .with_state(modes)
}

pub fn nexon(modes: Arc<Modes>) -> Router {
    Router::new()
        .route("/maplestorysea/v1/id", get(id))
        .route("/maplestorysea/v1/character/basic", get(basic))
        .route("/maplestorysea/v1/character/stat", get(stat))
        .with_state(modes)
}

async fn healthz(State(modes): State<Arc<Modes>>) -> StatusCode {
    if modes.state().scouter_down {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    }
}

/// One query value (IGNs and ocids are plain ASCII: nothing to decode).
fn param<'a>(query: Option<&'a str>, key: &str) -> Option<&'a str> {
    query?
        .split('&')
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
}

/// `ign`'s cards, scaled by its name so party members differ.
fn cards(ign: &str) -> Vec<(&'static str, f64, f64, &'static str, bool)> {
    let scale = 1.0 + 0.15 * f64::from(u8::try_from(ign.len() % 4).unwrap_or(0));
    let round = |value: f64| (value * scale * 100.0).round() / 100.0;
    CARDS
        .iter()
        .map(|&(key, percentage, share, rating, party)| {
            (key, round(percentage), round(share), rating, party)
        })
        .collect()
}

async fn bosses(State(modes): State<Arc<Modes>>, RawQuery(query): RawQuery) -> Response {
    let query = query.as_deref();
    let single = param(query, "ign");
    let batch = param(query, "igns");
    modes.until(|state| state.scenario != Scenario::Hold).await;
    if let Some(ign) = single {
        let key = ign.to_ascii_lowercase();
        modes.until(|state| !state.held.contains(&key)).await;
    }
    let (pace, failure, assumed) = {
        let mut state = modes.state();
        let pace = if state.scenario == Scenario::Paced {
            SLOW_PACE
        } else {
            PACE
        };
        let failure = match state.scenario.error() {
            Some(error) if single.is_some() && state.fail_next => {
                state.fail_next = false;
                Some(error)
            }
            _ => None,
        };
        let assumed = state.scenario == Scenario::Assumed
            && single
                .is_some_and(|ign| state.assumed.as_deref() == Some(&*ign.to_ascii_lowercase()));
        (pace, failure, assumed)
    };
    tokio::time::sleep(pace).await;
    if let Some((status, retry_after, code)) = failure {
        let body =
            Json(json!({"error": {"code": code, "message": "e2e scouter error", "ign": single}}));
        return match retry_after {
            Some(seconds) => (status, [(RETRY_AFTER, seconds.to_string())], body).into_response(),
            None => (status, body).into_response(),
        };
    }
    match (single, batch) {
        (Some(ign), _) => {
            let bosses: serde_json::Map<String, Value> = cards(ign)
                .into_iter()
                .map(|(key, percentage, share, rating, party)| {
                    (
                        key.to_owned(),
                        json!({
                            "percentage": percentage,
                            "rating": rating,
                            "party": party,
                            "effective_percentage": share,
                            "estimated_minutes": ((3000.0 / share) * 100.0).round() / 100.0,
                        }),
                    )
                })
                .collect();
            Json(json!({
                "ign": ign,
                "preset": "00010",
                "critical_rate_assumed": assumed,
                "critical_rate_actual": assumed.then_some(ACTUAL_CRIT),
                "source": assumed.then_some("manual_input"),
                "bosses": bosses,
            }))
            .into_response()
        }
        (None, Some(igns)) => {
            let members: Vec<&str> = igns.split(',').collect();
            let combined: serde_json::Map<String, Value> = CARDS
                .iter()
                .enumerate()
                .map(|(at, (key, ..))| {
                    let sum: f64 = members.iter().map(|ign| cards(ign)[at].2).sum();
                    let sum = (sum * 100.0).round() / 100.0;
                    (
                        (*key).to_owned(),
                        json!({
                            "combined_percentage": sum,
                            "estimated_minutes": ((3000.0 / sum) * 100.0).round() / 100.0,
                            "technically_possible": sum >= 95.0,
                            "complete": true,
                            "issues": {},
                        }),
                    )
                })
                .collect();
            Json(json!({"combined": combined})).into_response()
        }
        (None, None) => StatusCode::BAD_REQUEST.into_response(),
    }
}

/// An Open API error answer naming `code`.
fn refused(status: StatusCode, code: &str) -> Response {
    (
        status,
        Json(json!({"error": {"name": code, "message": "e2e"}})),
    )
        .into_response()
}

/// The answer every call gets while [`Nexon`] is not `Ok`.
fn outage(modes: &Modes) -> Option<Response> {
    match modes.state().nexon {
        Nexon::Ok => None,
        Nexon::Down => Some(StatusCode::SERVICE_UNAVAILABLE.into_response()),
        Nexon::Refused => Some(refused(StatusCode::BAD_REQUEST, "OPENAPI00002")),
    }
}

fn starts(name: &str, prefix: &str) -> bool {
    name.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

async fn id(State(modes): State<Arc<Modes>>, RawQuery(query): RawQuery) -> Response {
    if let Some(answer) = outage(&modes) {
        return answer;
    }
    let Some(name) = param(query.as_deref(), "character_name") else {
        return refused(StatusCode::BAD_REQUEST, "OPENAPI00004");
    };
    if starts(name, "Nope") {
        return refused(StatusCode::BAD_REQUEST, "OPENAPI00004");
    }
    if starts(name, "Busy") {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let ocid = format!("e2e-{}", name.to_ascii_lowercase());
    modes.state().ocids.insert(ocid.clone(), name.to_owned());
    Json(json!({"ocid": ocid})).into_response()
}

/// The name an issued ocid is for; `None` answers `OPENAPI00003`.
fn named(modes: &Modes, query: Option<&str>) -> Option<String> {
    let ocid = param(query, "ocid")?;
    modes.state().ocids.get(ocid).cloned()
}

async fn basic(State(modes): State<Arc<Modes>>, RawQuery(query): RawQuery) -> Response {
    if let Some(answer) = outage(&modes) {
        return answer;
    }
    let Some(name) = named(&modes, query.as_deref()) else {
        return refused(StatusCode::BAD_REQUEST, "OPENAPI00003");
    };
    if starts(&name, "Later") {
        return refused(StatusCode::BAD_REQUEST, "OPENAPI00009");
    }
    let level = 270 + name.len() % 10;
    Json(json!({
        "date": "2026-09-29T00:00+08:00",
        "character_name": name,
        "world_name": "Aquila",
        "character_class": if name.ends_with("Kain") { "Kain" } else { "Bishop" },
        "character_level": level,
        "character_image": "https://open.api.nexon.com/static/maplestorysea/character/look/e2e",
    }))
    .into_response()
}

async fn stat(State(modes): State<Arc<Modes>>, RawQuery(query): RawQuery) -> Response {
    if let Some(answer) = outage(&modes) {
        return answer;
    }
    let Some(name) = named(&modes, query.as_deref()) else {
        return refused(StatusCode::BAD_REQUEST, "OPENAPI00003");
    };
    let power = 40_000_000 + 1_234_567 * name.len();
    Json(json!({"final_stat": [{"stat_name": "Combat Power", "stat_value": power.to_string()}]}))
        .into_response()
}
