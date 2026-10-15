//! Scouter estimates (module `character_data` with
//! `KANADE_SCOUTER_URL`) over the seeded store of `reads.rs` (`r-kalos`
//! XKalos with 1001, 1002 and 1004) and a fake scouter sidecar on loopback
//! with invented IGNs and numbers. The fake answers by IGN prefix: `Busy`,
//! `Limited` (429), `Crit` (422), `Verify` (503), `Blocked`, `Mismatch`,
//! `Extract`, `Failed` (502), `Slow` (504), `Hang` (past the client
//! timeout), `Gate` (waits to be released), `Null` (unknown shares),
//! `Once` (429 busy with `Retry-After: 1` the first time), `Assumed`
//! (critical rate 89% assumed at 100%); anything else is a full result.

use std::{
    collections::HashSet,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use kanade::infrastructure::{
    character_data::{CharacterData, ConfirmOutcome},
    scouter::{MAX_QUEUED, TIMEOUT},
    store::SqliteCharData,
};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::{
    character_data::{config, confirm, maker},
    readiness::slot as ready_slot,
    reads::Reads,
    schemas::assert_valid,
    support::{fields, items, spawn_router, strs},
};

const JOB: &str = "character_data.json#/$defs/EstimateJob";
const STARTED: &str = "character_data.json#/$defs/EstimateStarted";
const STATUS: &str = "character_data.json#/$defs/EstimateStatus";
const ERROR: &str = "error.json#/$defs/ApiError";
const CONFIG: &str = "config.json#/$defs/ConfigView";
const ESTIMATES: &str = "/api/admin/estimates";
/// The client timeout under test (production: 190 s).
const CLIENT_TIMEOUT: Duration = Duration::from_millis(400);

/// One request the fake received.
#[derive(Clone, Debug)]
pub(crate) struct Seen {
    /// Path and query exactly as sent.
    target: String,
    headers: Vec<(String, String)>,
}

#[derive(Clone, Default)]
pub(crate) struct Fake {
    seen: Arc<Mutex<Vec<Seen>>>,
    gate: Arc<Notify>,
    /// `Once…` IGNs already answered busy.
    refused: Arc<Mutex<HashSet<String>>>,
}

impl Fake {
    /// The lookups received (`/healthz` probes left out).
    pub(crate) fn lookups(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| seen.target.starts_with("/v1/"))
            .cloned()
            .collect()
    }

    pub(crate) fn targets(&self) -> Vec<String> {
        self.lookups().into_iter().map(|seen| seen.target).collect()
    }
}

fn card(percentage: f64, rating: &str, party: bool, effective: Option<f64>) -> Value {
    json!({
        "hexa_converted": 100_000,
        "percentage": percentage,
        "rating": rating,
        "party": party,
        "benchmark_players": effective.map(|share| (percentage / share).round()),
        "count_source": if effective.is_some() { "rating" } else { "unknown" },
        "effective_percentage": effective,
        "estimated_seconds": effective.map(|share| 180_000.0 / share),
        "estimated_minutes": effective.map(|share| 3_000.0 / share),
    })
}

/// An invented member answer; `Null…` has an unknown Star share,
/// `Assumed…` a critical rate of 89% assumed at 100%.
fn member(ign: &str) -> Value {
    let star = if ign.starts_with("Null") {
        card(64.5, "Party-able", true, None)
    } else {
        card(64.5, "3p Min Cut", true, Some(21.5))
    };
    let mut answer = json!({
        "boss_380_hexa": 100_000,
        "requested_preset": "00010",
        "cache_hit": false,
        "ign": ign,
        "region": "msea",
        "preset": "00010",
        "timer_minutes": 20,
        "baseline_minutes": 30,
        "fetched_at": "2026-09-29T04:00:00Z",
        "bosses": {
            "extreme_kalos": card(54.0, "6p Min Cut", true, Some(9.0)),
            "hard_maleficStar": star,
            // Not in the test catalog, and not a catalog difficulty: dropped.
            "hard_limbo": card(72.4, "Party-able", false, Some(72.4)),
            "destiny_kalos": card(3.0, "Impossible", false, Some(3.0)),
        },
        "timings_ms": {"total": 1},
    });
    if ign.starts_with("Assumed") {
        answer["critical_rate_assumed"] = json!(true);
        answer["critical_rate_actual"] = json!(89);
        answer["source"] = json!("manual_input");
    }
    answer
}

fn total(sum: Option<f64>, issues: Value) -> Value {
    json!({
        "combined_percentage": sum,
        "estimated_seconds": sum.map(|sum| 180_000.0 / sum),
        "estimated_minutes": sum.map(|sum| 3_000.0 / sum),
        "technically_possible": sum.map(|sum| sum >= 95.0),
        "complete": sum.is_some(),
        "issues": issues,
    })
}

fn batch(igns: &[&str]) -> Value {
    let count = igns.len() as f64;
    let unknown: serde_json::Map<String, Value> = igns
        .iter()
        .filter(|ign| ign.starts_with("Null"))
        .map(|ign| ((*ign).to_owned(), json!("unknown_benchmark_players")))
        .collect();
    let star = if unknown.is_empty() {
        total(Some(21.5 * count), json!({}))
    } else {
        total(None, Value::Object(unknown))
    };
    json!({
        "region": "msea",
        "requested_preset": "00010",
        "preset": "00010",
        "baseline_minutes": 30,
        "possible_threshold_percentage": 95,
        "members": igns.iter().map(|ign| member(ign)).collect::<Vec<_>>(),
        "combined": {
            "extreme_kalos": total(Some(9.0 * count), json!({})),
            "hard_maleficStar": star,
        },
        "timings_ms": {"total": 1},
    })
}

/// The scouter's error answer for `ign`, by its prefix.
fn refusal(ign: &str) -> Option<Response> {
    let (status, code, retry) = [
        ("Busy", 429, "busy", Some("5")),
        ("Limited", 429, "upstream_rate_limited", Some("40")),
        ("Crit", 422, "critical_rate_setup_required", None),
        ("Verify", 503, "verification_required", None),
        ("Blocked", 502, "upstream_blocked", None),
        ("Mismatch", 502, "result_mismatch", None),
        ("Extract", 502, "extraction_failed", None),
        ("Failed", 502, "lookup_failed", None),
        ("Slow", 504, "lookup_timeout", None),
    ]
    .into_iter()
    .find(|(prefix, ..)| ign.starts_with(prefix))
    .map(|(_, status, code, retry)| (status, code, retry))?;
    let body = if code == "busy" {
        json!({"error": {"code": code, "message": "A browser lookup is already running; retry later."}})
    } else {
        json!({"error": {"code": code, "message": format!("{ign}: invented"), "ign": ign, "requested_preset": "00010", "preset_attempts": []}})
    };
    let mut response = (StatusCode::from_u16(status).unwrap(), Json(body)).into_response();
    if let Some(retry) = retry {
        response
            .headers_mut()
            .insert("retry-after", retry.parse().unwrap());
    }
    Some(response)
}

async fn bosses(State(fake): State<Fake>, request: Request) -> Response {
    let query = request.uri().query().unwrap_or_default().to_owned();
    let names: Vec<String> = query
        .split_once('=')
        .map(|(_, value)| value.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    for name in &names {
        if name.starts_with("Hang") {
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
        // The party total (`igns=`) answers from the sidecar's cache.
        if name.starts_with("Gate") && query.starts_with("ign=") {
            fake.gate.notified().await;
        }
        let huge = name.starts_with("Huge");
        if (huge || name.starts_with("Once")) && fake.refused.lock().unwrap().insert(name.clone()) {
            let body = json!({"error": {"code": "busy", "message": "4 lookups are waiting; retry later."}});
            let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
            response.headers_mut().insert(
                "retry-after",
                if huge { "4294967295" } else { "1" }.parse().unwrap(),
            );
            return response;
        }
        if let Some(refused) = refusal(name) {
            return refused;
        }
    }
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    if query.starts_with("igns=") {
        Json(batch(&names)).into_response()
    } else {
        Json(member(names.first().copied().unwrap_or_default())).into_response()
    }
}

pub(crate) async fn fake_scouter() -> (SocketAddr, Fake) {
    let fake = Fake::default();
    let recorded = fake.clone();
    let app = Router::new()
        .route(
            "/healthz",
            get(|| async { Json(json!({"status": "ok", "region": "msea"})) }),
        )
        .route("/v1/bosses", get(bosses))
        .layer(axum::middleware::from_fn(
            move |request: Request, next: axum::middleware::Next| {
                let recorded = recorded.clone();
                async move {
                    recorded.seen.lock().unwrap().push(Seen {
                        target: request
                            .uri()
                            .path_and_query()
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                        headers: request
                            .headers()
                            .iter()
                            .map(|(name, value)| {
                                (
                                    name.as_str().to_owned(),
                                    value.to_str().unwrap_or_default().to_owned(),
                                )
                            })
                            .collect(),
                    });
                    next.run(request).await
                }
            },
        ))
        .with_state(fake.clone());
    (spawn_router(app).await, fake)
}

/// A fake Open API for linking (invented data; the Nexon name differs in
/// case from the roster IGN on purpose: linking corrects the roster IGN).
async fn fake_nexon() -> SocketAddr {
    let app = Router::new().fallback(|request: Request| async move {
        let path = request.uri().path().to_owned();
        if path.ends_with("/v1/id") {
            r#"{"ocid":"ocid-invented-estimate-1"}"#.to_owned()
        } else if path.ends_with("/character/basic") {
            r#"{"date":"2026-09-28T00:00+08:00","character_name":"MIKABISH","world_name":"Aquila","character_class":"Bishop","character_level":283,"character_image":"https://open.api.nexon.com/static/invented"}"#.to_owned()
        } else {
            r#"{"final_stat":[{"stat_name":"Combat Power","stat_value":"48213377"}]}"#.to_owned()
        }
    });
    spawn_router(app).await
}

/// The module with estimates at `scouter` (off with `None`) under the
/// client `timeout`, and the Open API at `nexon` when given: the reads and
/// the service.
async fn serve(
    scouter: Option<SocketAddr>,
    nexon: Option<SocketAddr>,
    timeout: Duration,
) -> (Reads, Arc<CharacterData>) {
    let slot = Arc::new(std::sync::OnceLock::new());
    let file = Arc::new(SqliteCharData::open_temp().await);
    let scouter = scouter.map(|address| (address, timeout));
    let module = maker(file, nexon, scouter, Arc::default(), Arc::clone(&slot));
    let reads = Reads::with_character_data(Some(config()), module).await;
    (reads, slot.get().unwrap().clone())
}

async fn on(scouter: Option<SocketAddr>) -> Reads {
    serve(scouter, None, CLIENT_TIMEOUT).await.0
}

/// [`on`] for tests that hold a `Gate…` lookup until they release it: the
/// client timeout is far past any runner's pace, so the held lookup never
/// fails on its own and the line moves only when the test says so.
async fn on_gated(scouter: SocketAddr) -> Reads {
    serve(Some(scouter), None, Duration::from_secs(120)).await.0
}

async fn character(reads: &Reads, member: &str, ign: &str) -> String {
    let path = format!("/api/admin/members/{member}/characters");
    let reply = reads.call("POST", &path, json!({"ign": ign}), &[]).await;
    assert_eq!(reply.status, 201, "{}", reply.text());
    reply.json()["id"].as_str().unwrap().to_owned()
}

/// `POST` → `202 {job}`, schema-checked.
async fn post(reads: &Reads, body: Value) -> Value {
    let reply = reads.call("POST", ESTIMATES, body, &[]).await;
    reply.expect(202, STARTED)["job"].clone()
}

async fn refused(reads: &Reads, body: Value) -> (u16, String) {
    let reply = reads.call("POST", ESTIMATES, body, &[]).await;
    assert_valid(ERROR, ESTIMATES, &reply.json());
    reply.error()
}

/// The job once it has finished (`done` or `failed`).
async fn settled(reads: &Reads, job: &Value) -> Value {
    read_until(reads, job, |read| {
        read["state"] == "done" || read["state"] == "failed"
    })
    .await
}

/// Job `job` once `ready` holds for it, still unfinished or not.
async fn read_until(reads: &Reads, job: &Value, ready: impl Fn(&Value) -> bool) -> Value {
    let path = format!("{ESTIMATES}/{}", job["id"].as_str().unwrap());
    for _ in 0..400 {
        let read = reads.read(&path, JOB).await;
        if ready(&read) {
            return read;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{path} never got there: {}", reads.read(&path, JOB).await);
}

async fn look_up(reads: &Reads, body: Value) -> Value {
    let job = post(reads, body).await;
    settled(reads, &job).await
}

fn tokens(member: &Value) -> Vec<&str> {
    strs(&member["bosses"], "boss")
}

/// The status read for `query` (`run_id=…` or `character_ids=…`).
async fn status(reads: &Reads, query: &str) -> Value {
    reads.read(&format!("{ESTIMATES}?{query}"), STATUS).await
}

/// A `GET` under [`ESTIMATES`] that may be refused.
async fn fetch(reads: &Reads, rest: &str) -> crate::support::Reply {
    let path = format!("{ESTIMATES}{rest}");
    reads.call("GET", &path, json!(null), &[]).await
}

/// An XKalos party total the run's lookup reports as incomplete.
fn incomplete_kalos(sum: f64) -> Value {
    json!([{
        "boss": "XKalos",
        "combined_percentage": sum,
        "estimated_minutes": 3000.0 / sum,
        "technically_possible": false,
        "complete": false,
        "issues": [],
    }])
}

#[tokio::test]
async fn routes_are_the_generic_404_while_the_module_or_the_url_is_off() {
    for reads in [Reads::with_characters().await, on(None).await] {
        for (method, path) in [
            ("GET", format!("{ESTIMATES}?run_id=r-kalos")),
            ("POST", ESTIMATES.to_owned()),
            ("GET", format!("{ESTIMATES}/any")),
        ] {
            let body = json!({"run_id": "r-kalos"});
            reads.assert_unmounted(method, &path, body).await;
        }
    }
    // On without a URL: Config says the scouter is off.
    let reads = on(None).await;
    let view = reads.read("/api/admin/config", CONFIG).await;
    assert_eq!(
        view["character_data"]["scouter"],
        json!({"state": "off", "url": null, "last_lookup_at": null, "last_error": null, "last_error_at": null})
    );
}

#[tokio::test]
async fn one_character_is_one_ign_lookup_with_a_pinned_request() {
    assert_eq!(TIMEOUT, Duration::from_secs(190), "the production timeout");
    let (address, fake) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let id = character(&reads, "1001", "MikaBish").await;

    let running = post(&reads, json!({"character_ids": [id]})).await;
    assert_eq!(
        running["state"], "running",
        "nothing ahead: it runs at once"
    );
    assert_eq!(running["position"], Value::Null);
    assert_eq!(running["started_by"]["kind"], "admin");
    assert_eq!(
        running["progress"],
        json!({"done": 0, "total": 1, "current": null})
    );
    assert_eq!(running["characters"][0]["outcome"], "pending");
    let done = settled(&reads, &running).await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["label"], "Clear-time estimate");
    assert_eq!(done["note"], "For planning only, not eligibility");
    assert_eq!(done["character_ids"], json!([id]));
    assert_eq!(done["run_id"], Value::Null);
    assert_eq!(done["fetched_at"], "2026-09-29T04:00:00Z");
    let result = &done["result"];
    assert_eq!(result["cache_hit"], false);
    assert_eq!(result["complete"], true);
    assert_eq!(
        result["combined"],
        Value::Null,
        "no total for one character"
    );
    let mika = &result["members"][0];
    assert_eq!(
        fields(mika, &["character_id", "ign", "preset"]),
        json!([id, "MikaBish", "00010"])
    );
    assert_eq!(
        fields(mika, &["critical_rate_assumed", "critical_rate_actual"]),
        json!([false, null]),
        "the normal page"
    );
    assert_eq!(
        done["characters"][0],
        json!({"character_id": id, "member_id": "1001", "ign": "MikaBish", "outcome": "ok", "error": null})
    );
    // Catalog bosses only, in catalog order; Limbo and Destiny dropped.
    assert_eq!(tokens(mika), ["XKalos", "HMaleficStar"]);
    assert_eq!(
        mika["bosses"][0],
        json!({"boss": "XKalos", "percentage": 54.0, "effective_percentage": 9.0, "estimated_minutes": 3000.0 / 9.0, "rating": "6p Min Cut", "party": true})
    );

    // The request: exactly `GET /v1/bosses?ign=<roster IGN>`, no preset,
    // nothing but Host, Accept and User-Agent.
    let lookups = fake.lookups();
    assert_eq!(lookups.len(), 1);
    assert_eq!(lookups[0].target, "/v1/bosses?ign=MikaBish");
    let mut names: Vec<&str> = lookups[0]
        .headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["accept", "host", "user-agent"]);

    // The status read finds it again (closed and reopened).
    let status_read = status(&reads, &format!("character_ids={id}")).await;
    assert_eq!(status_read["job"]["id"], done["id"]);
    assert_eq!(status_read["running"], Value::Null);
    let none = status(&reads, "character_ids=nobody").await;
    assert_eq!(none, json!({"job": null, "running": null}));

    // The 24-hour memory answers at once, without the sidecar.
    let cached = post(&reads, json!({"character_ids": [id]})).await;
    assert_eq!(cached["state"], "done");
    assert_eq!(cached["result"]["cache_hit"], true);
    assert_eq!(cached["fetched_at"], done["fetched_at"]);
    assert_eq!(fake.lookups().len(), 1);
    // "Look up again" skips it.
    let again = look_up(&reads, json!({"character_ids": [id], "refresh": true})).await;
    assert_eq!(again["result"]["cache_hit"], false);
    assert_eq!(fake.targets(), ["/v1/bosses?ign=MikaBish"; 2]);

    let gone = fetch(&reads, "/nope").await;
    assert_eq!(gone.error(), (404, "not_found".into()));

    // Config's health block.
    let view = reads.read("/api/admin/config", CONFIG).await;
    let scouter = &view["character_data"]["scouter"];
    assert_eq!(scouter["state"], "ok");
    assert_eq!(scouter["url"], format!("http://{address}"));
    assert_eq!(scouter["last_lookup_at"], "2026-09-29T04:00:00Z");
    assert_eq!(scouter["last_error"], Value::Null);
}

#[tokio::test]
async fn post_needs_csrf_and_a_valid_body() {
    let (address, fake) = fake_scouter().await;
    let reads = on(Some(address)).await;
    // Alice's main (her first character) is on GMS.
    let gms = json!({"ign": "MikaGms", "world": "gms"});
    let path = "/api/admin/members/1001/characters";
    let gms = reads.call("POST", path, gms, &[]).await;
    let gms = gms.json()["id"].as_str().unwrap().to_owned();
    let id = character(&reads, "1001", "MikaBish").await;
    let body = json!({"character_ids": [id]});
    let forged = reads.without_csrf("POST", ESTIMATES, &body).await;
    assert_eq!(forged.error(), (403, "csrf".into()));

    for (body, status, code) in [
        (json!({}), 422, "invalid"),
        (
            json!({"character_ids": [id], "run_id": "r-kalos"}),
            422,
            "invalid",
        ),
        (json!({"character_ids": []}), 422, "invalid"),
        (
            json!({"character_ids": ["a", "b", "c", "d", "e", "f", "g"]}),
            422,
            "invalid",
        ),
        (json!({"character_ids": ["nobody"]}), 404, "not_found"),
        (json!({"run_id": "r-gone"}), 404, "not_found"),
        (
            json!({"run_id": "r-kalos", "preset": "00110"}),
            400,
            "invalid_body",
        ),
    ] {
        assert_eq!(
            refused(&reads, body.clone()).await,
            (status, code.into()),
            "{body}"
        );
    }
    assert_eq!(
        refused(&reads, json!({"character_ids": [gms]})).await,
        (422, "region".into())
    );
    // Alice brings her GMS main and nobody else in r-kalos has a character.
    assert_eq!(
        refused(&reads, json!({"run_id": "r-kalos"})).await,
        (422, "nothing_to_look_up".into())
    );
    let bad = fetch(&reads, "?run_id=a&character_ids=b").await;
    assert_eq!(bad.error(), (422, "invalid".into()));
    assert!(fake.lookups().is_empty(), "nothing was looked up");
}

#[tokio::test]
async fn scouter_errors_map_to_the_drawn_states_and_are_never_cached() {
    let (address, fake) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let busy = [
        ("BusyAa", "busy", Some(5)),
        ("LimitedAa", "upstream_rate_limited", Some(40)),
    ]
    .map(|(ign, detail, retry)| {
        let message = "The scouter is busy.";
        (ign, "busy", Some(429), detail, retry, message)
    });
    let failed = [
        ("BlockedAa", Some(502), "upstream_blocked"),
        ("MismatchAa", Some(502), "result_mismatch"),
        ("ExtractAa", Some(502), "extraction_failed"),
        ("FailedAa", Some(502), "lookup_failed"),
        ("SlowAa", Some(504), "lookup_timeout"),
        ("HangAa", None, "timeout"),
    ]
    .map(|(ign, status, detail)| (ign, "failed", status, detail, None, "Lookup failed."));
    let cases = busy.into_iter().chain([
        (
            "CritAa",
            "critical_rate",
            Some(422),
            "critical_rate_setup_required",
            None,
            "Set up critical rate on the scouter first.",
        ),
        (
            "VerifyAa",
            "manual_check",
            Some(503),
            "verification_required",
            None,
            "Scouter needs a manual check.",
        ),
    ]);
    // HangAa (a timeout) last: the success below keeps its error.
    for (ign, code, status, detail, retry, message) in cases.chain(failed) {
        let id = character(&reads, "1001", ign).await;
        let failed = look_up(&reads, json!({"character_ids": [id]})).await;
        assert_eq!(failed["state"], "failed", "{ign}: {failed}");
        assert_eq!(failed["result"], Value::Null);
        assert_eq!(failed["fetched_at"], Value::Null);
        assert_eq!(
            failed["error"],
            json!({
                "code": code,
                "message": message,
                "status": status,
                "detail": detail,
                "retry_after_seconds": retry,
                "character_id": id,
            }),
            "{ign}"
        );
        assert_eq!(failed["characters"][0]["outcome"], "failed", "{ign}");
        assert_eq!(failed["characters"][0]["error"], failed["error"], "{ign}");
        if ign == "VerifyAa" {
            let view = reads.read("/api/admin/config", CONFIG).await;
            assert_eq!(
                view["character_data"]["scouter"]["state"],
                "verification_required"
            );
            assert_eq!(
                view["character_data"]["scouter"]["last_error"],
                "verification_required"
            );
        }
        // Not cached: the next request calls the sidecar again.
        let before = fake.lookups().len();
        let again = look_up(&reads, json!({"character_ids": [id]})).await;
        assert_eq!(again["state"], "failed");
        assert_eq!(fake.lookups().len(), before + 1, "{ign}");
    }
    // A successful lookup clears verification_required.
    let id = character(&reads, "1002", "MikaBish").await;
    look_up(&reads, json!({"character_ids": [id]})).await;
    let view = reads.read("/api/admin/config", CONFIG).await;
    assert_eq!(view["character_data"]["scouter"]["state"], "ok");
    assert_eq!(view["character_data"]["scouter"]["last_error"], "timeout");
}

#[tokio::test]
async fn unknown_shares_stay_null() {
    let (address, _) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let id = character(&reads, "1001", "NullAa").await;
    let done = look_up(&reads, json!({"character_ids": [id]})).await;
    let star = &done["result"]["members"][0]["bosses"][1];
    assert_eq!(
        *star,
        json!({"boss": "HMaleficStar", "percentage": 64.5, "effective_percentage": null, "estimated_minutes": null, "rating": "Party-able", "party": true})
    );
}

#[tokio::test]
async fn a_run_is_looked_up_one_by_one_then_as_a_party_with_missing_members() {
    let (address, fake) = fake_scouter().await;
    let reads = on(Some(address)).await;
    // Alice picks MikaBish over her main MikaHero, Bob brings his main
    // (his first character), Dan has none.
    character(&reads, "1001", "MikaHero").await;
    let mika = character(&reads, "1001", "MikaBish").await;
    let ren = character(&reads, "1002", "RenShade").await;
    let pick = json!({"character_id": mika, "version": 0});
    let path = "/api/admin/runs/r-kalos/characters/1001";
    let picked = reads.call("PUT", path, pick, &[]).await;
    assert_eq!(picked.status, 200, "{}", picked.text());

    let done = look_up(&reads, json!({"run_id": "r-kalos"})).await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["run_id"], "r-kalos");
    assert_eq!(done["character_ids"], json!([]));
    assert_eq!(
        done["characters"],
        json!([
            {"character_id": mika, "member_id": "1001", "ign": "MikaBish", "outcome": "ok", "error": null},
            {"character_id": ren, "member_id": "1002", "ign": "RenShade", "outcome": "ok", "error": null},
        ])
    );
    assert_eq!(
        done["missing"],
        json!([{"member_id": "1004", "character_id": null, "reason": "no_characters"}])
    );
    assert_eq!(
        done["progress"],
        json!({"done": 2, "total": 2, "current": null})
    );
    assert_eq!(
        fake.targets(),
        [
            "/v1/bosses?ign=MikaBish",
            "/v1/bosses?ign=RenShade",
            "/v1/bosses?igns=MikaBish,RenShade",
        ]
    );
    let result = &done["result"];
    assert_eq!(result["complete"], false, "a member is missing");
    // The run's boss only, never 0% for the missing member.
    assert_eq!(result["combined"], incomplete_kalos(18.0));
    assert_eq!(result["members"].as_array().unwrap().len(), 2);

    // Cached by the party's IGN set, and each character by its own.
    let status = status(&reads, "run_id=r-kalos").await;
    assert_eq!(status["job"]["id"], done["id"]);
    let again = post(&reads, json!({"run_id": "r-kalos"})).await;
    assert_eq!(
        (&again["state"], &again["result"]["cache_hit"]),
        (&json!("done"), &json!(true))
    );
    let strip = post(&reads, json!({"character_ids": [ren]})).await;
    assert_eq!(strip["result"]["cache_hit"], true);
    assert_eq!(fake.lookups().len(), 3, "no new lookups");

    // Bob gains a second character: he still brings his main, so the
    // party's IGN set, and its memory, stay the same.
    character(&reads, "1002", "RenAlt").await;
    let later = look_up(&reads, json!({"run_id": "r-kalos"})).await;
    assert_eq!(later["characters"], done["characters"]);
    assert_eq!(later["missing"], done["missing"]);
    assert_eq!(later["result"]["cache_hit"], true);
    assert_eq!(fake.lookups().len(), 3, "still no new lookups");
}

#[tokio::test]
async fn a_party_with_an_unknown_share_has_no_total_and_says_why() {
    let (address, _) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let mika = character(&reads, "1001", "MikaBish").await;
    let unknown = character(&reads, "1002", "NullAa").await;
    let done = look_up(&reads, json!({"character_ids": [unknown, mika]})).await;
    let combined = done["result"]["combined"].as_array().unwrap();
    assert_eq!(
        combined.len(),
        2,
        "every catalog boss for several characters"
    );
    assert_eq!(combined[0]["boss"], "XKalos");
    assert_eq!(combined[0]["complete"], true);
    assert_eq!(
        combined[1],
        json!({
            "boss": "HMaleficStar",
            "combined_percentage": null,
            "estimated_minutes": null,
            "technically_possible": null,
            "complete": false,
            "issues": [{"character_id": unknown, "issue": "unknown_benchmark_players"}],
        })
    );
    assert_eq!(done["result"]["complete"], false);
    let mut ids = vec![unknown, mika];
    ids.sort();
    assert_eq!(done["character_ids"], json!(ids));
}

#[tokio::test]
async fn a_party_member_failure_never_stops_the_others() {
    let (address, fake) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let mika = character(&reads, "1001", "MikaBish").await;
    let crit = character(&reads, "1002", "CritAa").await;
    let done = look_up(&reads, json!({"run_id": "r-kalos"})).await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["error"], Value::Null, "the total itself came back");
    assert_eq!(
        fields(&done, &["characters/0/outcome", "characters/0/error"]),
        json!(["ok", null])
    );
    let failed = &done["characters"][1];
    assert_eq!(
        fields(failed, &["outcome", "error/code", "error/character_id"]),
        json!(["failed", "critical_rate", crit])
    );
    assert_eq!(done["progress"]["done"], 2);
    let result = &done["result"];
    assert_eq!(result["members"].as_array().unwrap().len(), 1);
    assert_eq!(result["members"][0]["character_id"], mika);
    // The total counts MikaBish only and is never complete.
    assert_eq!(result["combined"], incomplete_kalos(9.0));
    assert_eq!(result["complete"], false);
    assert_eq!(
        fake.targets(),
        [
            "/v1/bosses?ign=MikaBish",
            "/v1/bosses?ign=CritAa",
            "/v1/bosses?igns=MikaBish",
        ]
    );
    // MikaBish was kept; the partial party was not.
    let strip = post(&reads, json!({"character_ids": [mika]})).await;
    assert_eq!(strip["result"]["cache_hit"], true);
    let again = look_up(&reads, json!({"run_id": "r-kalos"})).await;
    assert_eq!(again["result"]["cache_hit"], false);
    assert_eq!(
        fake.targets()[3..],
        ["/v1/bosses?ign=CritAa", "/v1/bosses?igns=MikaBish"],
        "only the failed character is asked again"
    );
}

#[tokio::test]
async fn a_party_with_nobody_found_fails_with_each_reason() {
    let (address, fake) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let crit = character(&reads, "1001", "CritAa").await;
    let slow = character(&reads, "1002", "SlowAa").await;
    let failed = look_up(&reads, json!({"character_ids": [crit, slow]})).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["result"], Value::Null, "no total from nobody");
    let codes: Vec<(&Value, &Value)> = failed["characters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (&row["ign"], &row["error"]["detail"]))
        .collect();
    assert!(codes.contains(&(&json!("CritAa"), &json!("critical_rate_setup_required"))));
    assert!(codes.contains(&(&json!("SlowAa"), &json!("lookup_timeout"))));
    // The job's error is the first character's.
    assert_eq!(failed["error"], failed["characters"][0]["error"]);
    assert_eq!(fake.lookups().len(), 2, "no party total asked");
}

#[tokio::test]
async fn party_results_fill_in_as_each_character_finishes() {
    let (address, fake) = fake_scouter().await;
    let reads = on_gated(address).await;
    let mika = character(&reads, "1001", "MikaBish").await;
    let gate = character(&reads, "1002", "GateAa").await;
    let job = post(&reads, json!({"run_id": "r-kalos"})).await;
    let partial = read_until(&reads, &job, |read| read["progress"]["current"] == gate).await;
    assert_eq!(partial["state"], "running");
    assert_eq!(
        partial["progress"],
        json!({"done": 1, "total": 2, "current": gate})
    );
    assert_eq!(
        fields(&partial, &["characters/0/outcome", "characters/1/outcome"]),
        json!(["ok", "pending"])
    );
    let result = &partial["result"];
    assert_eq!(result["members"][0]["character_id"], mika);
    assert_eq!(result["members"].as_array().unwrap().len(), 1);
    assert_eq!(
        fields(result, &["combined", "complete", "cache_hit"]),
        json!([null, false, false])
    );
    assert_eq!(partial["fetched_at"], Value::Null, "until it is done");
    // Readiness already shows MikaBish's reading.
    let ready = reads.read("/api/admin/runs/r-kalos/readiness", READY).await;
    assert_eq!(
        ready_slot(&ready, "1001")["bosses"][0]["scouter"]["percentage"],
        9.0
    );
    fake.gate.notify_one();
    let done = settled(&reads, &job).await;
    assert_eq!(done["state"], "done");
    assert_eq!(done["result"]["members"].as_array().unwrap().len(), 2);
    assert_eq!(done["result"]["combined"][0]["combined_percentage"], 18.0);
}

#[tokio::test]
async fn lookups_wait_in_line_in_order_with_their_position() {
    let (address, fake) = fake_scouter().await;
    let reads = on_gated(address).await;
    let gate = character(&reads, "1001", "GateAa").await;
    let mika = character(&reads, "1002", "MikaBish").await;
    let ren = character(&reads, "1001", "RenShade").await;
    let running = post(&reads, json!({"character_ids": [gate]})).await;
    assert_eq!(running["state"], "running");
    let first = post(&reads, json!({"character_ids": [mika]})).await;
    let second = post(&reads, json!({"character_ids": [ren]})).await;
    assert_eq!(
        (&first["state"], &first["position"]),
        (&json!("queued"), &json!(1))
    );
    assert_eq!(
        (&second["state"], &second["position"]),
        (&json!("queued"), &json!(2))
    );
    assert_eq!(
        first["progress"],
        json!({"done": 0, "total": 1, "current": null})
    );
    // Asking again for a queued or running target answers its job.
    let joined = post(&reads, json!({"character_ids": [mika], "refresh": true})).await;
    assert_eq!(
        (&joined["id"], &joined["position"]),
        (&first["id"], &json!(1))
    );
    let same = post(&reads, json!({"character_ids": [gate]})).await;
    assert_eq!(
        (&same["id"], &same["state"]),
        (&running["id"], &json!("running"))
    );
    // The status read reports the place too, and the job ahead.
    let status_read = status(&reads, &format!("character_ids={ren}")).await;
    assert_eq!(
        (&status_read["job"]["id"], &status_read["job"]["position"]),
        (&second["id"], &json!(2))
    );
    assert_eq!(status_read["running"]["id"], running["id"]);
    assert!(
        fake.targets()
            .iter()
            .all(|target| target.ends_with("GateAa")),
        "only the running one is asked"
    );

    fake.gate.notify_one();
    let last = settled(&reads, &second).await;
    assert_eq!(
        (&last["state"], &last["position"]),
        (&json!("done"), &Value::Null)
    );
    assert_eq!(settled(&reads, &first).await["state"], "done");
    assert_eq!(
        fake.targets(),
        [
            "/v1/bosses?ign=GateAa",
            "/v1/bosses?ign=MikaBish",
            "/v1/bosses?ign=RenShade",
        ],
        "first in, first looked up"
    );
    assert_eq!(
        status(&reads, &format!("character_ids={ren}")).await["running"],
        Value::Null,
        "the worker stops once the line is empty"
    );
    // A later lookup starts a new worker.
    let id = character(&reads, "1002", "AsaAa").await;
    assert_eq!(
        look_up(&reads, json!({"character_ids": [id]})).await["state"],
        "done"
    );
}

#[tokio::test]
async fn the_line_holds_16_and_refuses_more() {
    assert_eq!(MAX_QUEUED, 16);
    let (address, fake) = fake_scouter().await;
    let reads = on_gated(address).await;
    let gate = character(&reads, "1001", "GateAa").await;
    post(&reads, json!({"character_ids": [gate]})).await;
    let mut queued = Vec::new();
    for n in 0..MAX_QUEUED {
        let id = character(&reads, "1002", &format!("Mika{n:02}")).await;
        let job = post(&reads, json!({"character_ids": [id]})).await;
        assert_eq!(job["position"], json!(n + 1));
        queued.push((id, job));
    }
    let late = character(&reads, "1002", "MikaLate").await;
    let reply = reads
        .call("POST", ESTIMATES, json!({"character_ids": [late]}), &[])
        .await;
    assert_valid(ERROR, ESTIMATES, &reply.json());
    assert_eq!(reply.error(), (429, "estimate_queue_full".into()));
    assert_eq!(
        reply.json()["message"],
        "16 lookups are waiting already. Try again in a few minutes."
    );
    // A target already in line still gets its job.
    let (id, job) = &queued[3];
    assert_eq!(
        post(&reads, json!({"character_ids": [id]})).await["id"],
        job["id"]
    );
    fake.gate.notify_one();
    let (_, last) = queued.last().unwrap();
    assert_eq!(settled(&reads, last).await["state"], "done");
    assert_eq!(
        look_up(&reads, json!({"character_ids": [late]})).await["state"],
        "done"
    );
}

#[tokio::test]
async fn busy_is_asked_again_after_retry_after_within_the_deadline() {
    let (address, fake) = fake_scouter().await;
    // One call's budget (3 s) leaves room for the 1-second wait.
    let (reads, _) = serve(Some(address), None, Duration::from_secs(3)).await;
    let id = character(&reads, "1001", "OnceAa").await;
    let begun = Instant::now();
    let done = look_up(&reads, json!({"character_ids": [id]})).await;
    assert_eq!(done["state"], "done", "{done}");
    assert!(
        begun.elapsed() >= Duration::from_secs(1),
        "waited Retry-After"
    );
    assert_eq!(fake.targets(), ["/v1/bosses?ign=OnceAa"; 2]);
    let view = reads.read("/api/admin/config", CONFIG).await;
    assert_eq!(view["character_data"]["scouter"]["state"], "ok");
    // A Retry-After past the deadline (5 s against 0.4 s) still fails as
    // busy: covered by `scouter_errors_map_to_the_drawn_states…`.
}

/// A Retry-After far past any deadline (`u32::MAX` seconds) fails that
/// lookup as busy and never stalls the line: the next lookup still runs.
#[tokio::test]
async fn a_huge_retry_after_fails_as_busy_and_the_line_moves_on() {
    let (address, _) = fake_scouter().await;
    let (reads, _) = serve(Some(address), None, Duration::from_secs(3)).await;
    let huge = character(&reads, "1001", "HugeAa").await;
    let failed = look_up(&reads, json!({"character_ids": [huge]})).await;
    assert_eq!(failed["state"], "failed", "{failed}");
    let next = character(&reads, "1002", "PlainBb").await;
    let done = look_up(&reads, json!({"character_ids": [next]})).await;
    assert_eq!(done["state"], "done", "{done}");
}

#[tokio::test]
async fn an_assumed_critical_rate_is_labelled_in_the_result_and_readiness() {
    let (address, _) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let id = character(&reads, "1001", "AssumedAa").await;
    let done = look_up(&reads, json!({"character_ids": [id]})).await;
    let member = &done["result"]["members"][0];
    assert_eq!(
        fields(member, &["critical_rate_assumed", "critical_rate_actual"]),
        json!([true, 89.0])
    );
    assert_eq!(done["note"], "For planning only, not eligibility");
    // Readiness carries the same label for each of the character's bosses.
    let ready = reads.read("/api/admin/runs/r-kalos/readiness", READY).await;
    assert_eq!(
        ready_slot(&ready, "1001")["bosses"][0]["scouter"],
        json!({
            "percentage": 9.0,
            "effective_percentage": 9.0,
            "rating": "6p Min Cut",
            "estimated_minutes": 3000.0 / 9.0,
            "fetched_at": "2026-09-29T04:00:00Z",
            "critical_rate_assumed": true,
            "critical_rate_actual": 89.0,
        })
    );
}

#[tokio::test]
async fn scouter_requests_never_carry_nexon_fields() {
    let (address, fake) = fake_scouter().await;
    let nexon = fake_nexon().await;
    let (reads, service) = serve(Some(address), Some(nexon), CLIENT_TIMEOUT).await;
    // The create already asked the member to link it (pending).
    let id = character(&reads, "1001", "MikaBish").await;
    // Created by the admin: linked at once.
    let ConfirmOutcome::AlreadyLinked = confirm(&service, "1001", &id).await else {
        panic!("not linked");
    };
    let linked = reads
        .read(
            &format!("/api/admin/characters/{id}/link"),
            "character_data.json#/$defs/CharacterLink",
        )
        .await;
    assert_eq!(linked["data"]["name"], "MIKABISH");

    let done = look_up(&reads, json!({"character_ids": [id]})).await;
    assert_eq!(done["state"], "done");
    let sent = format!("{:?}", fake.seen.lock().unwrap());
    // The roster IGN, which took NEXON's letter case when the link fetched
    // (user decision 2026-10-13); nothing else from NEXON.
    assert!(sent.contains("ign=MIKABISH"), "the roster IGN: {sent}");
    for nexon_field in [
        "ocid",
        "invented-estimate",
        "48213377",
        "283",
        "Bishop",
        "Aquila",
        "preset",
        "x-nxopen-api-key",
        "invented-key",
    ] {
        assert!(!sent.contains(nexon_field), "{nexon_field} in {sent}");
    }
    assert!(
        !done.to_string().contains("48213377"),
        "the job carries no Nexon data"
    );
}

#[tokio::test]
async fn health_says_unreachable_when_nothing_listens() {
    // A port that was free a moment ago.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let reads = on(Some(closed)).await;
    let view = reads.read("/api/admin/config", CONFIG).await;
    let scouter = &view["character_data"]["scouter"];
    assert_eq!(scouter["state"], "unreachable");
    assert_eq!(scouter["last_error"], "unreachable");
    let id = character(&reads, "1001", "MikaBish").await;
    let failed = look_up(&reads, json!({"character_ids": [id]})).await;
    assert_eq!(
        fields(&failed, &["error/code", "error/detail", "error/status"]),
        json!(["failed", "unreachable", null])
    );
}

#[tokio::test]
async fn jobs_stay_24_hours_one_per_target_past_many_lookups() {
    let (address, _) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let mut first = Value::Null;
    let mut ids = Vec::new();
    // More targets than the old 32-job cap.
    for n in 0..40 {
        let id = character(&reads, "1001", &format!("Mika{n:02}")).await;
        let done = look_up(&reads, json!({"character_ids": [id]})).await;
        if n == 0 {
            first = done;
        }
        ids.push(id);
    }
    let path = format!("/{}", first["id"].as_str().unwrap());
    let kept = reads.read(&format!("{ESTIMATES}{path}"), JOB).await;
    assert_eq!(kept["state"], "done", "the first job is still there");
    // A repeat lookup of a target replaces its job.
    let again = post(&reads, json!({"character_ids": [ids[0]]})).await;
    assert_ne!(again["id"], first["id"]);
    let replaced = fetch(&reads, &path).await;
    assert_eq!(replaced.error(), (404, "not_found".into()));
    let status = status(&reads, &format!("character_ids={}", ids[0])).await;
    assert_eq!(status["job"]["id"], again["id"]);
}

const READY: &str = "character_data.json#/$defs/RunReadiness";

/// Readiness (`r-kalos` XKalos; `r-star` HMaleficStar + HLucid with Alice)
/// shows each character's cached scouter reading per boss, from a
/// lookup of them alone or of a run that included them, and never looks
/// anything up itself. NEXON combat power stays as it was.
#[tokio::test]
async fn readiness_shows_cached_scouter_readings_per_boss_and_never_looks_up() {
    const KALOS: &str = "/api/admin/runs/r-kalos/readiness";
    const STAR: &str = "/api/admin/runs/r-star/readiness";
    let (address, fake) = fake_scouter().await;
    let nexon = fake_nexon().await;
    let (reads, service) = serve(Some(address), Some(nexon), CLIENT_TIMEOUT).await;
    // Alice brings RenShade (pending, no data), Bob MikaBish (linked), Dan
    // nobody. A MapleSEA create already asks for the link.
    let ren = character(&reads, "1001", "RenShade").await;
    let mika = character(&reads, "1002", "MikaBish").await;
    let ConfirmOutcome::AlreadyLinked = confirm(&service, "1002", &mika).await else {
        panic!("not linked");
    };
    let nexon_cp = json!({"kind": "nexon_cp", "value": 48_213_377, "as_of": "2026-09-28"});

    // Nothing looked up yet: no readings, and reading readiness starts nothing.
    let before = reads.read(KALOS, READY).await;
    for member in ["1001", "1002", "1004"] {
        assert_eq!(
            ready_slot(&before, member)["bosses"][0]["scouter"],
            Value::Null,
            "{member}"
        );
    }
    assert!(fake.lookups().is_empty());
    assert_eq!(
        status(&reads, "run_id=r-kalos").await,
        json!({"job": null, "running": null})
    );

    // RenShade alone: her reading per boss; MikaBish keeps NEXON CP only.
    look_up(&reads, json!({"character_ids": [ren]})).await;
    let mixed = reads.read(KALOS, READY).await;
    let alice = ready_slot(&mixed, "1001");
    assert_eq!(
        alice["bosses"][0]["scouter"],
        json!({"percentage": 9.0, "effective_percentage": 9.0, "rating": "6p Min Cut", "estimated_minutes": 3000.0 / 9.0, "fetched_at": "2026-09-29T04:00:00Z", "critical_rate_assumed": false, "critical_rate_actual": null})
    );
    assert_eq!(alice["power"], Value::Null);
    let bob = ready_slot(&mixed, "1002");
    assert_eq!(bob["power"], nexon_cp);
    assert_eq!(bob["bosses"][0]["scouter"], Value::Null);
    // Mapped bosses only: Star has a card; Lucid has none at the scouter.
    let star = reads.read(STAR, READY).await;
    let shares: Vec<&Value> = items(&ready_slot(&star, "1001")["bosses"])
        .iter()
        .map(|boss| &boss["scouter"]["percentage"])
        .collect();
    assert_eq!(shares, [&json!(21.5), &Value::Null]);

    // The run's lookup includes MikaBish: her reading too, CP still sent.
    look_up(&reads, json!({"run_id": "r-kalos"})).await;
    let after = reads.read(KALOS, READY).await;
    let bob = ready_slot(&after, "1002");
    assert_eq!(bob["bosses"][0]["scouter"]["percentage"], 9.0);
    assert_eq!(bob["power"], nexon_cp);
    assert_eq!(
        ready_slot(&after, "1004")["bosses"][0]["scouter"],
        Value::Null
    );
    let lookups = fake.lookups().len();
    reads.read(KALOS, READY).await;
    reads.read(STAR, READY).await;
    assert_eq!(fake.lookups().len(), lookups, "readiness never looks up");
    assert_eq!(
        status(&reads, "run_id=r-kalos").await["running"],
        Value::Null
    );
}

const PREVIEW: &str = "reminders.json#/$defs/ReminderPreview";

/// The admin `GET` of `path`: the raw reply (the PNG is no JSON).
async fn raw_get(reads: &Reads, path: &str) -> crate::support::Reply {
    crate::support::request(
        reads.admin,
        "GET",
        crate::support::ADMIN_HOST,
        path,
        &[("Cookie", &reads.cookie)],
    )
    .await
}

/// The reminder preview carries the redesigned countdown's "Who carries"
/// image (`m-kalos-15`, `r-kalos` 22:00 XKalos), drawn at its own route by
/// the delivery chart code from the estimates cached now, never a lookup.
/// Classic cards, estimates off and unknown reminders have none.
#[tokio::test]
async fn the_reminder_preview_shows_the_estimate_image_from_cached_readings() {
    const CARD: &str = "/api/admin/reminders/m-kalos-15/preview";
    const PNG: &str = "/api/admin/reminders/m-kalos-15/estimate.png";
    let redesigned = json!({"notifications": {"message_style": "redesigned"}});
    let (address, fake) = fake_scouter().await;
    let reads = on(Some(address)).await;

    // Classic (the default): no image and no PNG.
    let classic = reads.read(CARD, PREVIEW).await;
    assert_eq!(classic["card"]["estimate"], Value::Null);
    assert_eq!(
        raw_get(&reads, PNG).await.error(),
        (404, "not_found".into())
    );

    let saved = reads.call("PATCH", "/api/admin/config", redesigned.clone(), &[]);
    assert_eq!(saved.await.status, 200);
    // Nobody has a character yet: drawn as "no estimate yet".
    let none = reads.read(CARD, PREVIEW).await;
    assert_eq!(
        fields(&none["card"]["estimate"], &["url", "alt", "width"]),
        json!([
            PNG,
            "22:00 XKalos: XKalos no estimate yet. As of Tue 29 Sep 12:00.",
            560
        ])
    );

    // Alice brings MikaBish, looked up alone; Bob and Dan bring nobody.
    let mika = character(&reads, "1001", "MikaBish").await;
    look_up(&reads, json!({"character_ids": [mika]})).await;
    let lookups = fake.lookups().len();
    let preview = reads.read(CARD, PREVIEW).await;
    let image = &preview["card"]["estimate"];
    assert_eq!(
        image["alt"],
        "22:00 XKalos: XKalos 9.0%, MikaBish 100%; 2 with no estimate. As of Tue 29 Sep 12:00."
    );
    assert!(image["height"].as_u64().unwrap() > 0);
    let png = raw_get(&reads, PNG).await;
    assert_eq!(png.status, 200, "{}", png.text());
    assert_eq!(png.header("content-type"), Some("image/png"));
    assert_eq!(png.header("cache-control"), Some("no-store"));
    assert!(png.body.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(fake.lookups().len(), lookups, "cached readings only");

    // Admin only, and an unknown reminder has no image.
    assert_eq!(reads.status(PNG, false).await.0, 401);
    let unknown = raw_get(&reads, "/api/admin/reminders/nope/estimate.png").await;
    assert_eq!(unknown.error(), (404, "not_found".into()));

    // Estimates off (no scouter URL): the card stays, without the image.
    let off = on(None).await;
    let saved = off.call("PATCH", "/api/admin/config", redesigned, &[]);
    assert_eq!(saved.await.status, 200);
    let preview = off.read(CARD, PREVIEW).await;
    assert!(preview["card"]["content"].is_string());
    assert_eq!(preview["card"]["estimate"], Value::Null);
    assert_eq!(raw_get(&off, PNG).await.error(), (404, "not_found".into()));
}

/// The live report: a card drawn while the party's lookup had found only
/// its first members showed the rest as "no estimate". While a lookup of
/// anyone in the party is queued or running the image is left off; once
/// it finishes the image has every member.
#[tokio::test]
async fn the_reminder_preview_has_no_estimate_image_while_its_party_is_looked_up() {
    const CARD: &str = "/api/admin/reminders/m-kalos-15/preview";
    const PNG: &str = "/api/admin/reminders/m-kalos-15/estimate.png";
    let (address, fake) = fake_scouter().await;
    let reads = on_gated(address).await;
    let redesigned = json!({"notifications": {"message_style": "redesigned"}});
    let saved = reads.call("PATCH", "/api/admin/config", redesigned, &[]);
    assert_eq!(saved.await.status, 200);
    character(&reads, "1001", "MikaBish").await;
    let gate = character(&reads, "1002", "GateAa").await;

    // MikaBish is found and cached; GateAa's lookup is still running.
    let job = post(&reads, json!({"run_id": "r-kalos"})).await;
    read_until(&reads, &job, |read| read["progress"]["current"] == gate).await;
    let running = reads.read(CARD, PREVIEW).await;
    assert!(running["card"]["content"].is_string(), "the card stays");
    assert_eq!(running["card"]["estimate"], Value::Null);
    assert_eq!(
        raw_get(&reads, PNG).await.error(),
        (404, "not_found".into())
    );

    fake.gate.notify_one();
    assert_eq!(settled(&reads, &job).await["state"], "done");
    let done = reads.read(CARD, PREVIEW).await;
    assert_eq!(
        done["card"]["estimate"]["alt"],
        "22:00 XKalos: XKalos 18.0%, MikaBish 50%, GateAa 50%; 1 with no estimate. As of Tue 29 Sep 12:00."
    );
    assert_eq!(raw_get(&reads, PNG).await.status, 200);
}
