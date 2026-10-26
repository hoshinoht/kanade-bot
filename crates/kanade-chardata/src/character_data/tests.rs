//! The service against a fake Open API on loopback (axum) with wholly
//! invented fixtures: admin links (active at once) and member confirmation,
//! every error code's backoff, ocid re-resolution, empty answers, the
//! budget, unlink, the orphan pass, the roster's copy of NEXON's facts, the
//! startup passes, the sweep of scouter estimates, and that no Nexon field
//! reaches a log line or a link's History row.

use std::collections::{BTreeMap, HashMap, VecDeque};
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
use crate::nexon::{ApiKey, KEY_HEADER};
use crate::scouter::{RosterIgn, ScouterClient};
use kanade_domain::characters::{CharacterChange, CharacterRecord, CharacterRole, CharacterWrite};
use kanade_domain::chardata::{
    CharDataStore, ConsentSurface, DAILY_CALLS, Link, LinkStatus, MemberEstimate, NOTICE_VERSION,
    PartyEstimate, Post, StoredMember,
};
use kanade_domain::history::{Actor, Surface};
use kanade_domain::settings::SettingsChangeQuery;
use kanade_store::members_conformance::gateway;
use kanade_store::{SqliteCharData, SqliteStore};

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
    file: Arc<SqliteCharData>,
    roster: Arc<SqliteStore>,
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
    let roster = Arc::new(SqliteStore::open_temp().await);
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
                CharacterChange::Create(kanade_domain::characters::Character {
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
    let file = Arc::new(SqliteCharData::open_temp().await);
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

/// The roster side over a temp SQLite store, whose History writes can fail.
struct FlakyRoster {
    inner: Arc<SqliteStore>,
    fail_records: std::sync::atomic::AtomicBool,
}

impl RosterSide for FlakyRoster {
    fn character(
        &self,
        id: String,
    ) -> StoreFuture<'_, Option<kanade_domain::characters::Character>> {
        RosterSide::character(&*self.inner, id)
    }

    fn member(
        &self,
        user_id: String,
    ) -> StoreFuture<'_, Option<kanade_domain::members::MemberProfile>> {
        RosterSide::member(&*self.inner, user_id)
    }

    fn member_characters(
        &self,
        member_id: String,
    ) -> StoreFuture<'_, Vec<kanade_domain::characters::Character>> {
        RosterSide::member_characters(&*self.inner, member_id)
    }

    fn write_character(
        &self,
        change: CharacterChange,
        record: CharacterRecord,
    ) -> StoreFuture<'_, kanade_domain::characters::CharacterWrite> {
        RosterSide::write_character(&*self.inner, change, record)
    }

    fn record(
        &self,
        change: kanade_domain::settings::SettingsChange,
        replay: Option<StoredReplay>,
    ) -> StoreFuture<'_, ()> {
        if self.fail_records.load(std::sync::atomic::Ordering::SeqCst) {
            return Box::pin(async {
                Err(kanade_domain::scheduler::StoreError::Backend("down".into()))
            });
        }
        RosterSide::record(&*self.inner, change, replay)
    }

    fn last_link_audit(
        &self,
        character_id: String,
        actor: Actor,
    ) -> StoreFuture<'_, Option<kanade_domain::settings::SettingsChange>> {
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

    /// Make `id`'s link consented to a notice older than the current one.
    /// None predates version 1, so only a tampered row can say so.
    async fn older_notice(&self, id: &str) {
        let sql = format!(
            "PRAGMA ignore_check_constraints = ON;
             UPDATE links SET consent_version = {}, version = version + 1
             WHERE character_id = '{id}';",
            NOTICE_VERSION - 1
        );
        self.file.temp_dir().raw(&sql).await.expect("link tampered");
    }

    async fn confirm(&self) -> ConfirmOutcome {
        self.service
            .confirm_link(
                "1001",
                "c1",
                NOTICE_VERSION,
                kanade_domain::chardata::ConsentSurface::Discord,
            )
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

/// User decision 2026-10-13: an admin's link is active at once, with no
/// member confirmation, and fetches.
#[tokio::test]
async fn an_admin_link_is_active_at_once_and_fetches() {
    kanade_log::capture();
    let rig = rig().await;
    let admin = Actor::admin("token");
    let admin_link = async |id: &str, version: u32| {
        rig.service
            .admin_link(id, version, &admin, Surface::AdminPortal)
            .await
            .unwrap()
    };
    let AdminLinkOutcome::Linked(link, fetched) = admin_link("c1", 0).await else {
        panic!("not linked");
    };
    assert_eq!(fetched, FetchState::Stored);
    assert_eq!(link.status, LinkStatus::Active);
    assert_eq!(link.requested_by, admin);
    assert_eq!(
        (link.consent_version, link.consented_at, link.linked_at),
        (Some(NOTICE_VERSION), Some(at(11, 0)), Some(at(11, 0))),
        "the notice in force, and when"
    );
    assert_eq!(link.consent_surface, None, "the member gave no consent");
    assert!(link.admin_linked());
    assert_eq!(rig.link("c1").await, Some((*link).clone()));
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
    let row = roster_row(&rig, "c1").await;
    assert_eq!(
        (row.level, &row.updated_by),
        (Some(271), &Actor::system(NEXON_ACTOR)),
        "NEXON's level is copied into the roster"
    );

    // Linked or not, it is the admin's call; the member is not asked.
    assert!(matches!(
        admin_link("c1", link.version).await,
        AdminLinkOutcome::AlreadyLinked(_)
    ));
    assert_eq!(rig.confirm().await, ConfirmOutcome::AlreadyLinked);
    assert!(matches!(
        admin_link("c2", 3).await,
        AdminLinkOutcome::Stale(None)
    ));
    assert_eq!(admin_link("c3", 0).await, AdminLinkOutcome::Region);
    assert_eq!(admin_link("c9", 0).await, AdminLinkOutcome::Missing);
    assert_eq!(rig.fake.calls().len(), 3, "no call for a refused link");

    assert_eq!(
        rig.history().await,
        [(
            "link.c1".into(),
            "unlinked".into(),
            "linked".into(),
            "admin:token".into()
        )]
    );
    // No Nexon field in a link's History row (the roster's own rows carry
    // roster values) or in any log line.
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
    for line in kanade_log::captured() {
        nexon_free(&line.to_string());
    }
}

/// A member's own link is unchanged: their consent to the current notice
/// (and where they gave it) is recorded, then the first fetch runs.
#[tokio::test]
async fn a_member_confirms_their_own_link() {
    let rig = rig().await;
    assert_eq!(
        rig.service
            .confirm_link("1002", "c1", NOTICE_VERSION, ConsentSurface::Discord)
            .await
            .unwrap(),
        ConfirmOutcome::NotEligible
    );
    assert_eq!(
        rig.service
            .confirm_link("1001", "c1", NOTICE_VERSION + 1, ConsentSurface::Discord)
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
    assert_eq!(linked.consent_surface, Some(ConsentSurface::Discord));
    assert_eq!(linked.requested_by, Actor::member("1001"));
    assert!(!linked.admin_linked());
    assert_eq!(rig.fake.calls(), ["id", "basic", "stat"]);
    assert_eq!(rig.confirm().await, ConfirmOutcome::AlreadyLinked);
    assert_eq!(
        rig.history().await,
        [(
            "link.c1".into(),
            "unlinked".into(),
            "linked".into(),
            "member:1001".into()
        )]
    );
}

#[tokio::test]
async fn without_a_key_links_are_refused() {
    let rig = rig_with(false).await;
    assert_eq!(
        rig.service
            .admin_link("c1", 0, &Actor::admin("token"), Surface::AdminPortal)
            .await
            .unwrap(),
        AdminLinkOutcome::NotConnected
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
            .confirm_link(
                member,
                id,
                NOTICE_VERSION,
                kanade_domain::chardata::ConsentSurface::Discord,
            )
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
    // An admin links it again: active at once, and it fetches.
    let calls = rig.fake.calls().len();
    let version = rig.link("c1").await.unwrap().version;
    assert!(matches!(
        rig.service
            .admin_link("c1", version, &Actor::admin("token"), Surface::AdminPortal)
            .await
            .unwrap(),
        AdminLinkOutcome::Linked(_, FetchState::Stored)
    ));
    assert_eq!(rig.fake.calls()[calls..], ["id", "basic", "stat"]);
    assert_eq!(rig.link("c1").await.unwrap().status, LinkStatus::Active);
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
async fn an_admin_refresh_runs_at_once_until_the_budget_stops_it() {
    let rig = rig().await;
    rig.confirm().await;
    let before = rig.fake.calls().len();
    assert_eq!(
        rig.service.refresh_now("c1").await.unwrap(),
        RefreshOutcome::Done(FetchState::Stored),
        "no per-character wait after the link's own fetch"
    );
    assert_eq!(rig.fake.calls()[before..], ["basic", "stat"]);
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
    kanade_log::capture();
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
            kanade_domain::chardata::UnlinkReason::Requested,
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
    for line in kanade_log::captured() {
        nexon_free(&line.to_string());
    }
}

#[tokio::test]
async fn roster_edits_unlink_and_the_orphan_pass_catches_the_rest() {
    let rig = rig().await;
    rig.confirm().await;
    rig.service
        .confirm_link(
            "1001",
            "c2",
            NOTICE_VERSION,
            kanade_domain::chardata::ConsentSurface::Discord,
        )
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
    rig.older_notice("c1").await;
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
async fn rename(rig: &Rig, id: &str, ign: &str) -> kanade_domain::characters::Character {
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

/// User decision: a retired character keeps its NEXON data until it expires
/// (refresh paused), so the strip can say when it will be deleted; then it
/// reads as gone and the sweep removes the link.
#[tokio::test]
async fn a_retired_character_keeps_its_data_until_it_expires() {
    let rig = rig().await;
    // c2 "Heather", an alt (a main cannot be retired while it is the main).
    rig.fake.queue("basic", [(200, basic("Heather"))]);
    let ConfirmOutcome::Linked(_, FetchState::Stored) = rig
        .service
        .confirm_link(
            "1001",
            "c2",
            NOTICE_VERSION,
            kanade_domain::chardata::ConsentSurface::Discord,
        )
        .await
        .unwrap()
    else {
        panic!("c2 linked with data");
    };
    let current = roster_row(&rig, "c2").await;
    let mut retired = current.clone();
    retired.retired_at = Some(at(11, 1));
    let retired = retired.next_version(at(11, 1), &Actor::admin("token"));
    let written = rig
        .roster
        .apply_character(
            CharacterChange::Update {
                version: current.version,
                after: retired.clone(),
            },
            CharacterRecord {
                at: at(11, 1),
                actor: Actor::admin("token"),
                surface: Surface::AdminPortal,
            },
        )
        .await
        .unwrap();
    assert!(matches!(written, CharacterWrite::Updated(_)), "{written:?}");
    rig.service
        .roster_changed(
            "c2",
            Some(&retired),
            &Actor::admin("token"),
            Surface::AdminPortal,
        )
        .await;

    let view = rig.service.view("c2").await.unwrap();
    assert_eq!(
        view.link.unwrap().status,
        LinkStatus::Active,
        "still linked"
    );
    let snapshot = view.snapshot.expect("kept while retired");
    assert_eq!(snapshot.character_name.as_deref(), Some("Heather"));
    assert_eq!(
        rig.service.data("c2").await.unwrap().as_ref(),
        Some(&snapshot)
    );
    assert_eq!(
        rig.service.status("c2").await.unwrap().snapshot.as_ref(),
        Some(&snapshot)
    );
    assert_eq!(rig.service.sweep().await.unlinked, 0, "kept while fresh");

    rig.set_now(snapshot.expires_at);
    assert_eq!(
        rig.service.view("c2").await.unwrap().snapshot,
        None,
        "expired"
    );
    assert_eq!(rig.service.data("c2").await.unwrap(), None);
    assert_eq!(
        rig.service.sweep().await.unlinked,
        1,
        "retired with expired data"
    );
    assert_eq!(rig.link("c2").await, None);
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
    kanade_log::capture();
    let rig = rig().await;
    rig.confirm().await;
    rig.older_notice("c1").await;
    rig.flaky
        .fail_records
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let report = rig.service.sweep().await;
    assert_eq!(report.reconsent, 0);
    assert_eq!(rig.link("c1").await.unwrap().status, LinkStatus::Active);
    let warned = kanade_log::captured()
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
    kanade_log::capture();
    let rig = rig().await;
    rig.confirm().await;
    rig.older_notice("c1").await;
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
    let warned = kanade_log::captured()
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
    kanade_log::capture();
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
    for line in kanade_log::captured() {
        nexon_free(&line.to_string());
    }
    // Unlinked data is never tracked.
    rig.set_now(at(11, 0));
    rig.service
        .unlink(
            "c1",
            &Actor::member("1001"),
            Surface::Discord,
            kanade_domain::chardata::UnlinkReason::Requested,
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

/// The roster row `id` as stored now.
async fn roster_row(rig: &Rig, id: &str) -> kanade_domain::characters::Character {
    CharacterStore::character(&*rig.roster, id)
        .await
        .unwrap()
        .unwrap()
}

/// The History rows of roster writes whose values come from NEXON.
async fn nexon_rows(rig: &Rig) -> Vec<kanade_domain::settings::SettingsChange> {
    rig.roster
        .settings_changes(SettingsChangeQuery {
            actor: Some(Actor::system(NEXON_ACTOR)),
            ..SettingsChangeQuery::default()
        })
        .await
        .unwrap()
}

/// An admin's edit of the roster row `id` at 11:01.
async fn edit(
    rig: &Rig,
    id: &str,
    change: impl FnOnce(&mut kanade_domain::characters::Character),
) -> kanade_domain::characters::Character {
    let current = roster_row(rig, id).await;
    let mut after = current.clone();
    change(&mut after);
    let after = after.next_version(at(11, 1), &Actor::admin("token"));
    let written = rig
        .roster
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
    assert!(matches!(written, CharacterWrite::Updated(_)), "{written:?}");
    after
}

/// NEXON's `basic` answer with `class` and `level`.
fn basic_with(class: Option<&str>, level: Option<u32>) -> String {
    let mut body: serde_json::Value = serde_json::from_str(&basic(IGN)).unwrap();
    body["character_class"] = serde_json::json!(class);
    body["character_level"] = serde_json::json!(level);
    body.to_string()
}

/// User decision 2026-10-13 ("copy and keep"): a stored fetch writes
/// NEXON's class and level into the roster row, with NEXON's spelling when
/// the names differ only in case, in one write and one History row by
/// `system:nexon`; the link stays. A refresh with nothing new writes
/// nothing; a manual edit is overwritten by the next fetch.
#[tokio::test]
async fn a_fetch_copies_class_level_and_ign_case_into_the_roster() {
    let rig = rig().await;
    edit(&rig, "c1", |row| {
        row.ign = "wisteria".into();
        row.class = Some("Cleric".into());
    })
    .await;
    let ConfirmOutcome::Linked(link, fetched) = rig.confirm().await else {
        panic!()
    };
    assert_eq!(fetched, FetchState::Stored);
    let link = Some(*link);
    let row = roster_row(&rig, "c1").await;
    assert_eq!(
        (
            row.ign.as_str(),
            row.class.as_deref(),
            row.level,
            row.version,
            &row.updated_by
        ),
        (
            IGN,
            Some("Bishop"),
            Some(271),
            3,
            &Actor::system(NEXON_ACTOR)
        )
    );
    assert_eq!(rig.link("c1").await, link, "link kept");
    assert_eq!(
        rig.fake.calls(),
        ["id", "basic", "stat"],
        "no call of its own"
    );
    let rows = nexon_rows(&rig).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].section.as_str(), rows[0].surface),
        ("characters", Surface::DeliveryTick)
    );
    assert_eq!(rows[0].values.len(), 1);
    let diff = &rows[0].values["character.c1"];
    for before in [
        r#""ign":"wisteria""#,
        r#""class":"Cleric""#,
        r#""level":250"#,
    ] {
        assert!(diff.from.contains(before), "{diff:?}");
    }
    for after in [
        r#""ign":"Wisteria""#,
        r#""class":"Bishop""#,
        r#""level":271"#,
    ] {
        assert!(diff.to.contains(after), "{diff:?}");
    }

    // Nothing new: a refresh writes nothing.
    assert_eq!(
        rig.service.refresh_now("c1").await.unwrap(),
        RefreshOutcome::Done(FetchState::Stored)
    );
    assert_eq!(nexon_rows(&rig).await.len(), 1);

    // An admin's manual level and case-only edit keep the link; the next
    // fetch puts NEXON's values back.
    let edited = edit(&rig, "c1", |row| {
        row.ign = "WISTERIA".into();
        row.level = Some(200);
    })
    .await;
    rig.service
        .roster_changed(
            "c1",
            Some(&edited),
            &Actor::admin("token"),
            Surface::AdminPortal,
        )
        .await;
    assert_eq!(rig.link("c1").await, link);
    assert_eq!(
        rig.service.refresh_now("c1").await.unwrap(),
        RefreshOutcome::Done(FetchState::Stored)
    );
    let row = roster_row(&rig, "c1").await;
    assert_eq!((row.ign.as_str(), row.level), (IGN, Some(271)));
    assert_eq!(nexon_rows(&rig).await.len(), 2);
    assert_eq!(rig.link("c1").await, link);
}

/// What NEXON did not answer, or what the roster rules refuse, leaves the
/// roster's own value.
#[tokio::test]
async fn values_nexon_lacks_or_the_roster_refuses_are_not_copied() {
    let long = "C".repeat(33);
    for (class, level, expected) in [
        (None, None, (Some("Bishop"), Some(250))),
        (Some(long.as_str()), Some(0), (Some("Bishop"), Some(250))),
        (Some("Cleric"), Some(301), (Some("Cleric"), Some(250))),
        (None, Some(260), (Some("Bishop"), Some(260))),
    ] {
        let rig = rig().await;
        rig.fake.queue("basic", [(200, basic_with(class, level))]);
        assert!(matches!(
            rig.confirm().await,
            ConfirmOutcome::Linked(_, FetchState::Stored)
        ));
        let row = roster_row(&rig, "c1").await;
        assert_eq!(
            (row.class.as_deref(), row.level),
            expected,
            "{class:?} {level:?}"
        );
        let written = usize::from(expected != (Some("Bishop"), Some(250)));
        assert_eq!(nexon_rows(&rig).await.len(), written, "{class:?} {level:?}");
    }
}

/// The copied class and level are roster data: they stay after an unlink,
/// after the NEXON data expires, and after the sweep removes a retired
/// character's expired link.
#[tokio::test]
async fn copied_values_stay_after_unlink_and_expiry() {
    let copied = |row: kanade_domain::characters::Character| (row.class, row.level);
    let expected = (Some("Bishop".to_owned()), Some(271));

    let rig = rig().await;
    rig.confirm().await;
    rig.service
        .unlink(
            "c1",
            &Actor::member("1001"),
            Surface::Discord,
            kanade_domain::chardata::UnlinkReason::Requested,
            None,
        )
        .await
        .unwrap();
    assert_eq!(rig.link("c1").await, None);
    assert_eq!(copied(roster_row(&rig, "c1").await), expected, "unlinked");

    let rig = rig_with(true).await;
    rig.confirm().await;
    rig.set_now(at(11, 0) + TimeDelta::days(31));
    assert_eq!(rig.service.data("c1").await.unwrap(), None, "expired");
    rig.service.sweep().await;
    assert_eq!(copied(roster_row(&rig, "c1").await), expected, "expired");

    let rig = rig_with(true).await;
    rig.fake.queue("basic", [(200, basic("Heather"))]);
    rig.service
        .confirm_link("1001", "c2", NOTICE_VERSION, ConsentSurface::Discord)
        .await
        .unwrap();
    edit(&rig, "c2", |row| row.retired_at = Some(at(11, 1))).await;
    rig.set_now(at(11, 0) + TimeDelta::days(31));
    assert_eq!(rig.service.sweep().await.unlinked, 1);
    assert_eq!(rig.link("c2").await, None);
    assert_eq!(
        copied(roster_row(&rig, "c2").await),
        expected,
        "retired and swept"
    );
}

/// Another name makes the link stale and leaves the roster as it is.
#[tokio::test]
async fn a_different_name_goes_stale_and_copies_nothing() {
    let rig = rig().await;
    rename(&rig, "c1", "wisteria").await;
    rig.fake.queue("basic", [(200, basic("Wisterias"))]);
    let ConfirmOutcome::Linked(_, fetched) = rig.confirm().await else {
        panic!()
    };
    assert_eq!(fetched, FetchState::Stale);
    let row = roster_row(&rig, "c1").await;
    assert_eq!(
        (row.ign.as_str(), row.level, row.version),
        ("wisteria", Some(250), 2)
    );
    assert!(nexon_rows(&rig).await.is_empty());
    assert_eq!(rig.service.sync_rosters().await, 0);
}

/// A roster write between the fetch's read and the copy (another version)
/// wins: nothing is written.
#[tokio::test]
async fn a_roster_edit_since_the_fetch_read_skips_the_copy() {
    let rig = rig().await;
    rig.confirm().await;
    let link = rig.link("c1").await.unwrap();
    let snapshot = rig.service.data("c1").await.unwrap().unwrap();
    let seen = edit(&rig, "c1", |row| row.level = Some(200)).await;
    edit(&rig, "c1", |row| row.class = Some("Cleric".into())).await;
    let after = super::roster_sync::synced(&link, &seen, &snapshot).expect("a level to copy");
    assert!(!rig.service.sync_roster(&seen, after).await);
    let row = roster_row(&rig, "c1").await;
    assert_eq!(
        (row.class.as_deref(), row.level, row.version),
        (Some("Cleric"), Some(200), 4)
    );
    assert_eq!(nexon_rows(&rig).await.len(), 1, "only the confirm's copy");
}

/// Startup: an active link whose fresh snapshot holds what the roster row
/// lacks is copied once, without a Nexon call; a second pass does nothing.
#[tokio::test]
async fn the_startup_pass_copies_stored_data_once() {
    let rig = rig().await;
    rig.confirm().await;
    let link = rig.link("c1").await.unwrap();
    edit(&rig, "c1", |row| {
        row.ign = "WISTERIA".into();
        row.level = Some(200);
        row.class = None;
    })
    .await;
    let calls = rig.fake.calls().len();
    assert_eq!(rig.service.sync_rosters().await, 1);
    let row = roster_row(&rig, "c1").await;
    assert_eq!(
        (row.ign.as_str(), row.class.as_deref(), row.level),
        (IGN, Some("Bishop"), Some(271))
    );
    assert_eq!(nexon_rows(&rig).await.len(), 2);
    assert_eq!(rig.link("c1").await, Some(link));
    assert_eq!(rig.service.sync_rosters().await, 0, "idempotent");
    assert_eq!(nexon_rows(&rig).await.len(), 2);
    assert_eq!(rig.fake.calls().len(), calls, "no Nexon call");
}

/// Expired data, stale and pending links (an older notice included) are
/// never copied.
#[tokio::test]
async fn the_startup_pass_leaves_expired_stale_and_pending_links_alone() {
    for case in ["expired", "stale", "pending", "older_notice"] {
        let rig = rig().await;
        rig.confirm().await;
        let link = rig.link("c1").await.unwrap();
        let version = link.version;
        match case {
            "expired" => rig.set_now(at(11, 0) + TimeDelta::days(31)),
            "stale" => {
                rig.file
                    .put_link(link.with_status(LinkStatus::Stale), version)
                    .await
                    .unwrap();
            }
            "pending" => {
                let pending = Link {
                    consent_version: None,
                    consented_at: None,
                    linked_at: None,
                    consent_surface: None,
                    ..link.with_status(LinkStatus::Pending)
                };
                rig.file.put_link(pending, version).await.unwrap();
            }
            _ => rig.older_notice("c1").await,
        }
        edit(&rig, "c1", |row| {
            row.ign = "WISTERIA".into();
            row.level = Some(200);
        })
        .await;
        assert_eq!(rig.service.sync_rosters().await, 0, "{case}");
        let row = roster_row(&rig, "c1").await;
        assert_eq!(
            (row.ign.as_str(), row.level),
            ("WISTERIA", Some(200)),
            "{case}"
        );
        assert_eq!(
            nexon_rows(&rig).await.len(),
            1,
            "{case}: the confirm's copy only"
        );
    }
}

/// A pending link on file for `id`, requested by `by` at 10:00.
async fn put_pending(rig: &Rig, id: &str, by: Actor) {
    let character = roster_row(rig, id).await;
    let pending = Link {
        character_id: id.into(),
        member_id: character.member_id.clone(),
        ign_key: character.ign_key(),
        region: "msea".into(),
        status: LinkStatus::Pending,
        requested_by: by,
        requested_at: at(10, 0),
        consent_version: None,
        consented_at: None,
        linked_at: None,
        consent_surface: None,
        version: 1,
    };
    assert_eq!(
        rig.file.put_link(pending, 0).await.unwrap(),
        kanade_domain::chardata::LinkPut::Written
    );
}

/// Startup (user decision 2026-10-13): admin requests left pending become
/// active once, by the sweeper, and fetch (copying into the roster); a
/// member's own link waiting for a new notice is left alone, and a second
/// pass does nothing.
#[tokio::test]
async fn the_startup_pass_activates_admin_requests_once() {
    let rig = rig().await;
    let admin = Actor::admin("discord:2001");
    put_pending(&rig, "c1", admin.clone()).await;
    put_pending(&rig, "c2", Actor::system(SWEEPER)).await;
    assert_eq!(rig.service.activate_requests().await, 1);
    let link = rig.link("c1").await.unwrap();
    assert_eq!(link.status, LinkStatus::Active);
    assert_eq!(
        (&link.requested_by, link.requested_at),
        (&admin, at(10, 0)),
        "still the admin's request"
    );
    assert_eq!(
        (link.consent_version, link.linked_at, link.consent_surface),
        (Some(NOTICE_VERSION), Some(at(11, 0)), None)
    );
    assert!(link.admin_linked());
    assert_eq!(rig.fake.calls(), ["id", "basic", "stat"]);
    assert!(rig.service.data("c1").await.unwrap().is_some());
    assert_eq!(roster_row(&rig, "c1").await.level, Some(271));
    assert_eq!(
        rig.link("c2").await.unwrap().status,
        LinkStatus::Pending,
        "waits for its member"
    );
    assert_eq!(
        rig.history().await,
        [(
            "link.c1".into(),
            "pending".into(),
            "linked".into(),
            "system:chardata".into()
        )]
    );

    assert_eq!(rig.service.activate_requests().await, 0, "idempotent");
    assert_eq!(rig.fake.calls().len(), 3);
    assert_eq!(rig.history().await.len(), 1);

    // Without a key nothing is activated; a retired character's request
    // waits for the sweep.
    let rig = rig_with(false).await;
    put_pending(&rig, "c1", admin.clone()).await;
    assert_eq!(rig.service.activate_requests().await, 0);
    assert_eq!(rig.link("c1").await.unwrap().status, LinkStatus::Pending);
    let rig = rig_with(true).await;
    put_pending(&rig, "c2", admin).await;
    edit(&rig, "c2", |row| row.retired_at = Some(at(11, 1))).await;
    assert_eq!(rig.service.activate_requests().await, 0);
    assert_eq!(rig.link("c2").await.unwrap().status, LinkStatus::Pending);
    assert!(rig.fake.calls().is_empty());
}

/// User decision 2026-10-15: a notice bump re-asks only member-consented
/// links; an admin's link made under the older notice stays active with
/// its data.
#[tokio::test]
async fn a_notice_bump_re_asks_members_never_an_admins_link() {
    let rig = rig().await;
    let admin = Actor::admin("token");
    let linked = rig
        .service
        .admin_link("c1", 0, &admin, Surface::AdminPortal)
        .await
        .unwrap();
    assert!(matches!(linked, AdminLinkOutcome::Linked(..)));
    rig.service
        .confirm_link("1001", "c2", NOTICE_VERSION, ConsentSurface::Discord)
        .await
        .unwrap();
    rig.older_notice("c1").await;
    rig.older_notice("c2").await;
    let view = rig.service.view("c1").await.unwrap();
    assert_eq!(view.link.unwrap().status, LinkStatus::Active, "admin's");
    assert!(view.snapshot.is_some(), "its data still served");
    assert_eq!(
        rig.service.view("c2").await.unwrap().link.unwrap().status,
        LinkStatus::Pending,
        "the member's reads as pending at once"
    );
    assert_eq!(rig.service.sweep().await.reconsent, 1);
    assert_eq!(rig.link("c1").await.unwrap().status, LinkStatus::Active);
    assert_eq!(rig.link("c2").await.unwrap().status, LinkStatus::Pending);
    assert_eq!(
        rig.service.refresh_now("c1").await.unwrap(),
        RefreshOutcome::Done(FetchState::Stored),
        "still refreshed"
    );
}

/// An invented scouter result for `ign`.
fn scouted(ign: &str) -> StoredMember {
    StoredMember {
        ign: ign.into(),
        preset: "00010".into(),
        critical_rate_assumed: false,
        critical_rate_actual: None,
        source: None,
        bosses: BTreeMap::new(),
    }
}

/// The roster IGN `name`, as the scouter is sent it.
fn scouter_ign(name: &str) -> RosterIgn {
    RosterIgn::of(&kanade_domain::characters::Character {
        id: "c0".into(),
        member_id: "1001".into(),
        ign: name.into(),
        world: "msea".into(),
        class: None,
        level: None,
        role: CharacterRole::Main,
        retired_at: None,
        created_at: at(1, 0),
        updated_at: at(1, 0),
        updated_by: Actor::admin("token"),
        version: 1,
    })
    .unwrap()
}

/// `rig`'s file with results for c1 (Wisteria) and c2 (Heather) and their
/// party total, all fetched at `fetched_at`, and a service whose estimates
/// loaded them (nothing listens at the scouter).
async fn estimating(rig: &Rig, fetched_at: DateTime<Utc>) -> Arc<CharacterData> {
    for (id, ign) in [("c1", IGN), ("c2", "Heather")] {
        let estimate = MemberEstimate {
            character_id: id.into(),
            ign_key: ign.to_ascii_lowercase(),
            result: scouted(ign),
            fetched_at,
        };
        rig.file.put_member_estimate(estimate).await.unwrap();
    }
    let party = PartyEstimate {
        set_key: "heather,wisteria".into(),
        character_ids: vec!["c1".into(), "c2".into()],
        combined: BTreeMap::new(),
        fetched_at,
    };
    rig.file.put_party_estimate(party).await.unwrap();
    let clock: Clock = {
        let now = Arc::clone(&rig.now);
        Arc::new(move || *now.lock().unwrap())
    };
    let url = "http://127.0.0.1:9".to_owned();
    let estimates = Estimates::new(ScouterClient::new(&url).unwrap(), url, Arc::clone(&clock))
        .with_store(Arc::clone(&rig.file) as Arc<dyn CharDataStore>);
    estimates.load().await;
    let service = CharacterData::new(
        Arc::clone(&rig.file) as Arc<dyn CharDataStore>,
        Arc::clone(&rig.flaky) as Arc<dyn RosterSide>,
        None,
        clock,
        Timing::default(),
    );
    Arc::new(service.with_estimates(estimates))
}

/// Whether character `id`'s result is in the file (read as of 11:00), and
/// whether memory holds one under `ign`.
async fn held(service: &CharacterData, id: &str, ign: &str) -> (bool, bool) {
    let stored = service
        .store()
        .estimates(at(11, 0), u32::MAX)
        .await
        .unwrap();
    let memory = service.estimates().unwrap();
    (
        stored.members.iter().any(|row| row.character_id == id),
        memory.cached(&scouter_ign(ign)).is_some(),
    )
}

/// Whether c1 and c2's party total is in the file and in memory.
async fn party_held(service: &CharacterData) -> (bool, bool) {
    let stored = service
        .store()
        .estimates(at(11, 0), u32::MAX)
        .await
        .unwrap();
    let memory = service.estimates().unwrap();
    (
        !stored.parties.is_empty(),
        memory
            .cached_party(&[scouter_ign(IGN), scouter_ign("Heather")])
            .is_some(),
    )
}

/// A deleted roster character's estimates, and every party total naming
/// them, leave the file and memory at the next sweep; others stay.
#[tokio::test]
async fn the_sweep_drops_a_deleted_characters_estimates() {
    let rig = rig().await;
    let service = estimating(&rig, at(11, 0)).await;
    assert_eq!(held(&service, "c2", "Heather").await, (true, true));
    assert_eq!(party_held(&service).await, (true, true));
    let written = rig
        .roster
        .apply_character(
            CharacterChange::Delete {
                id: "c2".into(),
                version: 1,
            },
            CharacterRecord {
                at: at(11, 1),
                actor: Actor::admin("token"),
                surface: Surface::AdminPortal,
            },
        )
        .await
        .unwrap();
    assert!(matches!(written, CharacterWrite::Deleted(_)), "{written:?}");
    assert_eq!(service.sweep().await.estimates_dropped, 1);
    assert_eq!(held(&service, "c2", "Heather").await, (false, false));
    assert_eq!(
        party_held(&service).await,
        (false, false),
        "with the totals"
    );
    assert_eq!(held(&service, "c1", IGN).await, (true, true), "others stay");
}

#[tokio::test]
async fn the_sweep_drops_a_retired_characters_estimates() {
    let rig = rig().await;
    let service = estimating(&rig, at(11, 0)).await;
    edit(&rig, "c2", |row| row.retired_at = Some(at(11, 1))).await;
    assert_eq!(service.sweep().await.estimates_dropped, 1);
    assert_eq!(held(&service, "c2", "Heather").await, (false, false));
    assert_eq!(party_held(&service).await, (false, false));
    assert_eq!(held(&service, "c1", IGN).await, (true, true));
}

/// Off MapleSEA, the scouter's IGN is another character's.
#[tokio::test]
async fn the_sweep_drops_estimates_when_the_world_changes() {
    let rig = rig().await;
    let service = estimating(&rig, at(11, 0)).await;
    edit(&rig, "c2", |row| row.world = "kms".into()).await;
    assert_eq!(service.sweep().await.estimates_dropped, 1);
    assert_eq!(held(&service, "c2", "Heather").await, (false, false));
    assert_eq!(party_held(&service).await, (false, false));
    assert_eq!(held(&service, "c1", IGN).await, (true, true));
}

/// A renamed character's results were read under the old IGN.
#[tokio::test]
async fn the_sweep_drops_estimates_when_the_ign_changes() {
    let rig = rig().await;
    let service = estimating(&rig, at(11, 0)).await;
    edit(&rig, "c2", |row| row.ign = "HEATHER".into()).await;
    assert_eq!(
        service.sweep().await.estimates_dropped,
        0,
        "letter case alone is the same IGN"
    );
    rename(&rig, "c2", "Lavender").await;
    assert_eq!(service.sweep().await.estimates_dropped, 1);
    assert_eq!(held(&service, "c2", "Heather").await, (false, false));
    assert_eq!(party_held(&service).await, (false, false));
    assert_eq!(held(&service, "c1", IGN).await, (true, true));
}

#[tokio::test]
async fn the_sweep_drops_estimates_when_the_member_leaves() {
    let rig = rig().await;
    let service = estimating(&rig, at(11, 0)).await;
    assert!(rig.roster.member_departed("1001").await.unwrap());
    assert_eq!(service.sweep().await.estimates_dropped, 2);
    assert_eq!(held(&service, "c1", IGN).await, (false, false));
    assert_eq!(held(&service, "c2", "Heather").await, (false, false));
    assert_eq!(party_held(&service).await, (false, false));
}

/// Nothing is kept past 30 days, in the file or in memory.
#[tokio::test]
async fn the_sweep_drops_estimates_after_30_days() {
    let rig = rig().await;
    let fetched_at = at(11, 0) - TimeDelta::hours(1);
    let service = estimating(&rig, fetched_at).await;
    rig.set_now(fetched_at + TimeDelta::days(30) - TimeDelta::seconds(1));
    assert_eq!(service.sweep().await.estimates_expired, Some(0));
    assert_eq!(held(&service, "c1", IGN).await, (true, true));
    rig.set_now(fetched_at + TimeDelta::days(30));
    let report = service.sweep().await;
    assert_eq!(
        (report.estimates_expired, report.estimates_dropped),
        (Some(3), 0)
    );
    assert_eq!(held(&service, "c1", IGN).await, (false, false));
    assert_eq!(held(&service, "c2", "Heather").await, (false, false));
    assert_eq!(party_held(&service).await, (false, false));
}

/// A wall clock stepped back leaves results stamped in the future: they
/// go, so a stalled clock never keeps them.
#[tokio::test]
async fn the_sweep_drops_future_stamped_estimates() {
    let rig = rig().await;
    let service = estimating(&rig, at(11, 0)).await;
    rig.set_now(at(9, 0));
    assert_eq!(service.sweep().await.estimates_expired, Some(3));
    assert_eq!(held(&service, "c1", IGN).await, (false, false));
    assert_eq!(held(&service, "c2", "Heather").await, (false, false));
    assert_eq!(party_held(&service).await, (false, false));
}

/// Estimates come from the roster IGN, not NEXON data: an unlink keeps
/// them.
#[tokio::test]
async fn a_nexon_unlink_keeps_estimates() {
    let rig = rig().await;
    rig.confirm().await;
    let service = estimating(&rig, at(11, 0)).await;
    let removed = service
        .unlink(
            "c1",
            &Actor::admin("token"),
            Surface::AdminPortal,
            kanade_domain::chardata::UnlinkReason::Requested,
            None,
        )
        .await
        .unwrap();
    assert!(removed.is_some());
    let report = service.sweep().await;
    assert_eq!(
        (report.estimates_expired, report.estimates_dropped),
        (Some(0), 0)
    );
    assert_eq!(held(&service, "c1", IGN).await, (true, true));
    assert_eq!(party_held(&service).await, (true, true));
}
