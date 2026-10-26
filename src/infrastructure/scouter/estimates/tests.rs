use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::TimeZone;

use super::*;
use crate::infrastructure::{
    scouter::{Card, UpcomingRun},
    store::SqliteCharData,
};
use kanade_domain::characters::{Character, CharacterRole};

type Now = Arc<Mutex<DateTime<Utc>>>;

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 29, hour, 0, 0).unwrap()
}

fn set(now: &Now, to: DateTime<Utc>) {
    *now.lock().unwrap_or_else(PoisonError::into_inner) = to;
}

/// Invented roster IGN.
fn ign(name: &str) -> RosterIgn {
    RosterIgn::of(&Character {
        id: "ch-1".into(),
        member_id: "1001".into(),
        ign: name.into(),
        world: "msea".into(),
        class: None,
        level: None,
        role: CharacterRole::Main,
        retired_at: None,
        created_at: at(0),
        updated_at: at(0),
        updated_by: Actor::system("test"),
        version: 1,
    })
    .unwrap()
}

fn subject(id: &str, name: &str) -> Subject {
    Subject {
        character_id: id.into(),
        member_id: "1001".into(),
        ign: ign(name),
    }
}

/// Invented answer: one Limbo card worth `percentage`.
fn member(ign: &str, percentage: f64) -> Member {
    Member {
        ign: ign.into(),
        preset: "00010".into(),
        critical_rate_assumed: false,
        critical_rate_actual: None,
        source: None,
        bosses: BTreeMap::from([(
            "hard_limbo".to_owned(),
            Card {
                percentage,
                rating: "Party-able".into(),
                party: false,
                effective_percentage: Some(percentage),
                estimated_minutes: Some(3_000.0 / percentage),
            },
        )]),
    }
}

/// Invented party total.
fn total() -> BTreeMap<String, Combined> {
    BTreeMap::from([(
        "hard_limbo".into(),
        Combined {
            combined_percentage: Some(112.4),
            estimated_minutes: Some(26.7),
            technically_possible: Some(true),
            complete: true,
            issues: BTreeMap::new(),
        },
    )])
}

/// Nothing listens at the sidecar's address: a lookup that reaches it
/// fails.
fn estimates(now: Now) -> Estimates {
    let url = "http://127.0.0.1:9".to_owned();
    Estimates::new(
        ScouterClient::new(&url).unwrap(),
        url,
        Arc::new(move || *now.lock().unwrap_or_else(PoisonError::into_inner)),
    )
}

fn lookup(target: Target, subjects: Vec<Subject>) -> JobRequest {
    JobRequest {
        target,
        subjects,
        missing: Vec::new(),
        parties: Vec::new(),
        started_by: Actor::system("test"),
        refresh: false,
    }
}

/// Wait until job `id` has finished.
async fn finished(estimates: &Estimates, id: &str) -> Job {
    for _ in 0..1_000 {
        if let Some(job) = estimates.job(id).filter(|job| job.outcome.is_some()) {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the job did not finish");
}

fn hit(start: Start) -> Found {
    match start {
        Start::Started(Job {
            outcome: Some(Ok(found)),
            ..
        }) if found.cache_hit => found,
        _ => panic!("answered from memory"),
    }
}

/// `event`'s captured lines.
fn lines(event: &str) -> Vec<serde_json::Value> {
    kanade_log::captured()
        .into_iter()
        .filter(|line| line["event"] == event)
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_reading_of_any_age_is_the_newest_and_gone_after_30_days() {
    let now = Arc::new(Mutex::new(at(4)));
    let estimates = estimates(Arc::clone(&now));
    let ren = subject("ch-1", "RenShade");
    assert_eq!(estimates.cached(&ren.ign), None, "nothing looked up yet");

    estimates
        .shared
        .keep_member(&ren, &member("renshade", 40.0))
        .await;
    set(&now, at(6));
    tokio::time::advance(Duration::from_secs(2 * 3_600)).await;
    estimates
        .shared
        .keep_member(&subject("ch-1", "RENSHADE"), &member("RENSHADE", 72.4))
        .await;
    let (newest, fetched_at) = estimates.cached(&ren.ign).unwrap();
    assert_eq!(
        (newest.bosses["hard_limbo"].percentage, fetched_at),
        (72.4, at(6))
    );
    assert_eq!(estimates.cached(&ign("MikaBish")), None);

    // Long stale for a lookup, still read with its fetch time.
    set(&now, at(6) + TimeDelta::days(20));
    tokio::time::advance(CACHE_TTL - Duration::from_secs(1)).await;
    assert_eq!(estimates.cached(&ren.ign).map(|(_, at)| at), Some(at(6)));
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(estimates.cached(&ren.ign), None, "30 days in memory");
    assert!(estimates.running().is_none(), "reading starts nothing");
}

#[tokio::test(start_paused = true)]
async fn a_party_total_is_read_by_its_exact_ign_set_and_shares_use_effective_only() {
    let estimates = estimates(Arc::new(Mutex::new(at(4))));
    let (ren, mika) = (subject("ch-1", "RenShade"), subject("ch-2", "MikaBish"));
    let mut party = member("MikaBish", 52.0);
    party
        .bosses
        .get_mut("hard_limbo")
        .unwrap()
        .effective_percentage = None;
    estimates
        .shared
        .keep_party(&[mika.clone(), ren.clone()], &total(), at(4))
        .await;
    assert_eq!(
        estimates.cached_party(&[ign("renshade"), mika.ign.clone()]),
        Some((total(), at(4))),
        "any order and case, with its fetch time"
    );
    assert_eq!(estimates.cached_party(&[ren.ign]), None, "a different set");
    assert_eq!(member("RenShade", 60.4).effective("HLimbo"), Some(60.4));
    assert_eq!(
        party.effective("HLimbo"),
        None,
        "never the card's whole-party percentage"
    );
    assert_eq!(member("RenShade", 60.4).effective("NLimbo"), None);
    assert!(estimates.running().is_none(), "reading starts nothing");
}

/// Only a result younger than the saved `estimate_days` answers a lookup
/// at once; an older one is looked up again (the sidecar is asked, even by
/// the worker, which never falls back on it), while readers keep it. A
/// Config save applies to the next lookup.
#[tokio::test(start_paused = true)]
async fn a_stale_result_is_looked_up_again() {
    kanade_log::capture();
    let now = Arc::new(Mutex::new(at(4)));
    let (days, saved) = tokio::sync::watch::channel(7_u32);
    let estimates = estimates(Arc::clone(&now)).with_fresh_days(Arc::new(move || *saved.borrow()));
    let ren = subject("ch-1", "RenShade");
    estimates
        .shared
        .keep_member(&ren, &member("RenShade", 60.4))
        .await;
    let request = || lookup(Target::Characters(vec!["ch-1".into()]), vec![ren.clone()]);

    set(&now, at(4) + TimeDelta::days(7) - TimeDelta::seconds(1));
    assert_eq!(hit(estimates.start(request())).fetched_at, at(4), "fresh");

    set(&now, at(4) + TimeDelta::days(7));
    let Start::Started(job) = estimates.start(request()) else {
        panic!("a new job");
    };
    assert_eq!(job.outcome, None, "queued for a real lookup");
    let done = finished(&estimates, &job.id).await;
    assert!(
        matches!(done.results.as_slice(), [Some(Err(_))]),
        "the sidecar was asked: {:?}",
        done.results
    );
    assert_eq!(
        estimates.cached(&ren.ign).map(|(_, at)| at),
        Some(at(4)),
        "readers still get it"
    );

    days.send(30).unwrap();
    assert_eq!(hit(estimates.start(request())).fetched_at, at(4));
    // A result about another character under the same IGN never answers.
    let Start::Started(other) = estimates.start(lookup(
        Target::Characters(vec!["ch-9".into()]),
        vec![subject("ch-9", "RenShade")],
    )) else {
        panic!("a new job");
    };
    assert_eq!(other.outcome, None);
}

/// Each result and whole-party total is written to the file as it
/// arrives; a restart loads them, and they answer as before.
#[tokio::test(start_paused = true)]
async fn results_are_written_through_and_loaded_after_a_restart() {
    kanade_log::capture();
    let file = Arc::new(SqliteCharData::open_temp().await);
    let now = Arc::new(Mutex::new(at(4)));
    let estimates =
        estimates(Arc::clone(&now)).with_store(Arc::clone(&file) as Arc<dyn CharDataStore>);
    let (ren, mika) = (subject("ch-2", "RenShade"), subject("ch-1", "MikaBish"));
    estimates
        .shared
        .keep_member(&ren, &member("RenShade", 60.4))
        .await;
    estimates
        .shared
        .keep_member(&mika, &member("MikaBish", 52.0))
        .await;
    estimates
        .shared
        .keep_party(&[ren.clone(), mika.clone()], &total(), at(4))
        .await;
    let stored = file.estimates(at(4), 10).await.unwrap();
    let rows: Vec<(&str, &str)> = stored
        .members
        .iter()
        .map(|row| (row.character_id.as_str(), row.ign_key.as_str()))
        .collect();
    assert_eq!(rows, [("ch-1", "mikabish"), ("ch-2", "renshade")]);
    assert_eq!(stored.parties[0].set_key, "mikabish,renshade");
    assert_eq!(stored.parties[0].character_ids, ["ch-1", "ch-2"]);
    drop(estimates);

    // The "restart", a day later.
    set(&now, at(4) + TimeDelta::days(1));
    let restarted =
        self::estimates(Arc::clone(&now)).with_store(Arc::clone(&file) as Arc<dyn CharDataStore>);
    assert_eq!(restarted.cached(&ren.ign), None, "nothing before the load");
    restarted.load().await;
    assert_eq!(
        restarted.cached(&ren.ign),
        Some((member("RenShade", 60.4), at(4)))
    );
    assert_eq!(
        restarted.cached_party(&[mika.ign.clone(), ren.ign.clone()]),
        Some((total(), at(4)))
    );
    let found = hit(restarted.start(lookup(
        Target::Run {
            run_id: "r1".into(),
            bosses: vec!["HLimbo".into()],
        },
        vec![ren, mika],
    )));
    assert_eq!(found.combined, Some(total()));
    let loaded = lines("estimates_loaded");
    assert_eq!(
        (&loaded[0]["members"], &loaded[0]["parties"]),
        (&serde_json::json!(2), &serde_json::json!(1))
    );
}

/// A write the file refuses is logged and the result still answers from
/// memory.
#[tokio::test(start_paused = true)]
async fn a_failed_write_keeps_the_result_in_memory() {
    kanade_log::capture();
    let file = Arc::new(SqliteCharData::open_temp().await);
    file.temp_dir()
        .raw(
            "DROP TABLE estimate_party_members; DROP TABLE estimate_parties;
             DROP TABLE estimate_members;",
        )
        .await
        .expect("tables dropped");
    let estimates = estimates(Arc::new(Mutex::new(at(4))))
        .with_store(Arc::clone(&file) as Arc<dyn CharDataStore>);
    let (ren, mika) = (subject("ch-1", "RenShade"), subject("ch-2", "MikaBish"));
    estimates
        .shared
        .keep_member(&ren, &member("RenShade", 60.4))
        .await;
    estimates
        .shared
        .keep_party(&[ren.clone(), mika.clone()], &total(), at(4))
        .await;
    assert_eq!(
        estimates.cached(&ren.ign),
        Some((member("RenShade", 60.4), at(4)))
    );
    assert_eq!(
        estimates.cached_party(&[ren.ign, mika.ign]),
        Some((total(), at(4)))
    );
    let unsaved: Vec<(String, String)> = lines("estimate_unsaved")
        .iter()
        .map(|line| (line["level"].to_string(), line["kind"].to_string()))
        .collect();
    assert_eq!(
        unsaved,
        [
            ("\"WARN\"".into(), "\"member\"".into()),
            ("\"WARN\"".into(), "\"party\"".into())
        ]
    );
}

/// A fake sidecar: each `ign=` answers one Limbo card, `igns=` a total.
async fn sidecar() -> String {
    async fn answer(uri: axum::http::Uri) -> String {
        let query = uri.query().unwrap_or_default();
        match query.strip_prefix("ign=") {
            Some(name) => serde_json::json!({
                "ign": name,
                "preset": "00010",
                "bosses": {"hard_limbo": {
                    "percentage": 60.0, "rating": "Party-able", "party": false,
                    "effective_percentage": 60.0, "estimated_minutes": 50.0,
                }},
            })
            .to_string(),
            None => serde_json::json!({"combined": {"hard_limbo": {
                "combined_percentage": 112.4, "estimated_minutes": 26.7,
                "technically_possible": true, "complete": true,
            }}})
            .to_string(),
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new().route("/v1/bosses", axum::routing::get(answer));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{address}")
}

/// The worker writes each member and the whole party's total through.
#[tokio::test]
async fn a_party_lookup_writes_every_result_through() {
    let file = Arc::new(SqliteCharData::open_temp().await);
    let url = sidecar().await;
    let estimates = Estimates::new(ScouterClient::new(&url).unwrap(), url, Arc::new(|| at(4)))
        .with_store(Arc::clone(&file) as Arc<dyn CharDataStore>);
    let (ren, mika) = (subject("ch-1", "RenShade"), subject("ch-2", "MikaBish"));
    let Start::Started(job) = estimates.start(lookup(
        Target::Run {
            run_id: "r1".into(),
            bosses: vec!["HLimbo".into()],
        },
        vec![ren.clone(), mika.clone()],
    )) else {
        panic!("a new job");
    };
    let done = finished(&estimates, &job.id).await;
    assert!(matches!(&done.outcome, Some(Ok(found)) if !found.cache_hit));
    let stored = file.estimates(at(4), 10).await.unwrap();
    assert_eq!(stored.members.len(), 2);
    assert_eq!(stored.parties.len(), 1);
    assert_eq!(stored.parties[0].character_ids, ["ch-1", "ch-2"]);
    assert_eq!(
        estimates
            .cached_party(&[ren.ign, mika.ign])
            .map(|(total, _)| total["hard_limbo"].combined_percentage),
        Some(Some(112.4))
    );
}

/// A fake sidecar noting every query: each `ign=` answers HLimbo and
/// NLimbo cards, each `igns=` a total for both whose percentage says
/// whether the set held MikaNL (200) or not (100).
async fn noting_sidecar() -> (String, Arc<Mutex<Vec<String>>>) {
    let asked: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = Arc::clone(&asked);
    let answer = move |uri: axum::http::Uri| {
        let seen = Arc::clone(&seen);
        async move {
            let query = uri.query().unwrap_or_default().to_owned();
            seen.lock().unwrap().push(query.clone());
            let card = serde_json::json!({
                "percentage": 60.0, "rating": "Party-able", "party": false,
                "effective_percentage": 60.0, "estimated_minutes": 50.0,
            });
            match query.strip_prefix("ign=") {
                Some(name) => serde_json::json!({
                    "ign": name, "preset": "00010",
                    "bosses": {"hard_limbo": card, "normal_limbo": card},
                })
                .to_string(),
                None => {
                    let pct = if query.to_ascii_lowercase().contains("mikanl") {
                        200.0
                    } else {
                        100.0
                    };
                    let total = serde_json::json!({
                        "combined_percentage": pct, "estimated_minutes": 20.0,
                        "technically_possible": true, "complete": true,
                    });
                    serde_json::json!({"combined": {"hard_limbo": total, "normal_limbo": total}})
                        .to_string()
                }
            }
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = axum::Router::new().route("/v1/bosses", axum::routing::get(answer));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{address}"), asked)
}

/// Per-boss picks: each distinct character is looked up once, then one
/// total per distinct boss party, each counted only for its bosses; a
/// party another party already asked is not asked again; each whole party
/// is kept under its own IGN set.
#[tokio::test]
async fn each_distinct_character_once_and_one_total_per_distinct_boss_party() {
    let (url, asked) = noting_sidecar().await;
    let estimates = Estimates::new(ScouterClient::new(&url).unwrap(), url, Arc::new(|| at(4)));
    let (ren, bish, nl) = (
        subject("ch-1", "RenShade"),
        subject("ch-2", "MikaBish"),
        subject("ch-3", "MikaNL"),
    );
    let party = |bosses: &[&str], subjects: &[usize]| BossParty {
        bosses: bosses.iter().map(|boss| (*boss).to_owned()).collect(),
        subjects: subjects.to_vec(),
        missing: 0,
    };
    let request = JobRequest {
        parties: vec![
            party(&["HLimbo", "HLimbo2"], &[0, 1]),
            party(&["NLimbo"], &[0, 2]),
            // The same characters as the first party: its answer is reused.
            party(&["XLimbo"], &[0, 1]),
        ],
        ..lookup(
            Target::Run {
                run_id: "r1".into(),
                bosses: vec!["HLimbo".into(), "NLimbo".into(), "XLimbo".into()],
            },
            vec![ren.clone(), bish.clone(), nl.clone()],
        )
    };
    let Start::Started(job) = estimates.start(request) else {
        panic!("a new job");
    };
    let done = finished(&estimates, &job.id).await;
    let Some(Ok(found)) = &done.outcome else {
        panic!("found: {:?}", done.outcome);
    };
    let asked = asked.lock().unwrap().clone();
    let singles = asked
        .iter()
        .filter(|query| query.starts_with("ign="))
        .count();
    let totals = asked
        .iter()
        .filter(|query| query.starts_with("igns="))
        .count();
    assert_eq!((singles, totals), (3, 2), "{asked:?}");
    let combined = found.combined.as_ref().expect("totals");
    assert_eq!(combined["hard_limbo"].combined_percentage, Some(100.0));
    assert_eq!(combined["normal_limbo"].combined_percentage, Some(200.0));
    assert!(
        estimates
            .cached_party(&[ren.ign.clone(), bish.ign.clone()])
            .is_some()
    );
    assert!(
        estimates
            .cached_party(&[ren.ign.clone(), nl.ign.clone()])
            .is_some()
    );
    assert_eq!(done.parties.len(), 3);

    // The same run again: every result and both totals are fresh, so it is
    // answered from memory without asking the sidecar.
    let again = JobRequest {
        parties: vec![party(&["HLimbo"], &[0, 1]), party(&["NLimbo"], &[0, 2])],
        ..lookup(
            Target::Run {
                run_id: "r2".into(),
                bosses: vec!["HLimbo".into(), "NLimbo".into()],
            },
            vec![ren, bish, nl],
        )
    };
    let found = hit(estimates.start(again));
    assert_eq!(
        found.combined.unwrap()["normal_limbo"].combined_percentage,
        Some(200.0)
    );
}

/// A 422 is the crit-setup state unless the scouter says it tried assuming
/// 100% critical rate and couldn't finish.
#[test]
fn a_failed_crit_assumption_is_its_own_state() {
    let refused = |code: &str| LookupError::Refused {
        status: 422,
        code: code.into(),
        ign: Some("CritAa".into()),
        retry_after: None,
    };
    let igns = [ign("CritAa")];
    for (code, kind) in [
        ("critical_rate_setup_required", FailureKind::CriticalRate),
        (
            "critical_rate_assumption_failed",
            FailureKind::AssumptionFailed,
        ),
        ("unknown", FailureKind::CriticalRate),
    ] {
        let failure = Failure::of(&refused(code), &igns, None);
        assert_eq!(failure.kind, kind, "{code}");
        assert_eq!((failure.status, failure.detail.as_str()), (Some(422), code));
        assert_eq!(failure.subject, Some(0));
    }
}

/// `429 busy` is asked again after its Retry-After while the wait ends
/// before the deadline; past it, the busy answer stands.
#[tokio::test(start_paused = true)]
async fn busy_is_retried_after_retry_after_within_the_deadline() {
    fn busy(seconds: u32) -> LookupError {
        LookupError::Refused {
            status: 429,
            code: "busy".into(),
            ign: None,
            retry_after: Some(seconds),
        }
    }
    let estimates = estimates(Arc::new(Mutex::new(at(4))));
    let shared = &estimates.shared;
    let calls = AtomicUsize::new(0);
    let begun = Instant::now();
    let deadline = begun + Duration::from_secs(30);
    let answer = shared
        .call(deadline, |within| {
            let call = calls.fetch_add(1, Ordering::SeqCst);
            let left = deadline.saturating_duration_since(Instant::now());
            async move {
                assert_eq!(within, left, "each try gets the time left");
                if call < 2 { Err(busy(8)) } else { Ok(call) }
            }
        })
        .await;
    assert_eq!(answer, Ok(2), "the third try answers");
    assert_eq!(
        begun.elapsed(),
        Duration::from_secs(16),
        "8 s after each busy"
    );

    // A wait that would end past the deadline is not taken.
    calls.store(0, Ordering::SeqCst);
    let begun = Instant::now();
    let answer = shared
        .call(begun + Duration::from_secs(10), |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err::<usize, _>(busy(8)) }
        })
        .await;
    assert_eq!(answer, Err(busy(8)));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "one retry fits, the next doesn't"
    );
    assert_eq!(begun.elapsed(), Duration::from_secs(8));

    // Other refusals, a rate limit included, are never retried.
    calls.store(0, Ordering::SeqCst);
    let limited = LookupError::Refused {
        status: 429,
        code: "upstream_rate_limited".into(),
        ign: None,
        retry_after: Some(1),
    };
    let answer = shared
        .call(Instant::now() + Duration::from_secs(60), |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            let limited = limited.clone();
            async move { Err::<usize, _>(limited) }
        })
        .await;
    assert_eq!(answer, Err(limited));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        shared
            .inner()
            .seen
            .last_error
            .as_ref()
            .map(|(code, _)| code.as_str()),
        Some("upstream_rate_limited")
    );
}

#[test]
fn memory_keeps_results_as_long_as_the_file() {
    assert_eq!(TimeDelta::from_std(CACHE_TTL).unwrap(), ESTIMATE_TTL);
}

/// A fake sidecar for the background queue: it notes each `ign=` asked
/// (the IGN and the paused clock's instant), answers it once `gate` gives a
/// permit, and refuses it (`502`) while `fail` is set; `igns=` answers a
/// total at once and `/healthz` is up.
#[derive(Clone)]
struct Fake {
    asked: Arc<Mutex<Vec<(String, Instant)>>>,
    gate: Arc<tokio::sync::Semaphore>,
    fail: Arc<std::sync::atomic::AtomicBool>,
}

/// Permits enough for any test.
const OPEN: usize = 1 << 20;

impl Fake {
    /// The fake and its URL; `open`: answers at once, else on [`Self::open`].
    async fn start(open: bool) -> (Self, String) {
        use axum::{
            extract::State,
            http::{StatusCode, Uri},
            routing::get,
        };
        async fn bosses(State(fake): State<Fake>, uri: Uri) -> (StatusCode, String) {
            let query = uri.query().unwrap_or_default();
            let Some(name) = query.strip_prefix("ign=") else {
                let total = serde_json::json!({"combined": {"hard_limbo": {
                    "combined_percentage": 112.4, "estimated_minutes": 26.7,
                    "technically_possible": true, "complete": true,
                }}});
                return (StatusCode::OK, total.to_string());
            };
            fake.asked
                .lock()
                .unwrap()
                .push((name.to_owned(), Instant::now()));
            fake.gate.acquire().await.expect("never closed").forget();
            if fake.fail.load(Ordering::SeqCst) {
                let body = serde_json::json!({"error": {"code": "upstream_failed"}});
                return (StatusCode::BAD_GATEWAY, body.to_string());
            }
            let found = serde_json::json!({
                "ign": name,
                "preset": "00010",
                "bosses": {"hard_limbo": {
                    "percentage": 60.0, "rating": "Party-able", "party": false,
                    "effective_percentage": 60.0, "estimated_minutes": 50.0,
                }},
            });
            (StatusCode::OK, found.to_string())
        }
        let fake = Self {
            asked: Arc::default(),
            gate: Arc::new(tokio::sync::Semaphore::new(if open { OPEN } else { 0 })),
            fail: Arc::default(),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new()
            .route("/v1/bosses", get(bosses))
            .route("/healthz", get(|| async { StatusCode::OK }))
            .with_state(fake.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (fake, url)
    }

    fn open(&self) {
        self.gate.add_permits(OPEN);
    }

    fn names(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// When `name` was first asked for.
    fn at(&self, name: &str) -> Instant {
        self.asked
            .lock()
            .unwrap()
            .iter()
            .find(|(asked, _)| asked == name)
            .map(|(_, at)| *at)
            .expect("asked")
    }
}

fn at_url(url: &str) -> Estimates {
    Estimates::new(
        ScouterClient::new(url).unwrap(),
        url.to_owned(),
        Arc::new(|| at(4)),
    )
}

/// The sidecar was just probed and is up, so a pass does not probe it.
fn healthy(estimates: &Estimates) {
    estimates.shared.inner().seen.probed = Some(Instant::now());
}

/// The schedule a pass reads: these runs.
struct Parties(Vec<UpcomingRun>);

impl UpcomingParties for Parties {
    fn upcoming(&self, _: DateTime<Utc>) -> refresh::UpcomingFuture<'_> {
        let runs = self.0.clone();
        Box::pin(async move {
            Some(refresh::Upcoming {
                current_week: at(0),
                runs,
            })
        })
    }
}

/// Run `id`, `hours` after 04:00, with `subjects`.
fn upcoming(id: &str, hours: i64, subjects: Vec<Subject>) -> UpcomingRun {
    UpcomingRun {
        run_id: id.into(),
        bosses: vec!["HLimbo".into()],
        status: kanade_domain::schedule::RunStatus::Planned,
        datetime: at(4) + TimeDelta::hours(hours),
        week_start: at(0),
        subjects,
        missing: Vec::new(),
        parties: Vec::new(),
    }
}

fn run_target(id: &str) -> Target {
    Target::Run {
        run_id: id.into(),
        bosses: vec!["HLimbo".into()],
    }
}

fn characters_target(subject: &Subject) -> JobRequest {
    lookup(
        Target::Characters(vec![subject.character_id.clone()]),
        vec![subject.clone()],
    )
}

/// Wait (on the paused clock, 10 ms at a time) until `done` holds.
async fn until(mut done: impl FnMut() -> bool) {
    for _ in 0..10_000 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("it never happened");
}

/// Run `id`'s job, once finished.
async fn settled(estimates: &Estimates, id: &str) -> Job {
    let target = run_target(id);
    until(|| {
        estimates
            .latest(&target)
            .is_some_and(|job| job.outcome.is_some())
    })
    .await;
    estimates.latest(&target).unwrap()
}

fn field(event: &str, key: &str) -> Vec<serde_json::Value> {
    lines(event).iter().map(|line| line[key].clone()).collect()
}

fn ren() -> Subject {
    subject("ch-1", "RenShade")
}

fn mika() -> Subject {
    subject("ch-2", "MikaBish")
}

fn aoi() -> Subject {
    subject("ch-3", "AoiLyn")
}

/// A manual job waits only for the running background job, never for the
/// background jobs still waiting.
#[tokio::test(start_paused = true)]
async fn manual_jobs_run_before_waiting_background_jobs() {
    kanade_log::capture();
    let (fake, url) = Fake::start(false).await;
    let estimates = at_url(&url);
    healthy(&estimates);
    let parties = Parties(vec![
        upcoming("r1", 2, vec![ren()]),
        upcoming("r2", 3, vec![mika()]),
    ]);
    estimates.refresh(&parties).await;
    until(|| fake.names().len() == 1).await;
    let Start::Started(manual) = estimates.start(characters_target(&aoi())) else {
        panic!("a new job");
    };
    assert_eq!(manual.state(), JobState::Queued);
    assert_eq!(manual.position, Some(1), "behind the running job only");
    fake.open();
    settled(&estimates, "r2").await;
    assert_eq!(fake.names(), ["RenShade", "AoiLyn", "MikaBish"]);
    assert_eq!(
        field("estimate_finished", "source"),
        ["background", "manual", "background"]
    );
    let pass = &lines("estimate_refresh_pass")[0];
    assert_eq!(
        [
            &pass["runs"],
            &pass["due"],
            &pass["queued"],
            &pass["cap_left"]
        ],
        [2, 2, 2, 60]
    );
    assert!(pass["skipped"].is_null());
}

/// A manual lookup of a run whose background job waits takes that job into
/// the manual queue: it runs next, with no gap.
#[tokio::test(start_paused = true)]
async fn a_manual_lookup_of_a_waiting_background_run_moves_it_up() {
    kanade_log::capture();
    let (fake, url) = Fake::start(false).await;
    let estimates = at_url(&url);
    healthy(&estimates);
    let parties = Parties(vec![
        upcoming("r1", 2, vec![ren()]),
        upcoming("r2", 3, vec![mika()]),
        upcoming("r3", 4, vec![aoi()]),
    ]);
    estimates.refresh(&parties).await;
    until(|| fake.names().len() == 1).await;
    let Start::Joined(moved) = estimates.start(lookup(run_target("r3"), vec![aoi()])) else {
        panic!("joined");
    };
    assert_eq!(moved.started_by, Actor::system(REFRESHER));
    assert_eq!(moved.position, Some(1));
    assert_eq!(
        estimates.shared.inner().background.len(),
        1,
        "r2 still waits"
    );
    fake.open();
    settled(&estimates, "r2").await;
    assert_eq!(fake.names(), ["RenShade", "AoiLyn", "MikaBish"]);
    assert!(
        fake.at("AoiLyn") < fake.at("RenShade") + BACKGROUND_GAP,
        "no gap before it"
    );
    assert_eq!(
        field("estimate_finished", "source"),
        ["background", "manual", "background"]
    );
}

/// Background jobs never fill the manual queue, and the running one is
/// joined as before.
#[tokio::test(start_paused = true)]
async fn background_jobs_do_not_count_toward_max_queued() {
    let (fake, url) = Fake::start(false).await;
    let estimates = at_url(&url);
    healthy(&estimates);
    let parties = Parties(vec![
        upcoming("r1", 2, vec![ren()]),
        upcoming("r2", 3, vec![mika()]),
        upcoming("r3", 4, vec![aoi()]),
    ]);
    estimates.refresh(&parties).await;
    until(|| fake.names().len() == 1).await;
    for n in 0..MAX_QUEUED {
        let member = subject(&format!("ch-1{n:02}"), &format!("Member{n}"));
        assert!(matches!(
            estimates.start(characters_target(&member)),
            Start::Started(_)
        ));
    }
    let extra = subject("ch-999", "Extra");
    assert!(matches!(
        estimates.start(characters_target(&extra)),
        Start::Full
    ));
    assert_eq!(estimates.shared.inner().background.len(), 2);
    assert!(matches!(
        estimates.start(lookup(run_target("r1"), vec![ren()])),
        Start::Joined(job) if job.state() == JobState::Running
    ));
    estimates.stop().await;
    assert!(
        estimates.shared.inner().background.is_empty(),
        "stop drops them"
    );
}

/// The next background job waits until 30 s after the last one ended; a
/// manual job in that gap runs at once.
#[tokio::test(start_paused = true)]
async fn background_jobs_keep_a_gap_that_manual_jobs_skip() {
    let (fake, url) = Fake::start(true).await;
    let estimates = at_url(&url);
    healthy(&estimates);
    let parties = Parties(vec![
        upcoming("r1", 2, vec![ren()]),
        upcoming("r2", 3, vec![mika()]),
    ]);
    estimates.refresh(&parties).await;
    settled(&estimates, "r1").await;
    let ended = estimates.shared.inner().limits.ended.expect("r1 ended");
    tokio::time::sleep_until(ended + BACKGROUND_GAP - Duration::from_secs(1)).await;
    assert_eq!(fake.names(), ["RenShade"], "r2 waits out the gap");
    assert!(estimates.latest(&run_target("r2")).is_none());
    let Start::Started(manual) = estimates.start(characters_target(&aoi())) else {
        panic!("a new job");
    };
    assert_eq!(manual.state(), JobState::Running, "never waits on the gap");
    settled(&estimates, "r2").await;
    assert!(fake.at("AoiLyn") < ended + BACKGROUND_GAP);
    assert!(fake.at("MikaBish") >= ended + BACKGROUND_GAP);
}

/// Background `ign=` lookups stop at the cap: a job the cap no longer
/// covers stops (keeping what it found) and the rest are dropped; passes
/// queue nothing more until lookups leave the 24-hour window.
#[tokio::test(start_paused = true)]
async fn the_cap_stops_further_background_lookups() {
    kanade_log::capture();
    let (fake, url) = Fake::start(false).await;
    let estimates = at_url(&url);
    let spend = |count: usize| {
        let mut inner = estimates.shared.inner();
        for _ in 0..count {
            inner.limits.lookups.push_back(Instant::now());
        }
    };
    spend(BACKGROUND_CAP - 3);
    healthy(&estimates);
    let parties = Parties(vec![
        upcoming("r1", 2, vec![ren(), mika()]),
        upcoming("r2", 3, vec![aoi()]),
    ]);
    estimates.refresh(&parties).await;
    until(|| fake.names().len() == 1).await;
    // Spent meanwhile: Mika's lookup no longer fits.
    spend(2);
    fake.open();
    until(|| !estimates.shared.inner().worker).await;
    assert_eq!(fake.names(), ["RenShade"]);
    assert!(estimates.latest(&run_target("r1")).is_none(), "stopped");
    assert!(estimates.latest(&run_target("r2")).is_none(), "dropped");
    assert!(
        estimates.cached(&ren().ign).is_some(),
        "what it found stays"
    );

    healthy(&estimates);
    estimates.refresh(&parties).await;
    tokio::time::sleep(CAP_WINDOW).await;
    healthy(&estimates);
    estimates.refresh(&parties).await;
    assert_eq!(field("estimate_refresh_pass", "cap_left"), [3, 0, 60]);
    assert_eq!(field("estimate_refresh_pass", "due"), [2, 2, 2]);
    assert_eq!(field("estimate_refresh_pass", "queued"), [2, 0, 2]);
    estimates.stop().await;
}

/// No pass while the sidecar is unreachable or asks for a verification.
#[tokio::test(start_paused = true)]
async fn a_pass_is_skipped_while_the_sidecar_is_unhealthy() {
    kanade_log::capture();
    let estimates = estimates(Arc::new(Mutex::new(at(4))));
    let parties = Parties(vec![upcoming("r1", 2, vec![ren()])]);
    // Nothing listens: the probe finds it unreachable.
    estimates.refresh(&parties).await;
    {
        let mut inner = estimates.shared.inner();
        inner.seen.unreachable = false;
        inner.seen.verification = true;
    }
    estimates.refresh(&parties).await;
    assert_eq!(
        field("estimate_refresh_pass", "skipped"),
        ["unreachable", "verification_required"]
    );
    assert_eq!(field("estimate_refresh_pass", "queued"), [0, 0]);
    assert!(estimates.running().is_none());
    assert!(estimates.shared.inner().background.is_empty());
}

/// A background job that finds the sidecar unreachable drops the
/// background jobs still waiting.
#[tokio::test(start_paused = true)]
async fn an_unreachable_sidecar_drops_the_waiting_background_jobs() {
    let estimates = estimates(Arc::new(Mutex::new(at(4))));
    healthy(&estimates);
    let parties = Parties(vec![
        upcoming("r1", 2, vec![ren()]),
        upcoming("r2", 3, vec![mika()]),
    ]);
    estimates.refresh(&parties).await;
    assert_eq!(settled(&estimates, "r1").await.state(), JobState::Failed);
    assert!(estimates.shared.inner().background.is_empty());
    tokio::time::sleep(BACKGROUND_GAP * 2).await;
    assert!(estimates.latest(&run_target("r2")).is_none(), "never run");
}

/// A run whose background job failed is not queued again for 24 hours.
#[tokio::test(start_paused = true)]
async fn a_failed_background_run_is_left_alone_for_a_day() {
    kanade_log::capture();
    let (fake, url) = Fake::start(true).await;
    fake.fail.store(true, Ordering::SeqCst);
    let estimates = at_url(&url);
    let parties = Parties(vec![upcoming("r1", 2, vec![ren()])]);
    healthy(&estimates);
    estimates.refresh(&parties).await;
    assert_eq!(settled(&estimates, "r1").await.state(), JobState::Failed);
    fake.fail.store(false, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(60 * 60)).await;
    healthy(&estimates);
    estimates.refresh(&parties).await;
    tokio::time::sleep(BACKOFF).await;
    healthy(&estimates);
    estimates.refresh(&parties).await;
    assert_eq!(field("estimate_refresh_pass", "due"), [1, 1, 1]);
    assert_eq!(field("estimate_refresh_pass", "queued"), [1, 0, 1]);
    let done = settled(&estimates, "r1").await;
    assert_eq!(done.state(), JobState::Done);
}

/// A waiting background job is joined and moved up even while the manual
/// queue is full: it exists already, so only new jobs are refused.
#[tokio::test(start_paused = true)]
async fn a_waiting_background_run_is_joined_with_the_manual_queue_full() {
    let (fake, url) = Fake::start(false).await;
    let estimates = at_url(&url);
    healthy(&estimates);
    let parties = Parties(vec![
        upcoming("r1", 2, vec![ren()]),
        upcoming("r2", 3, vec![mika()]),
    ]);
    estimates.refresh(&parties).await;
    until(|| fake.names().len() == 1).await;
    for n in 0..MAX_QUEUED {
        let member = subject(&format!("ch-1{n:02}"), &format!("Member{n}"));
        assert!(matches!(
            estimates.start(characters_target(&member)),
            Start::Started(_)
        ));
    }
    let Start::Joined(moved) = estimates.start(lookup(run_target("r2"), vec![mika()])) else {
        panic!("joined, not refused");
    };
    assert_eq!(moved.position, Some(MAX_QUEUED + 1), "at the end");
    assert!(estimates.shared.inner().background.is_empty());
    estimates.stop().await;
}

/// After `stop` nothing starts a worker: a lookup is kept queued (memory
/// still answers at once) and a pass does nothing.
#[tokio::test(start_paused = true)]
async fn nothing_starts_after_stop() {
    kanade_log::capture();
    let estimates = estimates(Arc::new(Mutex::new(at(4))));
    estimates.stop().await;
    let Start::Started(job) = estimates.start(characters_target(&ren())) else {
        panic!("a new job");
    };
    assert_eq!(job.state(), JobState::Queued);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(
        estimates.job(&job.id).map(|job| job.state()),
        Some(JobState::Queued)
    );
    assert!(estimates.running().is_none());
    assert!(!estimates.shared.inner().worker);
    assert!(
        estimates
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_none(),
        "no task to outlive the store"
    );
    healthy(&estimates);
    let parties = Parties(vec![upcoming("r1", 2, vec![mika()])]);
    estimates.refresh(&parties).await;
    assert!(estimates.shared.inner().background.is_empty());
    assert!(lines("estimate_refresh_pass").is_empty());
    // Memory still answers.
    estimates
        .shared
        .keep_member(&aoi(), &member("AoiLyn", 40.0))
        .await;
    hit(estimates.start(characters_target(&aoi())));
}

/// When the sidecar asks for a verification while the worker waits out
/// the gap, the waiting background jobs go at once.
#[tokio::test(start_paused = true)]
async fn an_unhealthy_answer_drops_waiting_background_jobs_at_once() {
    let (_fake, url) = Fake::start(true).await;
    let estimates = at_url(&url);
    healthy(&estimates);
    let parties = Parties(vec![
        upcoming("r1", 2, vec![ren()]),
        upcoming("r2", 3, vec![mika()]),
    ]);
    estimates.refresh(&parties).await;
    settled(&estimates, "r1").await;
    assert!(estimates.looking_up(&[mika().ign]), "r2 waits out the gap");
    estimates.shared.saw(&Err::<(), _>(LookupError::Refused {
        status: 503,
        code: "verification_required".into(),
        ign: None,
        retry_after: None,
    }));
    assert!(!estimates.looking_up(&[mika().ign]), "dropped at once");
    assert!(estimates.shared.inner().background.is_empty());
    estimates.stop().await;
}
