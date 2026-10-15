use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::TimeZone;

use super::*;
use crate::{
    domain::characters::{Character, CharacterRole},
    infrastructure::{scouter::Card, store::SqliteCharData},
    runtime::logging,
};

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
    logging::captured()
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
    logging::capture();
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
    logging::capture();
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
    logging::capture();
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
