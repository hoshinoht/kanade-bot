//! MapleScouter estimates (module `character_data` with
//! `KANADE_SCOUTER_URL`) over the seeded store of `reads.rs` (`r-kalos`
//! XKalos with 1001, 1002 and 1004) and a fake scouter sidecar on loopback
//! with invented IGNs and numbers. The fake answers by IGN prefix: `Busy`,
//! `Limited` (429), `Crit` (422), `Verify` (503), `Blocked`, `Mismatch`,
//! `Extract`, `Failed` (502), `Slow` (504), `Hang` (past the client
//! timeout), `Gate` (waits to be released), `Null` (unknown shares);
//! anything else is a full result.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use kanade::{
    api::admin::config::{ConfigDesk, ConfigFacts, ConfigInputs},
    domain::{chardata::NOTICE_VERSION, settings::RuntimeSettings},
    infrastructure::{
        character_data::{CharacterData, ConfirmOutcome, RosterSide, Timing},
        nexon::{ApiKey, NexonClient},
        scouter::{Estimates, ScouterClient, TIMEOUT},
        store::MemoryCharData,
    },
};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::{
    reads::{CharacterDataMaker, ConfigMaker, Reads},
    schemas::assert_valid,
    support::{ADMIN_HOST, send},
};

const JOB: &str = "character_data.json#/$defs/EstimateJob";
const STARTED: &str = "character_data.json#/$defs/EstimateStarted";
const STATUS: &str = "character_data.json#/$defs/EstimateStatus";
const ERROR: &str = "error.json#/$defs/ApiError";
const CONFIG: &str = "config.json#/$defs/ConfigView";
const ESTIMATES: &str = "/api/admin/estimates";
/// The client timeout under test (production: 130 s).
const CLIENT_TIMEOUT: Duration = Duration::from_millis(400);

/// One request the fake received.
#[derive(Clone, Debug)]
struct Seen {
    /// Path and query exactly as sent.
    target: String,
    headers: Vec<(String, String)>,
}

#[derive(Clone, Default)]
struct Fake {
    seen: Arc<Mutex<Vec<Seen>>>,
    gate: Arc<Notify>,
}

impl Fake {
    /// The lookups received (`/healthz` probes left out).
    fn lookups(&self) -> Vec<Seen> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|seen| seen.target.starts_with("/v1/"))
            .cloned()
            .collect()
    }

    fn targets(&self) -> Vec<String> {
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

/// An invented member answer; `Null…` has an unknown Star share.
fn member(ign: &str) -> Value {
    let star = if ign.starts_with("Null") {
        card(64.5, "Party-able", true, None)
    } else {
        card(64.5, "3p Min Cut", true, Some(21.5))
    };
    json!({
        "boss_380_hexa": 100_000,
        "requested_preset": "00010",
        "preset_fallback_used": false,
        "preset_attempts": [],
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
    })
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

/// MapleScouter's error answer for `ign`, by its prefix.
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
        if name.starts_with("Gate") {
            fake.gate.notified().await;
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

async fn fake_scouter() -> (SocketAddr, Fake) {
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (address, fake)
}

/// A fake Open API for linking (invented data; the Nexon name differs in
/// case from the roster IGN on purpose).
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    address
}

fn config() -> ConfigMaker {
    Box::new(|store| {
        Arc::new(ConfigDesk::new(ConfigInputs {
            settings: RuntimeSettings::default(),
            store,
            models: None,
            facts: ConfigFacts {
                timezone: "Asia/Kuala_Lumpur".into(),
                model_gateway: None,
                model_permits: 2,
                model_groups: Vec::new(),
                chat_pilot_role_id: None,
            },
            personas: None,
        }))
    })
}

/// The module with estimates at `scouter` (off with `None`), and the Open
/// API at `nexon` when given; the service is handed back through `slot`.
fn module(
    scouter: Option<SocketAddr>,
    nexon: Option<SocketAddr>,
    slot: Arc<std::sync::OnceLock<Arc<CharacterData>>>,
) -> CharacterDataMaker {
    Box::new(move |store, clock| {
        let mut service = CharacterData::new(
            Arc::new(MemoryCharData::new()),
            store as Arc<dyn RosterSide>,
            nexon.map(|address| NexonClient::plain(address, ApiKey::new("invented-key"))),
            Arc::clone(&clock),
            Timing {
                spacing: Duration::from_millis(1),
                rate_limit_unit: Duration::from_millis(1),
            },
        );
        if let Some(address) = scouter {
            let url = format!("http://{address}");
            let client = ScouterClient::new(&url)
                .unwrap()
                .with_timeout(CLIENT_TIMEOUT);
            service = service.with_estimates(Estimates::new(client, url, clock));
        }
        let service = Arc::new(service);
        let _ = slot.set(Arc::clone(&service));
        service
    })
}

async fn on(scouter: Option<SocketAddr>) -> Reads {
    Reads::with_character_data(Some(config()), module(scouter, None, Arc::default())).await
}

async fn character(reads: &Reads, member: &str, ign: &str) -> String {
    let reply = reads
        .call(
            "POST",
            &format!("/api/admin/members/{member}/characters"),
            json!({"ign": ign}),
            &[],
        )
        .await;
    assert_eq!(reply.status, 201, "{}", reply.text());
    reply.json()["id"].as_str().unwrap().to_owned()
}

/// `POST` → `202 {job}`, schema-checked.
async fn post(reads: &Reads, body: Value) -> Value {
    let reply = reads.call("POST", ESTIMATES, body, &[]).await;
    assert_eq!(reply.status, 202, "{}", reply.text());
    let started = reply.json();
    assert_valid(STARTED, ESTIMATES, &started);
    started["job"].clone()
}

async fn refused(reads: &Reads, body: Value) -> (u16, String) {
    let reply = reads.call("POST", ESTIMATES, body, &[]).await;
    assert_valid(ERROR, ESTIMATES, &reply.json());
    (reply.status, reply.api_error())
}

/// The job once it stops running.
async fn settled(reads: &Reads, job: &Value) -> Value {
    let path = format!("{ESTIMATES}/{}", job["id"].as_str().unwrap());
    for _ in 0..400 {
        let read = reads.read(&path, JOB).await;
        if read["state"] != "running" {
            return read;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{path} still running");
}

async fn look_up(reads: &Reads, body: Value) -> Value {
    let job = post(reads, body).await;
    settled(reads, &job).await
}

fn tokens(member: &Value) -> Vec<&str> {
    member["bosses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|boss| boss["boss"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn routes_are_the_generic_404_while_the_module_or_the_url_is_off() {
    for reads in [Reads::with_characters().await, on(None).await] {
        for (method, path) in [
            ("GET", format!("{ESTIMATES}?run_id=r-kalos")),
            ("POST", ESTIMATES.to_owned()),
            ("GET", format!("{ESTIMATES}/any")),
        ] {
            let signed_in = reads
                .call(method, &path, json!({"run_id": "r-kalos"}), &[])
                .await;
            assert_eq!(
                (signed_in.status, signed_in.api_error()),
                (404, "not_found".into()),
                "{method} {path}"
            );
            let anonymous = send(
                reads.admin,
                method,
                ADMIN_HOST,
                &path,
                &[("Origin", "https://kanade.test")],
                Some("{}"),
            )
            .await;
            assert_eq!(anonymous.status, 404, "{method} {path} signed out");
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
    assert_eq!(TIMEOUT, Duration::from_secs(130), "the production timeout");
    let (address, fake) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let id = character(&reads, "1001", "MikaBish").await;

    let running = post(&reads, json!({"character_ids": [id]})).await;
    assert_eq!(running["state"], "running");
    assert_eq!(running["started_by"]["kind"], "admin");
    assert_eq!(
        running["progress"],
        json!({"done": 0, "total": 1, "current": null})
    );
    let done = settled(&reads, &running).await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["label"], "Estimate from MapleScouter");
    assert_eq!(done["note"], "Estimate for planning only, not eligibility");
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
        (&mika["character_id"], &mika["ign"], &mika["preset"]),
        (&json!(id), &json!("MikaBish"), &json!("00010"))
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
    let status = reads
        .read(&format!("{ESTIMATES}?character_ids={id}"), STATUS)
        .await;
    assert_eq!(status["job"]["id"], done["id"]);
    assert_eq!(status["running"], Value::Null);
    let none = reads
        .read(&format!("{ESTIMATES}?character_ids=nobody"), STATUS)
        .await;
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

    let gone = reads
        .call("GET", &format!("{ESTIMATES}/nope"), json!(null), &[])
        .await;
    assert_eq!((gone.status, gone.api_error()), (404, "not_found".into()));

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
    let id = character(&reads, "1001", "MikaBish").await;
    let body = json!({"character_ids": [id]}).to_string();
    let forged = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        ESTIMATES,
        &[
            ("Cookie", reads.cookie.as_str()),
            ("Origin", "https://kanade.test"),
        ],
        Some(&body),
    )
    .await;
    assert_eq!((forged.status, forged.api_error()), (403, "csrf".into()));

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
    let gms = reads
        .call(
            "POST",
            "/api/admin/members/1001/characters",
            json!({"ign": "MikaGms", "world": "gms"}),
            &[],
        )
        .await;
    let gms = gms.json()["id"].as_str().unwrap().to_owned();
    assert_eq!(
        refused(&reads, json!({"character_ids": [gms]})).await,
        (422, "region".into())
    );
    // Nobody in r-kalos has a character yet.
    assert_eq!(
        refused(&reads, json!({"run_id": "r-kalos"})).await,
        (422, "nothing_to_look_up".into())
    );
    let bad = reads
        .call(
            "GET",
            &format!("{ESTIMATES}?run_id=a&character_ids=b"),
            json!(null),
            &[],
        )
        .await;
    assert_eq!((bad.status, bad.api_error()), (422, "invalid".into()));
    assert!(fake.lookups().is_empty(), "nothing was looked up");
}

#[tokio::test]
async fn scouter_errors_map_to_the_drawn_states_and_are_never_cached() {
    let (address, fake) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let cases = [
        (
            "BusyAa",
            "busy",
            Some(429),
            "busy",
            Some(5),
            "MapleScouter is busy.",
        ),
        (
            "LimitedAa",
            "busy",
            Some(429),
            "upstream_rate_limited",
            Some(40),
            "MapleScouter is busy.",
        ),
        (
            "CritAa",
            "critical_rate",
            Some(422),
            "critical_rate_setup_required",
            None,
            "Set up critical rate on MapleScouter first.",
        ),
        (
            "VerifyAa",
            "manual_check",
            Some(503),
            "verification_required",
            None,
            "Scouter needs a manual check.",
        ),
        (
            "BlockedAa",
            "failed",
            Some(502),
            "upstream_blocked",
            None,
            "Lookup failed.",
        ),
        (
            "MismatchAa",
            "failed",
            Some(502),
            "result_mismatch",
            None,
            "Lookup failed.",
        ),
        (
            "ExtractAa",
            "failed",
            Some(502),
            "extraction_failed",
            None,
            "Lookup failed.",
        ),
        (
            "FailedAa",
            "failed",
            Some(502),
            "lookup_failed",
            None,
            "Lookup failed.",
        ),
        (
            "SlowAa",
            "failed",
            Some(504),
            "lookup_timeout",
            None,
            "Lookup failed.",
        ),
        ("HangAa", "failed", None, "timeout", None, "Lookup failed."),
    ];
    for (ign, code, status, detail, retry, message) in cases {
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
    // Alice picks MikaBish of two, Bob's only character is inferred, Dan
    // has none.
    let mika = character(&reads, "1001", "MikaBish").await;
    character(&reads, "1001", "MikaHero").await;
    let ren = character(&reads, "1002", "RenShade").await;
    let picked = reads
        .call(
            "PUT",
            "/api/admin/runs/r-kalos/characters/1001",
            json!({"character_id": mika, "version": 0}),
            &[],
        )
        .await;
    assert_eq!(picked.status, 200, "{}", picked.text());

    let done = look_up(&reads, json!({"run_id": "r-kalos"})).await;
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["run_id"], "r-kalos");
    assert_eq!(done["character_ids"], json!([]));
    assert_eq!(
        done["characters"],
        json!([
            {"character_id": mika, "member_id": "1001", "ign": "MikaBish"},
            {"character_id": ren, "member_id": "1002", "ign": "RenShade"},
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
    assert_eq!(
        result["combined"],
        json!([{
            "boss": "XKalos",
            "combined_percentage": 18.0,
            "estimated_minutes": 3000.0 / 18.0,
            "technically_possible": false,
            "complete": false,
            "issues": [],
        }])
    );
    assert_eq!(result["members"].as_array().unwrap().len(), 2);

    // Cached by the party's IGN set, and each character by its own.
    let status = reads
        .read(&format!("{ESTIMATES}?run_id=r-kalos"), STATUS)
        .await;
    assert_eq!(status["job"]["id"], done["id"]);
    let again = post(&reads, json!({"run_id": "r-kalos"})).await;
    assert_eq!(
        (&again["state"], &again["result"]["cache_hit"]),
        (&json!("done"), &json!(true))
    );
    let strip = post(&reads, json!({"character_ids": [ren]})).await;
    assert_eq!(strip["result"]["cache_hit"], true);
    assert_eq!(fake.lookups().len(), 3, "no new lookups");

    // Bob gains a second character without a pick: missing, not guessed;
    // MikaBish alone comes from memory, then the party total.
    character(&reads, "1002", "RenAlt").await;
    let alone = look_up(&reads, json!({"run_id": "r-kalos"})).await;
    assert_eq!(
        alone["missing"],
        json!([
            {"member_id": "1002", "character_id": null, "reason": "no_character_picked"},
            {"member_id": "1004", "character_id": null, "reason": "no_characters"},
        ])
    );
    assert_eq!(
        fake.targets().last().map(String::as_str),
        Some("/v1/bosses?igns=MikaBish")
    );
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
async fn a_party_failure_keeps_finished_characters_and_makes_no_total() {
    let (address, fake) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let mika = character(&reads, "1001", "MikaBish").await;
    let crit = character(&reads, "1002", "CritAa").await;
    let failed = look_up(&reads, json!({"run_id": "r-kalos"})).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["result"], Value::Null, "no partial total");
    assert_eq!(failed["error"]["code"], "critical_rate");
    assert_eq!(
        failed["error"]["character_id"], crit,
        "names who stopped it"
    );
    assert_eq!(failed["progress"]["done"], 1);
    // MikaBish was kept: the strip answers from memory.
    let strip = post(&reads, json!({"character_ids": [mika]})).await;
    assert_eq!(strip["result"]["cache_hit"], true);
    assert_eq!(
        fake.targets(),
        ["/v1/bosses?ign=MikaBish", "/v1/bosses?ign=CritAa"]
    );
}

#[tokio::test]
async fn a_second_lookup_while_one_runs_is_409() {
    let (address, fake) = fake_scouter().await;
    let reads = on(Some(address)).await;
    let gate = character(&reads, "1001", "GateAa").await;
    let other = character(&reads, "1002", "MikaBish").await;
    let running = post(&reads, json!({"character_ids": [gate]})).await;
    assert_eq!(running["state"], "running");
    assert_eq!(
        refused(&reads, json!({"character_ids": [other]})).await,
        (409, "estimate_running".into())
    );
    let refusal = reads
        .call("POST", ESTIMATES, json!({"character_ids": [other]}), &[])
        .await;
    assert_eq!(refusal.json()["message"], "Another lookup is running.");
    // Another target's status names the running job (who and what).
    let status = reads
        .read(&format!("{ESTIMATES}?character_ids={other}"), STATUS)
        .await;
    assert_eq!(status["job"], Value::Null);
    assert_eq!(status["running"]["id"], running["id"]);
    assert_eq!(status["running"]["characters"][0]["ign"], "GateAa");
    fake.gate.notify_one();
    let done = settled(&reads, &running).await;
    assert_eq!(done["state"], "done");
    // Free again.
    look_up(&reads, json!({"character_ids": [other]})).await;
}

#[tokio::test]
async fn scouter_requests_never_carry_nexon_fields() {
    let (address, fake) = fake_scouter().await;
    let nexon = fake_nexon().await;
    let slot = Arc::new(std::sync::OnceLock::new());
    let reads = Reads::with_character_data(
        Some(config()),
        module(Some(address), Some(nexon), Arc::clone(&slot)),
    )
    .await;
    let service = slot.get().unwrap().clone();
    // The create already asked the member to link it (pending).
    let id = character(&reads, "1001", "MikaBish").await;
    let ConfirmOutcome::Linked(..) = service
        .confirm_link("1001", &id, NOTICE_VERSION)
        .await
        .unwrap()
    else {
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
    assert!(sent.contains("ign=MikaBish"), "the roster IGN: {sent}");
    for nexon_field in [
        "MIKABISH",
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
        (
            &failed["error"]["code"],
            &failed["error"]["detail"],
            &failed["error"]["status"]
        ),
        (&json!("failed"), &json!("unreachable"), &Value::Null)
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
    let kept = reads
        .read(
            &format!("{ESTIMATES}/{}", first["id"].as_str().unwrap()),
            JOB,
        )
        .await;
    assert_eq!(kept["state"], "done", "the first job is still there");
    // A repeat lookup of a target replaces its job.
    let again = post(&reads, json!({"character_ids": [ids[0]]})).await;
    assert_ne!(again["id"], first["id"]);
    let replaced = reads
        .call(
            "GET",
            &format!("{ESTIMATES}/{}", first["id"].as_str().unwrap()),
            json!(null),
            &[],
        )
        .await;
    assert_eq!(
        (replaced.status, replaced.api_error()),
        (404, "not_found".into())
    );
    let status = reads
        .read(&format!("{ESTIMATES}?character_ids={}", ids[0]), STATUS)
        .await;
    assert_eq!(status["job"]["id"], again["id"]);
}

const READY: &str = "character_data.json#/$defs/RunReadiness";

fn ready_slot<'a>(answer: &'a Value, member: &str) -> &'a Value {
    answer["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["member_id"] == member)
        .unwrap_or_else(|| panic!("no slot for {member}: {answer}"))
}

/// Readiness (`r-kalos` XKalos; `r-star` HMaleficStar + HLucid with Alice)
/// shows each character's cached MapleScouter reading per boss, from a
/// lookup of them alone or of a run that included them, and never looks
/// anything up itself. NEXON combat power stays as it was.
#[tokio::test]
async fn readiness_shows_cached_scouter_readings_per_boss_and_never_looks_up() {
    const KALOS: &str = "/api/admin/runs/r-kalos/readiness";
    const STAR: &str = "/api/admin/runs/r-star/readiness";
    let (address, fake) = fake_scouter().await;
    let nexon = fake_nexon().await;
    let slot = Arc::new(std::sync::OnceLock::new());
    let reads = Reads::with_character_data(
        Some(config()),
        module(Some(address), Some(nexon), Arc::clone(&slot)),
    )
    .await;
    let service = slot.get().unwrap().clone();
    // Alice brings RenShade (pending, no data), Bob MikaBish (linked), Dan
    // nobody. A MapleSEA create already asks for the link.
    let ren = character(&reads, "1001", "RenShade").await;
    let mika = character(&reads, "1002", "MikaBish").await;
    let ConfirmOutcome::Linked(..) = service
        .confirm_link("1002", &mika, NOTICE_VERSION)
        .await
        .unwrap()
    else {
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
        reads
            .read(&format!("{ESTIMATES}?run_id=r-kalos"), STATUS)
            .await,
        json!({"job": null, "running": null})
    );

    // RenShade alone: her reading per boss; MikaBish keeps NEXON CP only.
    look_up(&reads, json!({"character_ids": [ren]})).await;
    let mixed = reads.read(KALOS, READY).await;
    let alice = ready_slot(&mixed, "1001");
    assert_eq!(
        alice["bosses"][0]["scouter"],
        json!({"percentage": 9.0, "rating": "6p Min Cut", "estimated_minutes": 3000.0 / 9.0, "fetched_at": "2026-09-29T04:00:00Z"})
    );
    assert_eq!(alice["power"], Value::Null);
    let bob = ready_slot(&mixed, "1002");
    assert_eq!(bob["power"], nexon_cp);
    assert_eq!(bob["bosses"][0]["scouter"], Value::Null);
    // Mapped bosses only: Star has a card; Lucid has none at MapleScouter.
    let star = reads.read(STAR, READY).await;
    let shares: Vec<&Value> = ready_slot(&star, "1001")["bosses"]
        .as_array()
        .unwrap()
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
        reads
            .read(&format!("{ESTIMATES}?run_id=r-kalos"), STATUS)
            .await["running"],
        Value::Null
    );
}
