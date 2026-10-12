//! The service against a fake Open API on loopback (axum) with wholly
//! invented fixtures: pending requests that call nothing, confirmation,
//! every error code's backoff, ocid re-resolution, empty answers, the
//! budget, unlink, the orphan pass, and that no Nexon field reaches a log
//! line or a History row.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use super::*;
use crate::domain::characters::{CharacterChange, CharacterRecord, CharacterRole};
use crate::domain::chardata::{
    CharDataStore, DAILY_CALLS, Link, LinkStatus, NOTICE_VERSION, Post, REFRESH_CAP,
};
use crate::domain::history::{Actor, Surface};
use crate::domain::settings::SettingsChangeQuery;
use crate::infrastructure::nexon::{ApiKey, KEY_HEADER};
use crate::infrastructure::store::members_conformance::gateway;
use crate::infrastructure::store::{MemoryCharData, MemoryScheduleStore};

const IGN: &str = "Wisteria";
const OCID: &str = "ocid-invented-0001";
const OCID_NEW: &str = "ocid-invented-0002";
const CP: &str = "31,415,926";
const KEY: &str = "invented-test-key";

fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap()
}

fn basic(name: &str) -> String {
    serde_json::json!({
        "date": "2026-10-11T00:00+08:00",
        "character_name": name,
        "world_name": "Aquila",
        "character_class": "Bishop",
        "character_class_level": "6",
        "character_level": 271,
        "character_exp": 123,
        "character_gender": "Female",
        "character_guild_name": "InventedGuild",
        "character_image": "https://open.api.nexon.com/static/maplestorysea/character/look/invented",
        "character_date_create": "2025-05-01T00:00+08:00",
        "access_flag": "true"
    })
    .to_string()
}

fn ok_body(endpoint: &str) -> String {
    match endpoint {
        "id" => format!(r#"{{"ocid":"{OCID}"}}"#),
        "basic" => basic(IGN),
        _ => format!(
            r#"{{"date":null,"character_class":"Bishop","final_stat":[{{"stat_name":"Max HP","stat_value":"99999"}},{{"stat_name":"Combat Power","stat_value":"{CP}"}}],"remain_ap":0}}"#
        ),
    }
}

fn error(code: &str, status: u16) -> (u16, String) {
    (
        status,
        format!(r#"{{"error":{{"name":"{code}","message":"invented"}}}}"#),
    )
}

/// The fake Open API: scripted answers per endpoint (else a success) and a
/// log of every call with the key header it carried.
#[derive(Default)]
struct Fake {
    script: Mutex<HashMap<&'static str, VecDeque<(u16, String)>>>,
    calls: Mutex<Vec<(String, Option<String>)>>,
}

impl Fake {
    fn queue(&self, endpoint: &'static str, answers: impl IntoIterator<Item = (u16, String)>) {
        self.script
            .lock()
            .unwrap()
            .entry(endpoint)
            .or_default()
            .extend(answers);
    }

    fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(endpoint, _)| endpoint.clone())
            .collect()
    }
}

async fn answer(State(fake): State<Arc<Fake>>, request: Request) -> impl IntoResponse {
    let path = request.uri().path().to_owned();
    let endpoint: &'static str = if path.ends_with("/v1/id") {
        "id"
    } else if path.ends_with("/character/basic") {
        "basic"
    } else if path.ends_with("/character/stat") {
        "stat"
    } else {
        "unknown"
    };
    let key = request
        .headers()
        .get(KEY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    fake.calls.lock().unwrap().push((endpoint.into(), key));
    let scripted = fake
        .script
        .lock()
        .unwrap()
        .get_mut(endpoint)
        .and_then(VecDeque::pop_front);
    let (status, body) = scripted.unwrap_or_else(|| (200, ok_body(endpoint)));
    (StatusCode::from_u16(status).unwrap(), body)
}

struct Rig {
    service: Arc<CharacterData>,
    file: Arc<MemoryCharData>,
    roster: Arc<MemoryScheduleStore>,
    flaky: Arc<FlakyRoster>,
    fake: Arc<Fake>,
    now: Arc<Mutex<DateTime<Utc>>>,
}

async fn rig_with(connected: bool) -> Rig {
    let fake = Arc::new(Fake::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().fallback(answer).with_state(Arc::clone(&fake));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let roster = Arc::new(MemoryScheduleStore::new());
    roster
        .apply_gateway(gateway("1001", "Alice", &["10"]))
        .await
        .unwrap();
    roster
        .apply_gateway(gateway("1002", "Bob", &["10"]))
        .await
        .unwrap();
    for (id, member, ign, world) in [
        ("c1", "1001", IGN, "msea"),
        ("c2", "1001", "Heather", "msea"),
        ("c3", "1002", "Lupin", "kms"),
    ] {
        roster
            .apply_character(
                CharacterChange::Create(crate::domain::characters::Character {
                    id: id.into(),
                    member_id: member.into(),
                    ign: ign.into(),
                    world: world.into(),
                    class: Some("Bishop".into()),
                    level: Some(250),
                    role: if id == "c2" {
                        CharacterRole::Alt
                    } else {
                        CharacterRole::Main
                    },
                    retired_at: None,
                    created_at: at(1, 0),
                    updated_at: at(1, 0),
                    updated_by: Actor::admin("token"),
                    version: 1,
                }),
                CharacterRecord {
                    at: at(1, 0),
                    actor: Actor::admin("token"),
                    surface: Surface::AdminPortal,
                },
            )
            .await
            .unwrap();
    }
    let file = Arc::new(MemoryCharData::new());
    let now = Arc::new(Mutex::new(at(11, 0)));
    let clock = {
        let now = Arc::clone(&now);
        Arc::new(move || *now.lock().unwrap()) as Clock
    };
    let nexon = connected.then(|| NexonClient::plain(address, ApiKey::new(KEY)));
    let flaky = Arc::new(FlakyRoster {
        inner: Arc::clone(&roster),
        fail_records: std::sync::atomic::AtomicBool::new(false),
    });
    let service = Arc::new(CharacterData::new(
        Arc::clone(&file) as Arc<dyn CharDataStore>,
        Arc::clone(&flaky) as Arc<dyn RosterSide>,
        nexon,
        clock,
        Timing {
            spacing: Duration::from_millis(1),
            rate_limit_unit: Duration::from_millis(1),
        },
    ));
    Rig {
        service,
        file,
        roster,
        flaky,
        fake,
        now,
    }
}

/// The roster side over the memory store, whose History writes can fail.
struct FlakyRoster {
    inner: Arc<MemoryScheduleStore>,
    fail_records: std::sync::atomic::AtomicBool,
}

impl RosterSide for FlakyRoster {
    fn character(
        &self,
        id: String,
    ) -> StoreFuture<'_, Option<crate::domain::characters::Character>> {
        RosterSide::character(&*self.inner, id)
    }

    fn member(
        &self,
        user_id: String,
    ) -> StoreFuture<'_, Option<crate::domain::members::MemberProfile>> {
        RosterSide::member(&*self.inner, user_id)
    }

    fn member_characters(
        &self,
        member_id: String,
    ) -> StoreFuture<'_, Vec<crate::domain::characters::Character>> {
        RosterSide::member_characters(&*self.inner, member_id)
    }

    fn record(
        &self,
        change: crate::domain::settings::SettingsChange,
        replay: Option<StoredReplay>,
    ) -> StoreFuture<'_, ()> {
        if self.fail_records.load(std::sync::atomic::Ordering::SeqCst) {
            return Box::pin(async {
                Err(crate::domain::scheduler::StoreError::Backend("down".into()))
            });
        }
        RosterSide::record(&*self.inner, change, replay)
    }

    fn last_link_audit(
        &self,
        character_id: String,
        actor: Actor,
    ) -> StoreFuture<'_, Option<crate::domain::settings::SettingsChange>> {
        RosterSide::last_link_audit(&*self.inner, character_id, actor)
    }
}

async fn rig() -> Rig {
    rig_with(true).await
}

impl Rig {
    fn set_now(&self, at: DateTime<Utc>) {
        *self.now.lock().unwrap() = at;
    }

    async fn link(&self, id: &str) -> Option<Link> {
        self.file.link(id.into()).await.unwrap()
    }

    async fn confirm(&self) -> ConfirmOutcome {
        self.service
            .confirm_link("1001", "c1", NOTICE_VERSION)
            .await
            .unwrap()
    }

    async fn history(&self) -> Vec<(String, String, String, String)> {
        let mut rows: Vec<_> = self
            .roster
            .settings_changes(SettingsChangeQuery::default())
            .await
            .unwrap()
            .into_iter()
            .filter(|change| change.values.keys().any(|key| key.starts_with("link.")))
            .flat_map(|change| {
                let actor = format!("{}:{}", change.actor.kind(), change.actor.id());
                change
                    .values
                    .into_iter()
                    .map(move |(key, diff)| (key, diff.from, diff.to, actor.clone()))
            })
            .collect();
        rows.reverse();
        rows
    }
}

fn nexon_free(text: &str) {
    for fixture in [
        IGN,
        "wisteria",
        OCID,
        OCID_NEW,
        "31415926",
        CP,
        "Aquila",
        "InventedGuild",
    ] {
        assert!(!text.contains(fixture), "{fixture} leaked into {text}");
    }
}

#[tokio::test]
async fn an_admin_request_waits_for_the_member_and_calls_nothing() {
    crate::runtime::logging::capture();
    let rig = rig().await;
    let admin = Actor::admin("token");
    let outcome = rig
        .service
        .request_link("c1", 0, &admin, Surface::AdminPortal, |_| None)
        .await
        .unwrap();
    let RequestOutcome::Requested(link) = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(link.status, LinkStatus::Pending);
    assert_eq!(link.consent_version, None);
    assert!(rig.fake.calls().is_empty(), "no Nexon call while pending");
    assert!(matches!(
        rig.service
            .request_link("c1", 1, &admin, Surface::AdminPortal, |_| None)
            .await
            .unwrap(),
        RequestOutcome::AlreadyLinked(_)
    ));
    assert!(matches!(
        rig.service
            .request_link("c2", 3, &admin, Surface::AdminPortal, |_| None)
            .await
            .unwrap(),
        RequestOutcome::Stale(None)
    ));
    assert_eq!(
        rig.service
            .request_link("c3", 0, &admin, Surface::AdminPortal, |_| None)
            .await
            .unwrap(),
        RequestOutcome::Region
    );
    assert_eq!(
        rig.service
            .request_link("c9", 0, &admin, Surface::AdminPortal, |_| None)
            .await
            .unwrap(),
        RequestOutcome::Missing
    );

    // Only the member, with the current notice, makes it active.
    assert_eq!(
        rig.service
            .confirm_link("1002", "c1", NOTICE_VERSION)
            .await
            .unwrap(),
        ConfirmOutcome::NotEligible
    );
    assert_eq!(
        rig.service
            .confirm_link("1001", "c1", NOTICE_VERSION + 1)
            .await
            .unwrap(),
        ConfirmOutcome::NoticeOutdated
    );
    let ConfirmOutcome::Linked(linked, fetched) = rig.confirm().await else {
        panic!("not linked");
    };
    assert_eq!(fetched, FetchState::Stored);
    assert_eq!(linked.status, LinkStatus::Active);
    assert_eq!(linked.consent_version, Some(NOTICE_VERSION));
    assert_eq!(linked.consented_at, Some(at(11, 0)));
    assert_eq!(linked.requested_by, admin, "the request stays the admin's");
    assert_eq!(rig.fake.calls(), ["id", "basic", "stat"]);
    assert!(
        rig.fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|(_, key)| key.as_deref() == Some(KEY))
    );
    let view = rig.service.view("c1").await.unwrap();
    let data = view.snapshot.unwrap();
    assert_eq!(data.ocid, OCID);
    assert_eq!(data.character_name.as_deref(), Some(IGN));
    assert_eq!(data.level, Some(271));
    assert_eq!(data.combat_power, Some(31_415_926));
    assert_eq!(
        data.data_date,
        chrono::NaiveDate::from_ymd_opt(2026, 10, 11)
    );
    assert_eq!(data.expires_at, at(11, 0) + TimeDelta::days(30));
    assert_eq!(rig.confirm().await, ConfirmOutcome::AlreadyLinked);

    assert_eq!(
        rig.history().await,
        [
            (
                "link.c1".into(),
                "unlinked".into(),
                "pending".into(),
                "admin:token".into()
            ),
            (
                "link.c1".into(),
                "pending".into(),
                "linked".into(),
                "member:1001".into()
            ),
        ]
    );
    // No Nexon field in a link's History row (the roster's own rows carry
    // the admin-entered IGN) or in any log line.
    for change in rig
        .roster
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|change| change.values.keys().all(|key| key.starts_with("link.")))
    {
        nexon_free(&format!("{change:?}"));
    }
    for line in crate::runtime::logging::captured() {
        nexon_free(&line.to_string());
    }
}

#[tokio::test]
async fn without_a_key_links_are_refused() {
    let rig = rig_with(false).await;
    assert_eq!(
        rig.service
            .request_link(
                "c1",
                0,
                &Actor::admin("token"),
                Surface::AdminPortal,
                |_| None
            )
            .await
            .unwrap(),
        RequestOutcome::NotConnected
    );
    assert_eq!(rig.confirm().await, ConfirmOutcome::NotConnected);
    assert_eq!(
        rig.service.refresh_now("c1").await.unwrap(),
        RefreshOutcome::NotConnected
    );
    let health = rig.service.health().await;
    assert_eq!(health.nexon, NexonState::NoKey);
    assert_eq!(rig.link("c1").await, None);
}

#[tokio::test]
async fn every_error_code_backs_off_as_documented() {
    // 00009: this character again in 15 minutes.
    let rig = rig().await;
    rig.fake.queue("basic", [error("OPENAPI00009", 400)]);
    let ConfirmOutcome::Linked(_, fetched) = rig.confirm().await else {
        panic!()
    };
    assert_eq!(
        fetched,
        FetchState::Failed {
            code: "OPENAPI00009".into(),
            retry_at: Some(at(11, 0) + TimeDelta::minutes(15)),
        }
    );
    assert_eq!(
        rig.service.next_retry().await,
        Some(at(11, 0) + TimeDelta::minutes(15))
    );
    // The retry pass picks it up once its time comes.
    rig.set_now(at(11, 1));
    rig.service.retry_due().await;
    assert!(rig.service.view("c1").await.unwrap().snapshot.is_some());

    // 00004 / 00006: logged, never retried.
    for code in ["OPENAPI00004", "OPENAPI00006"] {
        let rig = rig_with(true).await;
        rig.fake.queue("stat", [error(code, 400)]);
        let ConfirmOutcome::Linked(_, fetched) = rig.confirm().await else {
            panic!()
        };
        assert_eq!(
            fetched,
            FetchState::Failed {
                code: code.into(),
                retry_at: None
            }
        );
        assert_eq!(rig.fake.calls(), ["id", "basic", "stat"], "{code}: one try");
    }

    // 00010 / 00011 / 00001 / 5xx: every call held 30 minutes, then hourly.
    for (code, status) in [
        ("OPENAPI00010", 400),
        ("OPENAPI00011", 503),
        ("OPENAPI00001", 500),
    ] {
        let rig = rig_with(true).await;
        rig.fake.queue("id", [error(code, status)]);
        let ConfirmOutcome::Linked(_, fetched) = rig.confirm().await else {
            panic!()
        };
        let held = at(11, 0) + TimeDelta::minutes(30);
        assert_eq!(
            fetched,
            FetchState::Failed {
                code: code.into(),
                retry_at: Some(held)
            }
        );
        let health = rig.service.health().await;
        assert_eq!(
            (health.nexon, health.backoff_until),
            (NexonState::Backoff, Some(held))
        );
        rig.set_now(at(11, 0) + TimeDelta::days(2));
        assert_eq!(
            rig.service.refresh_now("c1").await.unwrap(),
            RefreshOutcome::Done(FetchState::Stored),
            "{code}: calls resume after the hold"
        );
    }
    let rig = rig_with(true).await;
    rig.fake.queue("id", [(502, "bad gateway".into())]);
    rig.confirm().await;
    rig.fake.queue("id", [(502, "bad gateway".into())]);
    rig.set_now(at(11, 0) + TimeDelta::minutes(31));
    assert!(matches!(
        rig.service.refresh_now("c1").await.unwrap(),
        RefreshOutcome::Done(FetchState::Failed { .. })
    ));
    assert_eq!(
        rig.service.health().await.backoff_until,
        Some(at(11, 0) + TimeDelta::minutes(91)),
        "then hourly"
    );

    // 00002 / 00005: no call at all until a restart.
    for code in ["OPENAPI00002", "OPENAPI00005"] {
        let rig = rig_with(true).await;
        rig.fake.queue("id", [error(code, 403)]);
        rig.confirm().await;
        assert_eq!(rig.service.health().await.nexon, NexonState::KeyRefused);
        let before = rig.fake.calls().len();
        assert_eq!(
            rig.service.refresh_now("c1").await.unwrap(),
            RefreshOutcome::Unavailable("key_refused".into())
        );
        rig.set_now(at(20, 0));
        let report = rig.service.refresh_due().await;
        assert_eq!(report.stopped.as_deref(), Some("key_refused"));
        assert_eq!(rig.fake.calls().len(), before, "{code}: nothing called");
    }

    // 00007: 1 s, 2 s with jitter, at most 3 tries.
    let rig = rig_with(true).await;
    rig.fake.queue(
        "basic",
        [error("OPENAPI00007", 429), error("OPENAPI00007", 429)],
    );
    let ConfirmOutcome::Linked(_, fetched) = rig.confirm().await else {
        panic!()
    };
    assert_eq!(fetched, FetchState::Stored, "the third try went through");
    let rig = rig_with(true).await;
    rig.fake
        .queue("basic", (0..3).map(|_| error("OPENAPI00007", 429)));
    let ConfirmOutcome::Linked(_, fetched) = rig.confirm().await else {
        panic!()
    };
    assert!(matches!(fetched, FetchState::Failed { .. }));
    assert_eq!(rig.fake.calls(), ["id", "basic", "basic", "basic"]);
}

#[tokio::test]
async fn a_second_429_ends_the_batch_for_the_day() {
    let rig = rig().await;
    for (id, member) in [("c1", "1001"), ("c2", "1001")] {
        rig.service
            .confirm_link(member, id, NOTICE_VERSION)
            .await
            .unwrap();
    }
    rig.fake
        .queue("basic", [(429, String::new()), (429, String::new())]);
    rig.set_now(at(12, 0));
    let before = rig.fake.calls().len();
    let report = rig.service.refresh_due().await;
    assert_eq!(report.stopped.as_deref(), Some("OPENAPI00007"));
    assert_eq!(
        rig.fake.calls().len() - before,
        2,
        "no call after the second 429"
    );
    assert_eq!(rig.service.health().await.last_batch, Some(report));
}

#[tokio::test]
async fn a_changed_ocid_is_resolved_again_once_then_the_link_is_stale() {
    let rig = rig().await;
    rig.confirm().await;
    // 29 days on the ocid is renewed before the data calls.
    rig.set_now(at(11, 0) + TimeDelta::days(29) + TimeDelta::hours(1));
    rig.fake
        .queue("id", [(200, format!(r#"{{"ocid":"{OCID_NEW}"}}"#))]);
    let before = rig.fake.calls().len();
    rig.service.refresh_now("c1").await.unwrap();
    assert_eq!(rig.fake.calls()[before..], ["id", "basic", "stat"]);
    assert_eq!(
        rig.service.view("c1").await.unwrap().snapshot.unwrap().ocid,
        OCID_NEW
    );

    // 00003 on the data: resolved again once, then the data call repeats.
    let rig = rig_with(true).await;
    rig.confirm().await;
    rig.set_now(at(12, 1));
    rig.fake.queue("basic", [error("OPENAPI00003", 400)]);
    rig.fake
        .queue("id", [(200, format!(r#"{{"ocid":"{OCID_NEW}"}}"#))]);
    let before = rig.fake.calls().len();
    assert_eq!(
        rig.service.refresh_now("c1").await.unwrap(),
        RefreshOutcome::Done(FetchState::Stored)
    );
    assert_eq!(rig.fake.calls()[before..], ["basic", "id", "basic", "stat"]);

    // Twice: stale, its data gone, and no loop.
    let rig = rig_with(true).await;
    rig.confirm().await;
    rig.set_now(at(12, 1));
    rig.fake.queue(
        "basic",
        [error("OPENAPI00003", 400), error("OPENAPI00003", 400)],
    );
    let before = rig.fake.calls().len();
    assert_eq!(
        rig.service.refresh_now("c1").await.unwrap(),
        RefreshOutcome::Done(FetchState::Stale)
    );
    assert_eq!(rig.fake.calls()[before..], ["basic", "id", "basic"]);
    assert_eq!(rig.link("c1").await.unwrap().status, LinkStatus::Stale);
    assert_eq!(rig.service.view("c1").await.unwrap().snapshot, None);
    // An admin may ask again: pending, no call.
    let calls = rig.fake.calls().len();
    let version = rig.link("c1").await.unwrap().version;
    assert!(matches!(
        rig.service
            .request_link(
                "c1",
                version,
                &Actor::admin("token"),
                Surface::AdminPortal,
                |_| None
            )
            .await
            .unwrap(),
        RequestOutcome::Requested(_)
    ));
    assert_eq!(rig.fake.calls().len(), calls);
}

#[tokio::test]
async fn another_name_or_no_character_makes_the_link_stale() {
    let rig = rig().await;
    rig.fake.queue("basic", [(200, basic("SomeoneElse"))]);
    let ConfirmOutcome::Linked(_, fetched) = rig.confirm().await else {
        panic!()
    };
    assert_eq!(fetched, FetchState::Stale);
    // Letter case alone is the same character.
    let rig = rig_with(true).await;
    rig.fake.queue("basic", [(200, basic("WISTERIA"))]);
    let ConfirmOutcome::Linked(_, fetched) = rig.confirm().await else {
        panic!()
    };
    assert_eq!(fetched, FetchState::Stored);
    let rig = rig_with(true).await;
    rig.fake.queue("id", [(200, r#"{"ocid":null}"#.into())]);
    let ConfirmOutcome::Linked(_, fetched) = rig.confirm().await else {
        panic!()
    };
    assert_eq!(fetched, FetchState::Stale);
    // Live MapleSEA answers an unknown name 400 OPENAPI00004, once.
    let rig = rig_with(true).await;
    rig.fake.queue("id", [error("OPENAPI00004", 400)]);
    let ConfirmOutcome::Linked(_, fetched) = rig.confirm().await else {
        panic!()
    };
    assert_eq!(fetched, FetchState::Stale);
    assert_eq!(rig.fake.calls(), ["id"]);
}

#[tokio::test]
async fn an_empty_answer_is_no_data() {
    let rig = rig().await;
    rig.fake.queue(
        "basic",
        [(200, r#"{"date":"2026-10-11T00:00+08:00","character_name":null,"world_name":null,"character_class":null,"character_level":null,"character_image":null}"#.into())],
    );
    rig.fake
        .queue("stat", [(200, r#"{"date":null,"final_stat":[]}"#.into())]);
    let ConfirmOutcome::Linked(_, fetched) = rig.confirm().await else {
        panic!()
    };
    assert_eq!(fetched, FetchState::Stored);
    let snapshot = rig.service.view("c1").await.unwrap().snapshot.unwrap();
    assert!(!snapshot.has_data());
    assert_eq!(snapshot.combat_power, None);
}

#[tokio::test]
async fn the_budget_and_the_24_hour_cap_hold() {
    let rig = rig().await;
    rig.confirm().await;
    assert_eq!(
        rig.service.refresh_now("c1").await.unwrap(),
        RefreshOutcome::TooSoon(at(11, 0) + REFRESH_CAP)
    );
    rig.set_now(at(12, 1));
    for n in 0..DAILY_CALLS {
        rig.file
            .reserve_call(at(12, 0) + TimeDelta::seconds(i64::from(n)), DAILY_CALLS)
            .await
            .unwrap();
    }
    let before = rig.fake.calls().len();
    assert_eq!(
        rig.service.refresh_now("c1").await.unwrap(),
        RefreshOutcome::Done(FetchState::Failed {
            code: "budget".into(),
            retry_at: None
        })
    );
    assert_eq!(rig.fake.calls().len(), before, "the hard stop holds");
    assert_eq!(rig.service.health().await.calls_24h, Some(DAILY_CALLS));
}

#[tokio::test]
async fn unlink_deletes_at_once_and_makes_posts_due() {
    crate::runtime::logging::capture();
    let rig = rig().await;
    rig.confirm().await;
    let post = Post::new("abc123".into(), "900".into(), "c1".into(), at(11, 0));
    rig.service.store().create_post(post).await.unwrap();
    let removed = rig
        .service
        .unlink(
            "c1",
            &Actor::admin("token"),
            Surface::AdminPortal,
            crate::domain::chardata::UnlinkReason::Requested,
            None,
        )
        .await
        .unwrap();
    assert!(removed.is_some());
    assert_eq!(rig.link("c1").await, None);
    assert_eq!(rig.service.view("c1").await.unwrap().snapshot, None);
    let posts = rig.file.posts().await.unwrap();
    assert!(posts[0].is_due(at(11, 0)), "its post is due now");
    // The sweeper was woken.
    tokio::time::timeout(Duration::from_secs(1), rig.service.woken())
        .await
        .expect("woken");
    let last = rig.history().await.pop().unwrap();
    assert_eq!(
        last,
        (
            "link.c1".into(),
            "linked".into(),
            "unlinked".into(),
            "admin:token".into()
        )
    );
    for line in crate::runtime::logging::captured() {
        nexon_free(&line.to_string());
    }
}

#[tokio::test]
async fn roster_edits_unlink_and_the_orphan_pass_catches_the_rest() {
    let rig = rig().await;
    rig.confirm().await;
    rig.service
        .confirm_link("1001", "c2", NOTICE_VERSION)
        .await
        .unwrap();
    let mut renamed = CharacterStore::character(&*rig.roster, "c1")
        .await
        .unwrap()
        .unwrap();
    // Same IGN in another case: kept.
    renamed.ign = "WISTERIA".into();
    rig.service
        .roster_changed(
            "c1",
            Some(&renamed),
            &Actor::admin("token"),
            Surface::AdminPortal,
        )
        .await;
    assert!(rig.link("c1").await.is_some());
    renamed.ign = "Lavender".into();
    rig.service
        .roster_changed(
            "c1",
            Some(&renamed),
            &Actor::admin("token"),
            Surface::AdminPortal,
        )
        .await;
    assert_eq!(rig.link("c1").await, None, "IGN edit unlinks");

    // Member departure, caught by the sweeper.
    assert!(rig.roster.member_departed("1001").await.unwrap());
    let report = rig.service.sweep().await;
    assert_eq!(report.unlinked, 1);
    assert_eq!(rig.link("c2").await, None);
    assert!(rig.service.health().await.last_sweep.is_some());

    // An older notice sends a link back to pending, its data gone.
    let rig = rig_with(true).await;
    rig.confirm().await;
    let mut old = rig.link("c1").await.unwrap();
    let version = old.version;
    old.consent_version = Some(NOTICE_VERSION - 1);
    old.version += 1;
    rig.file.put_link(old, version).await.unwrap();
    let report = rig.service.sweep().await;
    assert_eq!(report.reconsent, 1);
    assert_eq!(rig.link("c1").await.unwrap().status, LinkStatus::Pending);
    assert_eq!(rig.service.view("c1").await.unwrap().snapshot, None);
    assert_eq!(
        rig.history().await.pop().unwrap(),
        (
            "link.c1".into(),
            "linked".into(),
            "pending".into(),
            "system:chardata".into()
        )
    );
}

/// Rename the roster character `id` to `ign` in the main store.
async fn rename(rig: &Rig, id: &str, ign: &str) -> crate::domain::characters::Character {
    let current = CharacterStore::character(&*rig.roster, id)
        .await
        .unwrap()
        .unwrap();
    let mut after = current.clone();
    after.ign = ign.into();
    let after = after.next_version(at(11, 1), &Actor::admin("token"));
    rig.roster
        .apply_character(
            CharacterChange::Update {
                version: current.version,
                after: after.clone(),
            },
            CharacterRecord {
                at: at(11, 1),
                actor: Actor::admin("token"),
                surface: Surface::AdminPortal,
            },
        )
        .await
        .unwrap();
    after
}

/// An IGN edit whose post-commit unlink fails: reads fail closed (no data,
/// no link) although the rows are still in the file, and the sweep removes
/// them once the file works again.
#[tokio::test]
async fn a_failed_unlink_after_an_ign_edit_never_serves_data() {
    let rig = rig().await;
    rig.confirm().await;
    let renamed = rename(&rig, "c1", "Lavender").await;
    rig.file.fail_unlinks(true);
    rig.service
        .roster_changed(
            "c1",
            Some(&renamed),
            &Actor::admin("token"),
            Surface::AdminPortal,
        )
        .await;
    assert!(rig.link("c1").await.is_some(), "the unlink failed");
    assert!(
        rig.file
            .snapshot("c1".into(), at(11, 1))
            .await
            .unwrap()
            .is_some(),
        "the data is still in the file"
    );
    let view = rig.service.view("c1").await.unwrap();
    assert_eq!((view.link, view.snapshot), (None, None), "never served");
    assert_eq!(rig.service.data("c1").await.unwrap(), None);
    rig.file.fail_unlinks(false);
    let report = rig.service.sweep().await;
    assert_eq!(report.unlinked, 1);
    assert_eq!(rig.link("c1").await, None);
    assert_eq!(
        rig.file.snapshot("c1".into(), at(11, 1)).await.unwrap(),
        None
    );
}

/// A notice re-pend whose History write fails changes nothing, logs the
/// character, and is done (with its row) by the next sweep.
#[tokio::test]
async fn a_failed_reconsent_audit_is_retried_by_the_next_sweep() {
    crate::runtime::logging::capture();
    let rig = rig().await;
    rig.confirm().await;
    let mut old = rig.link("c1").await.unwrap();
    let version = old.version;
    old.consent_version = Some(NOTICE_VERSION - 1);
    old.version += 1;
    rig.file.put_link(old, version).await.unwrap();
    rig.flaky
        .fail_records
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let report = rig.service.sweep().await;
    assert_eq!(report.reconsent, 0);
    assert_eq!(rig.link("c1").await.unwrap().status, LinkStatus::Active);
    let warned = crate::runtime::logging::captured()
        .into_iter()
        .any(|line| line["event"] == "chardata_reconsent_unrecorded" && line["character"] == "c1");
    assert!(warned);
    rig.flaky
        .fail_records
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(rig.service.sweep().await.reconsent, 1);
    assert_eq!(rig.link("c1").await.unwrap().status, LinkStatus::Pending);
    assert_eq!(
        rig.history().await.pop().unwrap(),
        (
            "link.c1".into(),
            "linked".into(),
            "pending".into(),
            "system:chardata".into()
        )
    );
}

/// A notice bump whose re-pend write fails: the link reads as pending and
/// no data is served or fetched although the file still says active, the
/// failure is logged, and the retry appends no second History row.
#[tokio::test]
async fn a_failed_repend_write_serves_nothing_and_audits_once() {
    crate::runtime::logging::capture();
    let rig = rig().await;
    rig.confirm().await;
    let mut old = rig.link("c1").await.unwrap();
    let version = old.version;
    old.consent_version = Some(NOTICE_VERSION - 1);
    old.version += 1;
    rig.file.put_link(old, version).await.unwrap();
    rig.file.fail_put_links(true);
    assert_eq!(rig.service.sweep().await.reconsent, 0);
    assert_eq!(rig.link("c1").await.unwrap().status, LinkStatus::Active);
    let view = rig.service.view("c1").await.unwrap();
    assert_eq!(view.link.unwrap().status, LinkStatus::Pending);
    assert_eq!(view.snapshot, None, "no data after a notice bump");
    assert_eq!(rig.service.data("c1").await.unwrap(), None);
    rig.set_now(at(13, 0));
    let calls = rig.fake.calls().len();
    assert_eq!(
        rig.service.refresh_now("c1").await.unwrap(),
        RefreshOutcome::NotLinked
    );
    rig.service.refresh_due().await;
    assert_eq!(rig.fake.calls().len(), calls, "no Nexon call");
    let warned = crate::runtime::logging::captured()
        .into_iter()
        .any(|line| line["event"] == "chardata_reconsent_unwritten" && line["character"] == "c1");
    assert!(warned);

    rig.file.fail_put_links(false);
    assert_eq!(rig.service.sweep().await.reconsent, 1);
    assert_eq!(rig.link("c1").await.unwrap().status, LinkStatus::Pending);
    let repends = rig
        .history()
        .await
        .into_iter()
        .filter(|(_, from, to, _)| from == "linked" && to == "pending")
        .count();
    assert_eq!(repends, 1, "one History row for the re-pend");
}

/// A Discord side that answers every strip with `outcome` and finds
/// nothing by ref.
struct Desk(Stripped);

impl PostDesk for Desk {
    fn strip<'a>(&'a self, _: &'a str, _: &'a str) -> DeskFuture<'a, Stripped> {
        let outcome = self.0;
        Box::pin(async move { outcome })
    }

    fn locate<'a>(&'a self, _: &'a str, _: &'a str) -> DeskFuture<'a, Located> {
        Box::pin(async { Located::Missing })
    }
}

/// Tracking and stripping share cards log refs, channels and counts only.
#[tokio::test]
async fn the_post_pipeline_logs_no_nexon_field() {
    crate::runtime::logging::capture();
    let rig = rig().await;
    rig.confirm().await;
    let Tracked::Ready(post, snapshot) = rig.service.track_post("c1", "900").await.unwrap() else {
        panic!("fresh data to share")
    };
    assert_eq!(snapshot.character_name.as_deref(), Some(IGN));
    assert_eq!(post.reference.len(), 6);
    // Not sent yet and past the grace: looked for, not found, unknown.
    rig.set_now(at(11, 0) + TimeDelta::days(29));
    rig.service.attach_posts(Arc::new(Desk(Stripped::Stuck)));
    let report = rig.service.strip_posts().await;
    assert_eq!(report.unknown, 1);
    rig.set_now(at(11, 0));
    let Tracked::Ready(stuck, _) = rig.service.track_post("c1", "900").await.unwrap() else {
        panic!()
    };
    rig.service
        .post_sent(&stuck.reference, "4242")
        .await
        .unwrap();
    rig.set_now(at(11, 0) + TimeDelta::days(29));
    let report = rig.service.strip_posts().await;
    assert_eq!(report.stuck, 1);
    let health = rig.service.health().await;
    assert_eq!(health.post_issues.len(), 2);
    for line in crate::runtime::logging::captured() {
        nexon_free(&line.to_string());
    }
    // Unlinked data is never tracked.
    rig.set_now(at(11, 0));
    rig.service
        .unlink(
            "c1",
            &Actor::member("1001"),
            Surface::Discord,
            crate::domain::chardata::UnlinkReason::Requested,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        rig.service.track_post("c1", "900").await.unwrap(),
        Tracked::NoData
    );
}

/// A confirmation the budget or a hold cannot serve writes nothing.
#[tokio::test]
async fn a_confirmation_nexon_cannot_serve_writes_nothing() {
    let rig = rig().await;
    for n in 0..DAILY_CALLS - 2 {
        rig.file
            .reserve_call(at(11, 0) - TimeDelta::seconds(i64::from(n)), DAILY_CALLS)
            .await
            .unwrap();
    }
    assert_eq!(rig.confirm().await, ConfirmOutcome::Busy);
    assert_eq!(rig.link("c1").await, None);
    assert!(rig.fake.calls().is_empty());
}
