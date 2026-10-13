//! The member's own characters on the public origin (module `roster`, and
//! `character_data` over a fake Open API and a fake MapleScouter on
//! loopback, all invented): own rows with the admin rules, the NEXON notice,
//! Link / Cancel / Unlink / My data, the lookup preview, the member's own
//! MapleScouter lookups and the party's characters on their runs; module
//! gates, other-member isolation, fresh sign-in, CSRF and rate limits.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use axum::{
    Router,
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::TimeDelta;
use kanade::{
    api::auth::Clock,
    bot::chardata::copy::notice_text,
    domain::{
        chardata::{ConsentSurface, NOTICE_VERSION},
        history::{Actor, Surface},
        settings::{SettingsChangeQuery, SettingsStore},
    },
    infrastructure::{
        character_data::{CharacterData, ConfirmOutcome, RosterSide, Timing},
        nexon::{ApiKey, NexonClient},
        scouter::{Estimates, JobState, ScouterClient},
        store::MemoryCharData,
    },
};
use serde_json::{Value, json};

use super::{ALICE, BOB, CARA, Portal, refused};
use crate::{
    auth::member_support::{Browser, PUB_ORIGIN},
    reads::{CharacterDataMaker, Reads},
    schemas::assert_valid,
    support::{ADMIN_HOST, Reply, send},
};

const MINE: &str = "/api/public/me/characters";
const LOOKUP: &str = "/api/public/me/characters/lookup";
const LINKS: &str = "/api/public/me/characters/links";
const DATA: &str = "/api/public/me/data";
const ESTIMATES: &str = "/api/public/me/estimates";
const PARTY: &str = "/api/public/runs/r-kalos/characters";

const MY_CHARACTERS: &str = "public.json#/$defs/MyCharacters";
const CREATED: &str = "public.json#/$defs/MyCharacterCreated";
const ROW: &str = "characters.json#/$defs/CharacterRow";
const NOTICE: &str = "public.json#/$defs/LinkNotice";
const OUTCOME: &str = "public.json#/$defs/LinkOutcome";
const CANCELLED: &str = "public.json#/$defs/LinkCancelled";
const UNLINKED: &str = "public.json#/$defs/Unlinked";
const UNLINKED_ALL: &str = "public.json#/$defs/UnlinkedAll";
const MY_DATA: &str = "public.json#/$defs/MyData";
const LINK: &str = "character_data.json#/$defs/CharacterLink";
const PREVIEW: &str = "character_data.json#/$defs/CharacterLookup";
const STARTED: &str = "character_data.json#/$defs/EstimateStarted";
const STATUS: &str = "character_data.json#/$defs/EstimateStatus";
const JOB: &str = "character_data.json#/$defs/EstimateJob";
const LIMITED: &str = "public.json#/$defs/MemberEstimateLimit";
const PARTY_VIEW: &str = "public.json#/$defs/RunPartyCharacters";

fn row(id: &str) -> String {
    format!("{MINE}/{id}")
}

fn link(id: &str) -> String {
    format!("{MINE}/{id}/link")
}

fn notice(id: &str) -> String {
    format!("{MINE}/{id}/link/notice")
}

fn cancel(id: &str) -> String {
    format!("{MINE}/{id}/link/cancel")
}

fn id(value: &Value) -> String {
    value["id"].as_str().unwrap().to_owned()
}

/// A 2xx answer with `status`, valid against `target`.
fn answer(reply: &Reply, status: u16, target: &str) -> Value {
    assert_eq!(reply.status, status, "{}", reply.text());
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    let value = reply.json();
    assert_valid(target, target, &value);
    value
}

/// Every member character route, with a body for the writes.
fn every_route(character: &str) -> Vec<(&'static str, String, Option<Value>)> {
    vec![
        ("GET", MINE.to_owned(), None),
        ("POST", MINE.to_owned(), Some(json!({"ign": "AsahiKanna"}))),
        ("PATCH", row(character), Some(json!({"version": 1}))),
        (
            "POST",
            LOOKUP.to_owned(),
            Some(json!({"ign": "AsahiKanna"})),
        ),
        ("GET", link(character), None),
        ("POST", link(character), Some(json!({"notice": 1}))),
        ("DELETE", link(character), None),
        ("GET", notice(character), None),
        ("POST", cancel(character), None),
        ("DELETE", LINKS.to_owned(), None),
        ("GET", DATA.to_owned(), None),
        ("GET", format!("{ESTIMATES}?character_id={character}"), None),
        (
            "POST",
            ESTIMATES.to_owned(),
            Some(json!({"character_id": character})),
        ),
        ("GET", format!("{ESTIMATES}/j-1"), None),
        ("GET", PARTY.to_owned(), None),
    ]
}

/// A fake Open API that knows every name (echoing it back as NEXON's, Lv.
/// 271, CP 27,182,818) except `Nobody…`; invented data only.
async fn fake_nexon() -> SocketAddr {
    let app = Router::new().fallback(|request: Request| async move {
        let path = request.uri().path().to_owned();
        let query = request.uri().query().unwrap_or_default().to_owned();
        let value = query
            .split_once('=')
            .map(|(_, value)| value.to_owned())
            .unwrap_or_default();
        let reply: Response = if path.ends_with("/v1/id") && value.starts_with("Nobody") {
            (
                StatusCode::BAD_REQUEST,
                r#"{"error":{"name":"OPENAPI00004","message":"invented"}}"#,
            )
                .into_response()
        } else if path.ends_with("/v1/id") {
            format!(r#"{{"ocid":"ocid-invented-{value}"}}"#).into_response()
        } else if path.ends_with("/character/basic") {
            let name = value.trim_start_matches("ocid-invented-");
            format!(
                r#"{{"date":"2026-09-28T00:00+08:00","character_name":"{name}","world_name":"Aquila","character_class":"Bishop","character_level":271,"character_image":"https://open.api.nexon.com/static/invented"}}"#
            )
            .into_response()
        } else {
            r#"{"final_stat":[{"stat_name":"Combat Power","stat_value":"27182818"}]}"#
                .into_response()
        };
        reply
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    address
}

/// The module, its clock moved by `skew` (the lookup spacing), with the
/// Open API at `nexon` and MapleScouter at `scouter` when given.
fn module(
    nexon: Option<SocketAddr>,
    scouter: Option<SocketAddr>,
    file: Arc<MemoryCharData>,
    slot: Arc<OnceLock<Arc<CharacterData>>>,
    skew: Arc<Mutex<TimeDelta>>,
) -> CharacterDataMaker {
    Box::new(move |store, clock| {
        let clock: Clock = Arc::new(move || clock() + *skew.lock().unwrap());
        let mut service = CharacterData::new(
            file,
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
                .with_timeout(Duration::from_millis(400));
            service = service.with_estimates(Estimates::new(client, url, clock));
        }
        let service = Arc::new(service);
        slot.set(Arc::clone(&service)).ok().unwrap();
        service
    })
}

/// The portal over both modules: NEXON connected when `nexon`, MapleScouter
/// on when `scouter`.
struct Chars {
    portal: Portal,
    service: Arc<CharacterData>,
    file: Arc<MemoryCharData>,
    skew: Arc<Mutex<TimeDelta>>,
    scouter: Option<crate::estimates::Fake>,
}

impl Chars {
    async fn new(nexon: bool, scouter: bool) -> Self {
        let nexon = if nexon {
            Some(fake_nexon().await)
        } else {
            None
        };
        let (address, fake) = if scouter {
            let (address, fake) = crate::estimates::fake_scouter().await;
            (Some(address), Some(fake))
        } else {
            (None, None)
        };
        let slot = Arc::new(OnceLock::new());
        let file = Arc::new(MemoryCharData::new());
        let skew = Arc::new(Mutex::new(TimeDelta::zero()));
        let reads = Reads::with_character_data(
            None,
            module(
                nexon,
                address,
                Arc::clone(&file),
                Arc::clone(&slot),
                Arc::clone(&skew),
            ),
        )
        .await;
        Self {
            portal: Portal::over(reads).await,
            service: slot.get().unwrap().clone(),
            file,
            skew,
            scouter: fake,
        }
    }

    fn reads(&self) -> &Reads {
        &self.portal.reads
    }

    /// An admin adds a character for `member` (a pending link request for
    /// a MapleSEA one while NEXON is connected): its id.
    async fn admin_add(&self, member: u64, body: Value) -> String {
        let reply = self
            .reads()
            .call(
                "POST",
                &format!("/api/admin/members/{member}/characters"),
                body,
                &[],
            )
            .await;
        assert_eq!(reply.status, 201, "{}", reply.text());
        id(&reply.json())
    }

    async fn add(&self, browser: &Browser, key: &str, body: Value) -> Reply {
        self.portal
            .write(browser, "POST", MINE, key, Some(&body))
            .await
    }

    async fn added(&self, browser: &Browser, key: &str, body: Value) -> Value {
        answer(&self.add(browser, key, body).await, 201, CREATED)
    }

    async fn patch(&self, browser: &Browser, id: &str, key: &str, body: Value) -> Reply {
        self.portal
            .write(browser, "PATCH", &row(id), key, Some(&body))
            .await
    }

    async fn link(&self, browser: &Browser, id: &str, key: &str, notice: u32) -> Reply {
        self.portal
            .write(
                browser,
                "POST",
                &link(id),
                key,
                Some(&json!({ "notice": notice })),
            )
            .await
    }

    async fn unlink(&self, browser: &Browser, id: &str, key: &str) -> Reply {
        self.portal
            .write(browser, "DELETE", &link(id), key, None)
            .await
    }

    /// A read without the markers of a write (lookups, cancel, estimates).
    async fn post(&self, browser: &Browser, path: &str, body: Option<&Value>) -> Reply {
        let body = body.map(Value::to_string);
        self.portal
            .send(
                Some(browser),
                "POST",
                path,
                &[PUB_ORIGIN, ("X-Kanade-CSRF", &browser.csrf)],
                body.as_deref(),
            )
            .await
    }

    async fn look_up(&self, browser: &Browser, ign: &str) -> Reply {
        *self.skew.lock().unwrap() += TimeDelta::seconds(4);
        self.post(browser, LOOKUP, Some(&json!({ "ign": ign })))
            .await
    }

    async fn start(&self, browser: &Browser, body: Value) -> Reply {
        self.post(browser, ESTIMATES, Some(&body)).await
    }

    /// Job `job` once it has finished, read from the queue itself (polling
    /// the route would spend the member's read tokens).
    async fn settled(&self, job: &str) -> JobState {
        let estimates = self.service.estimates().unwrap();
        for _ in 0..400 {
            let state = estimates.job(job).unwrap().state();
            if matches!(state, JobState::Done | JobState::Failed) {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{job} still queued or running");
    }
}

#[tokio::test]
async fn every_route_is_the_generic_404_while_its_module_is_off() {
    let generic = |reply: &Reply, what: &str| {
        assert_eq!(refused(reply), (404, "not_found".into()), "{what}");
        assert_eq!(
            reply.json()["message"],
            "No such endpoint on this origin.",
            "{what}"
        );
    };
    // Neither module: every route, signed in or not.
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let markers = [
        PUB_ORIGIN,
        ("X-Kanade-CSRF", alice.csrf.as_str()),
        ("Idempotency-Key", "k-1"),
    ];
    for (method, path, body) in every_route("c-1") {
        let body = body.map(|body| body.to_string());
        for browser in [None, Some(&alice)] {
            let reply = portal
                .send(browser, method, &path, &markers, body.as_deref())
                .await;
            generic(&reply, &format!("{method} {path}"));
        }
    }

    // The roster alone: its routes answer (401 signed out), the NEXON and
    // estimate routes stay the generic 404 before the session.
    let portal = Portal::over(Reads::with_characters().await).await;
    let alice = portal.sign_in(ALICE).await;
    let markers = [
        PUB_ORIGIN,
        ("X-Kanade-CSRF", alice.csrf.as_str()),
        ("Idempotency-Key", "k-2"),
    ];
    let roster = [
        ("GET", MINE.to_owned()),
        ("POST", MINE.to_owned()),
        ("PATCH", row("c-1")),
        ("GET", PARTY.to_owned()),
    ];
    for (method, path, body) in every_route("c-1") {
        let body = body.map(|body| body.to_string());
        let anonymous = portal
            .send(None, method, &path, &markers, body.as_deref())
            .await;
        if roster.contains(&(method, path.clone())) {
            assert_eq!(
                refused(&anonymous),
                (401, "unauthenticated".into()),
                "{method} {path}"
            );
        } else {
            generic(&anonymous, &format!("{method} {path}"));
            let signed_in = portal
                .send(Some(&alice), method, &path, &markers, body.as_deref())
                .await;
            generic(&signed_in, &format!("{method} {path}"));
        }
    }
    let mine = portal.read(&alice, MINE, MY_CHARACTERS).await;
    assert_eq!(
        (
            &mine["character_data"],
            &mine["connected"],
            &mine["links"],
            &mine["pending"]
        ),
        (&json!(false), &json!(false), &json!({}), &json!([]))
    );
    // Closed, the portal answers `closed` first.
    portal.harness.set_open(false);
    let closed = portal.get(Some(&alice), MINE).await;
    assert_eq!(refused(&closed), (503, "closed".into()));

    // Character data without MapleScouter: only the estimate routes stay 404.
    let chars = Chars::new(true, false).await;
    let alice = chars.portal.sign_in(ALICE).await;
    for path in [
        format!("{ESTIMATES}?character_id=c-1"),
        format!("{ESTIMATES}/j-1"),
    ] {
        generic(&chars.portal.get(Some(&alice), &path).await, &path);
    }
    let start = chars.start(&alice, json!({"character_id": "c-1"})).await;
    generic(&start, "POST estimates");
    let mine = chars.portal.read(&alice, MINE, MY_CHARACTERS).await;
    assert_eq!(
        (
            &mine["character_data"],
            &mine["connected"],
            &mine["estimates"]
        ),
        (&json!(true), &json!(true), &json!(false))
    );
}

#[tokio::test]
async fn admin_routes_stay_off_the_public_origin_and_member_routes_off_the_admin() {
    let chars = Chars::new(true, true).await;
    let alice = chars.portal.sign_in(ALICE).await;
    let bob = chars
        .admin_add(
            BOB,
            json!({"ign": "BobKnight", "class": "Hero", "level": 260}),
        )
        .await;
    for (method, path) in [
        ("GET", "/api/admin/members/1002/characters".to_owned()),
        ("POST", "/api/admin/members/1001/characters".to_owned()),
        ("PATCH", format!("/api/admin/characters/{bob}")),
        ("DELETE", format!("/api/admin/characters/{bob}")),
        ("GET", format!("/api/admin/characters/{bob}/link")),
        ("POST", "/api/admin/characters/lookup".to_owned()),
        ("POST", "/api/admin/estimates".to_owned()),
        ("GET", "/api/admin/runs/r-kalos/characters".to_owned()),
        ("GET", "/api/admin/runs/r-kalos/readiness".to_owned()),
    ] {
        let reply = chars
            .portal
            .send(
                Some(&alice),
                method,
                &path,
                &[PUB_ORIGIN, ("X-Kanade-CSRF", &alice.csrf)],
                Some("{}"),
            )
            .await;
        assert_eq!(
            refused(&reply),
            (404, "not_found".into()),
            "{method} {path}"
        );
    }
    let reads = chars.reads();
    for path in [MINE, DATA, PARTY] {
        let reply = send(
            reads.admin,
            "GET",
            ADMIN_HOST,
            path,
            &[("Cookie", reads.cookie.as_str())],
            None,
        )
        .await;
        assert_eq!(reply.status, 404, "{path}: {}", reply.text());
    }
}

#[tokio::test]
async fn members_add_edit_and_retire_only_their_own_characters() {
    let chars = Chars::new(true, true).await;
    let alice = chars.portal.sign_in(ALICE).await;
    // MapleSEA with NEXON connected: no class or level, and no link asked.
    let kanna = chars
        .added(&alice, "a-1", json!({"ign": "AsahiKanna"}))
        .await;
    assert_eq!(
        (&kanna["member_id"], &kanna["role"], &kanna["linkable"]),
        (&json!("1001"), &json!("main"), &json!(true))
    );
    let replayed = chars.add(&alice, "a-1", json!({"ign": "AsahiKanna"})).await;
    assert_eq!(answer(&replayed, 201, CREATED), kanna);
    let other = chars.add(&alice, "a-1", json!({"ign": "AsahiPath"})).await;
    assert_eq!(refused(&other), (422, "idempotency_mismatch".into()));
    // Another region (or no NEXON) needs the class and level by hand.
    let manual = chars
        .add(&alice, "a-2", json!({"ign": "AsahiMech", "world": "gms"}))
        .await;
    assert_eq!(refused(&manual), (422, "class_level_required".into()));
    let mech = chars
        .added(
            &alice,
            "a-3",
            json!({"ign": "AsahiMech", "world": "gms", "class": "Mechanic", "level": 250}),
        )
        .await;
    assert_eq!(
        (&mech["role"], &mech["linkable"]),
        (&json!("alt"), &json!(false))
    );
    let taken = chars.add(&alice, "a-4", json!({"ign": "asahikanna"})).await;
    assert_eq!(refused(&taken), (422, "ign_taken".into()));
    let mine = chars.portal.read(&alice, MINE, MY_CHARACTERS).await;
    assert_eq!(mine["characters"].as_array().unwrap().len(), 2);
    assert_eq!(mine["pending"], json!([]));
    assert_eq!(mine["links"][id(&kanna)]["state"], "not_linked");
    assert_eq!(
        chars.service.status(&id(&kanna)).await.unwrap().link,
        None,
        "Add never asks for a link"
    );

    // The admin rules: the main stays while an alt is active, Make main
    // moves the crown, a stale version is refused.
    let locked = chars
        .patch(
            &alice,
            &id(&kanna),
            "e-1",
            json!({"version": 1, "retired": true}),
        )
        .await;
    assert_eq!(refused(&locked), (409, "main_required".into()));
    let crowned = answer(
        &chars
            .patch(
                &alice,
                &id(&mech),
                "e-2",
                json!({"version": 1, "role": "main"}),
            )
            .await,
        200,
        ROW,
    );
    assert_eq!(crowned["role"], "main");
    let stale = chars
        .patch(
            &alice,
            &id(&kanna),
            "e-3",
            json!({"version": 1, "retired": true}),
        )
        .await;
    assert_eq!(refused(&stale), (409, "stale".into()));
    let retired = answer(
        &chars
            .patch(
                &alice,
                &id(&kanna),
                "e-4",
                json!({"version": 2, "retired": true}),
            )
            .await,
        200,
        ROW,
    );
    assert!(retired["retired_at"].is_string());
    // No member delete.
    let delete = chars
        .portal
        .write(&alice, "DELETE", &row(&id(&kanna)), "d-1", None)
        .await;
    assert_eq!(refused(&delete), (404, "not_found".into()));

    // History names the member, through the portal.
    let changes = chars
        .reads()
        .store
        .settings_changes(SettingsChangeQuery {
            actor: Some(Actor::member("1001")),
            ..SettingsChangeQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(changes.len(), 4, "{changes:?}");
    assert!(
        changes
            .iter()
            .all(|change| change.surface == Surface::PublicPortal && change.section == "characters")
    );

    // Bob's characters: not listed, not readable, not writable, the same
    // 404 as an id nobody has.
    let bob = chars
        .admin_add(
            BOB,
            json!({"ign": "BobKnight", "class": "Hero", "level": 260}),
        )
        .await;
    let mine = chars.portal.read(&alice, MINE, MY_CHARACTERS).await;
    let ids: Vec<&str> = mine["characters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap())
        .collect();
    assert!(!ids.contains(&bob.as_str()));
    assert_eq!(mine["pending"], json!([]), "Bob's request is his");
    for character in [bob.as_str(), "nobody"] {
        for (method, path, body) in [
            (
                "PATCH",
                row(character),
                Some(json!({"version": 1, "retired": true})),
            ),
            ("GET", link(character), None),
            ("POST", link(character), Some(json!({"notice": 1}))),
            ("DELETE", link(character), None),
            ("GET", notice(character), None),
            ("POST", cancel(character), None),
            ("GET", format!("{ESTIMATES}?character_id={character}"), None),
            (
                "POST",
                ESTIMATES.to_owned(),
                Some(json!({"character_id": character})),
            ),
        ] {
            let reply = chars
                .portal
                .write(&alice, method, &path, &format!("x-{method}"), body.as_ref())
                .await;
            assert_eq!(
                refused(&reply),
                (404, "not_found".into()),
                "{method} {path}"
            );
            assert_eq!(reply.json()["message"], "No such endpoint on this origin.");
        }
    }
    let theirs = chars
        .reads()
        .read(
            "/api/admin/members/1002/characters",
            "characters.json#/$defs/MemberCharacters",
        )
        .await;
    assert_eq!(theirs["characters"][0]["version"], 1, "untouched");
    assert_eq!(theirs["characters"][0]["retired_at"], Value::Null);
    assert_eq!(
        chars
            .service
            .status(&bob)
            .await
            .unwrap()
            .link
            .unwrap()
            .member_id,
        "1002",
        "Bob's pending request is still there"
    );
}

#[tokio::test]
async fn writes_need_the_markers_and_character_writes_and_link_a_fresh_sign_in() {
    let chars = Chars::new(true, true).await;
    let alice = chars.portal.sign_in(ALICE).await;
    let kanna = id(&chars
        .added(&alice, "a-1", json!({"ign": "AsahiKanna"}))
        .await);
    let body = json!({"ign": "AsahiPath"}).to_string();
    let csrf = ("X-Kanade-CSRF", alice.csrf.as_str());
    let key = ("Idempotency-Key", "k-1");
    for (method, path) in [
        ("POST", MINE.to_owned()),
        ("PATCH", row(&kanna)),
        ("POST", link(&kanna)),
        ("DELETE", link(&kanna)),
        ("DELETE", LINKS.to_owned()),
    ] {
        let reply = chars
            .portal
            .send(
                Some(&alice),
                method,
                &path,
                &[PUB_ORIGIN, csrf],
                Some(&body),
            )
            .await;
        assert_eq!(
            refused(&reply),
            (400, "invalid_idempotency_key".into()),
            "{method} {path}"
        );
    }
    for (method, path) in [
        ("POST", MINE.to_owned()),
        ("PATCH", row(&kanna)),
        ("POST", link(&kanna)),
        ("DELETE", link(&kanna)),
        ("DELETE", LINKS.to_owned()),
        ("POST", LOOKUP.to_owned()),
        ("POST", cancel(&kanna)),
        ("POST", ESTIMATES.to_owned()),
    ] {
        let reply = chars
            .portal
            .send(Some(&alice), method, &path, &[PUB_ORIGIN, key], Some(&body))
            .await;
        assert_eq!(refused(&reply), (403, "csrf".into()), "{method} {path}");
        let reply = chars
            .portal
            .send(None, method, &path, &[PUB_ORIGIN, key], Some(&body))
            .await;
        assert_eq!(
            refused(&reply),
            (401, "unauthenticated".into()),
            "{method} {path}"
        );
    }

    // Past the 15-minute window: Add, Edit and Link ask to sign in again;
    // Unlink, the lookups, Cancel and reads do not.
    chars.portal.harness.advance(TimeDelta::minutes(16));
    let add = chars.add(&alice, "a-2", json!({"ign": "AsahiPath"})).await;
    assert_eq!(refused(&add), (401, "reauth_required".into()));
    let edit = chars
        .patch(
            &alice,
            &kanna,
            "e-1",
            json!({"version": 1, "class": "Kanna"}),
        )
        .await;
    assert_eq!(refused(&edit), (401, "reauth_required".into()));
    let linked = chars.link(&alice, &kanna, "l-1", NOTICE_VERSION).await;
    assert_eq!(refused(&linked), (401, "reauth_required".into()));
    let unlink = chars.unlink(&alice, &kanna, "u-1").await;
    assert_eq!(refused(&unlink), (404, "not_linked".into()));
    let all = chars
        .portal
        .write(&alice, "DELETE", LINKS, "u-2", None)
        .await;
    assert_eq!(answer(&all, 200, UNLINKED_ALL), json!({"unlinked": []}));
    answer(&chars.look_up(&alice, "AsahiKanna").await, 200, PREVIEW);
    answer(
        &chars.post(&alice, &cancel(&kanna), None).await,
        200,
        CANCELLED,
    );
    answer(
        &chars.start(&alice, json!({"character_id": kanna})).await,
        202,
        STARTED,
    );
    chars.portal.read(&alice, MINE, MY_CHARACTERS).await;
    // Signed in again, the same writes go through.
    let alice = chars.portal.sign_in(ALICE).await;
    chars
        .added(&alice, "a-2", json!({"ign": "AsahiPath"}))
        .await;
    answer(
        &chars.link(&alice, &kanna, "l-1", NOTICE_VERSION).await,
        200,
        OUTCOME,
    );
}

#[tokio::test]
async fn character_writes_share_the_member_write_bucket() {
    let chars = Chars::new(true, false).await;
    let alice = chars.portal.sign_in(ALICE).await;
    let bob = chars.portal.sign_in(BOB).await;
    let kanna = id(&chars
        .added(&alice, "a-1", json!({"ign": "AsahiKanna"}))
        .await);
    for n in 0..19 {
        let reply = chars
            .patch(&alice, &kanna, &format!("e-{n}"), json!({"version": 1}))
            .await;
        answer(&reply, 200, ROW);
    }
    for (method, path) in [
        ("PATCH", row(&kanna)),
        ("DELETE", link(&kanna)),
        ("POST", MINE.to_owned()),
    ] {
        let reply = chars
            .portal
            .write(&alice, method, &path, "e-x", Some(&json!({"version": 1})))
            .await;
        assert_eq!(
            refused(&reply),
            (429, "rate_limited".into()),
            "{method} {path}"
        );
    }
    // Reads and the lookup preview have buckets of their own, and Bob his.
    chars.portal.read(&alice, MINE, MY_CHARACTERS).await;
    answer(&chars.look_up(&alice, "AsahiKanna").await, 200, PREVIEW);
    chars.added(&bob, "b-1", json!({"ign": "BobKnight"})).await;
    // The bucket refills: two writes a minute.
    chars.portal.harness.advance(TimeDelta::minutes(1));
    let alice = chars.portal.sign_in(ALICE).await;
    answer(
        &chars
            .patch(&alice, &kanna, "e-y", json!({"version": 1}))
            .await,
        200,
        ROW,
    );
}

#[tokio::test]
async fn the_notice_link_cancel_unlink_and_my_data_follow_discord() {
    let chars = Chars::new(true, false).await;
    let alice = chars.portal.sign_in(ALICE).await;
    // An admin asks to link AsahiDK; Alice adds AsahiKanna herself.
    let dk = chars.admin_add(ALICE, json!({"ign": "AsahiDK"})).await;
    let kanna = id(&chars
        .added(&alice, "a-1", json!({"ign": "AsahiKanna"}))
        .await);
    let mine = chars.portal.read(&alice, MINE, MY_CHARACTERS).await;
    let pending = mine["pending"].as_array().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        (
            &pending[0]["character_id"],
            &pending[0]["requested_by"]["kind"]
        ),
        (&json!(dk), &json!("admin"))
    );
    assert_eq!(mine["links"][&dk]["state"], "pending");

    // The notice is v1 word for word, led by the admin's request.
    let asked = chars.portal.read(&alice, &notice(&dk), NOTICE).await;
    let words = notice_text("AsahiDK", Some(None));
    assert_eq!(asked["version"], NOTICE_VERSION);
    assert_eq!(asked["title"], words.title);
    assert_eq!(
        asked["lead"],
        "An admin asked to link AsahiDK (MapleSEA). Nothing is fetched until you press Link."
    );
    for (key, part) in [
        ("what", &words.what),
        ("why", &words.why),
        ("how_long", &words.how_long),
        ("who", &words.who),
    ] {
        assert_eq!(
            asked[key],
            json!({"heading": part.heading, "text": part.text}),
            "{key}"
        );
    }
    assert_eq!(
        (
            &asked["as_is"],
            &asked["link_label"],
            &asked["cancel_label"]
        ),
        (
            &json!(words.as_is),
            &json!("Link AsahiDK"),
            &json!("Cancel")
        )
    );
    assert_eq!(asked["request"]["character_id"], dk);
    let own = chars.portal.read(&alice, &notice(&kanna), NOTICE).await;
    assert_eq!(own["lead"], "Nothing is fetched until you press Link.");
    assert_eq!(own["request"], Value::Null);

    // Cancel changes nothing: the admin's request stays.
    let kept = answer(
        &chars.post(&alice, &cancel(&dk), None).await,
        200,
        CANCELLED,
    );
    assert_eq!(
        kept,
        json!({"request_kept": true, "requested_by_name": null})
    );
    let none = answer(
        &chars.post(&alice, &cancel(&kanna), None).await,
        200,
        CANCELLED,
    );
    assert_eq!(none["request_kept"], false);
    assert_eq!(
        chars.portal.read(&alice, &link(&dk), LINK).await["state"],
        "pending"
    );
    assert_eq!(
        chars.portal.read(&alice, &link(&kanna), LINK).await["state"],
        "not_linked"
    );

    // Link: another notice version is refused; v1 records the consent on
    // the portal and fetches at once.
    let outdated = chars.link(&alice, &dk, "l-0", NOTICE_VERSION + 1).await;
    assert_eq!(refused(&outdated), (409, "notice_changed".into()));
    let linked = answer(
        &chars.link(&alice, &dk, "l-1", NOTICE_VERSION).await,
        200,
        OUTCOME,
    );
    assert_eq!(linked["state"], "linked");
    assert_eq!(linked["link"]["state"], "linked");
    assert_eq!(linked["link"]["consent_surface"], "portal");
    assert_eq!(linked["link"]["consent_version"], NOTICE_VERSION);
    assert_eq!(linked["link"]["data"]["level"], 271);
    let stored = chars.service.status(&dk).await.unwrap().link.unwrap();
    assert_eq!(stored.consent_surface, Some(ConsentSurface::Portal));
    assert_eq!(stored.consent_version, Some(NOTICE_VERSION));
    assert_eq!(
        stored.requested_by.kind(),
        "admin",
        "the request was answered"
    );
    let already = chars.portal.get(Some(&alice), &notice(&dk)).await;
    assert_eq!(refused(&already), (409, "already_linked".into()));
    let changes = chars
        .reads()
        .store
        .settings_changes(SettingsChangeQuery {
            actor: Some(Actor::member("1001")),
            ..SettingsChangeQuery::default()
        })
        .await
        .unwrap();
    let consent = changes
        .iter()
        .find(|change| change.values.contains_key(&format!("link.{dk}")))
        .expect("the consent's History row");
    assert_eq!(consent.surface, Surface::PublicPortal);

    // My data: the link with its consent, then what is not linked.
    let data = chars.portal.read(&alice, DATA, MY_DATA).await;
    let entries = data["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    let first = &entries[0];
    assert_eq!(
        (
            &first["character_id"],
            &first["state"],
            &first["notice"],
            &first["consent_surface"]
        ),
        (&json!(dk), &json!("linked"), &json!(1), &json!("portal"))
    );
    assert_eq!(first["data"]["level"], 271);
    assert_eq!(
        (&entries[1]["character_id"], &entries[1]["state"]),
        (&json!(kanna), &json!("not_linked"))
    );

    // Unlink: no fresh sign-in, the data goes at once.
    chars.portal.harness.advance(TimeDelta::minutes(16));
    assert!(chars.service.data(&dk).await.unwrap().is_some());
    let gone = answer(&chars.unlink(&alice, &dk, "u-1").await, 200, UNLINKED);
    assert_eq!(gone, json!({"unlinked": "AsahiDK", "cards": 0}));
    assert_eq!(chars.service.data(&dk).await.unwrap(), None);
    assert_eq!(chars.service.status(&dk).await.unwrap().link, None);
    assert_eq!(
        chars.portal.read(&alice, &link(&dk), LINK).await["state"],
        "not_linked"
    );
    assert_eq!(
        answer(&chars.unlink(&alice, &dk, "u-1").await, 200, UNLINKED),
        gone,
        "a retry replays"
    );
    let again = chars.unlink(&alice, &dk, "u-2").await;
    assert_eq!(refused(&again), (404, "not_linked".into()));

    // Unlink all.
    let alice = chars.portal.sign_in(ALICE).await;
    answer(
        &chars.link(&alice, &kanna, "l-2", NOTICE_VERSION).await,
        200,
        OUTCOME,
    );
    // A failed unlink answers 503 and records nothing under the key: the
    // same key retried later removes what is left.
    chars.file.fail_unlinks(true);
    let failed = chars
        .portal
        .write(&alice, "DELETE", LINKS, "u-3", None)
        .await;
    assert_eq!(refused(&failed).0, 503);
    assert!(chars.service.status(&kanna).await.unwrap().link.is_some());
    chars.file.fail_unlinks(false);
    let all = answer(
        &chars
            .portal
            .write(&alice, "DELETE", LINKS, "u-3", None)
            .await,
        200,
        UNLINKED_ALL,
    );
    assert_eq!(all, json!({"unlinked": ["AsahiKanna"]}));
    assert_eq!(chars.service.status(&kanna).await.unwrap().link, None);
    let data = chars.portal.read(&alice, DATA, MY_DATA).await;
    assert!(
        data["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["state"] == "not_linked")
    );
}

#[tokio::test]
async fn the_lookup_preview_is_never_stored_and_ten_per_ten_minutes() {
    let chars = Chars::new(true, false).await;
    let alice = chars.portal.sign_in(ALICE).await;
    let bob = chars.portal.sign_in(BOB).await;
    for _ in 0..10 {
        let found = answer(&chars.look_up(&alice, "AsahiLumi").await, 200, PREVIEW);
        assert_eq!(
            (&found["name"], &found["level"]),
            (&json!("AsahiLumi"), &json!(271))
        );
    }
    let limited = chars.look_up(&alice, "AsahiLumi").await;
    assert_eq!(refused(&limited), (429, "rate_limited".into()));
    // Per member, and refilled one a minute.
    answer(&chars.look_up(&bob, "BobLumi").await, 200, PREVIEW);
    chars.portal.harness.advance(TimeDelta::minutes(1));
    let unknown = chars.look_up(&alice, "Nobody42").await;
    assert_eq!(refused(&unknown), (404, "not_found".into()));
    // Nothing was kept.
    let mine = chars.portal.read(&alice, MINE, MY_CHARACTERS).await;
    assert_eq!(mine["characters"], json!([]));
    assert!(
        chars
            .service
            .member_characters("1001")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn members_look_up_their_own_characters_once_per_30_minutes() {
    let chars = Chars::new(false, true).await;
    let alice = chars.portal.sign_in(ALICE).await;
    let bob = chars.portal.sign_in(BOB).await;
    let manual = |ign: &str| json!({"ign": ign, "class": "Bishop", "level": 250});
    let failing = id(&chars.added(&alice, "a-1", manual("FailedOne")).await);
    let kanna = id(&chars.added(&alice, "a-2", manual("AsahiKanna")).await);
    let bobs = id(&chars.added(&bob, "b-1", manual("BobKnight")).await);

    // A failed lookup does not count.
    let first = answer(
        &chars.start(&alice, json!({"character_id": failing})).await,
        202,
        STARTED,
    );
    assert_eq!(chars.settled(&id(&first["job"])).await, JobState::Failed);
    let second = answer(
        &chars.start(&alice, json!({"character_id": kanna})).await,
        202,
        STARTED,
    );
    let job = id(&second["job"]);
    assert_eq!(chars.settled(&job).await, JobState::Done);
    let status = chars
        .portal
        .read(&alice, &format!("{ESTIMATES}?character_id={kanna}"), STATUS)
        .await;
    assert_eq!(status["job"]["id"], job);
    let read = chars
        .portal
        .read(&alice, &format!("{ESTIMATES}/{job}"), JOB)
        .await;
    assert_eq!(read["state"], "done");

    // That one counted: the next waits 30 minutes, with or without refresh.
    for body in [
        json!({"character_id": kanna, "refresh": true}),
        json!({"character_id": failing}),
    ] {
        let limited = chars.start(&alice, body).await;
        assert_eq!(limited.status, 429, "{}", limited.text());
        let limit = limited.json();
        assert_valid(LIMITED, ESTIMATES, &limit);
        assert_eq!(limit["error"], "member_limited");
        assert_eq!(limit["next_at"], "2026-10-08T04:30:00Z");
    }

    // Bob never sees Alice's lookups, nor starts one for her character.
    let theirs = chars
        .portal
        .get(Some(&bob), &format!("{ESTIMATES}/{job}"))
        .await;
    assert_eq!(refused(&theirs), (404, "not_found".into()));
    let theirs = chars
        .portal
        .get(Some(&bob), &format!("{ESTIMATES}?character_id={kanna}"))
        .await;
    assert_eq!(refused(&theirs), (404, "not_found".into()));
    let theirs = chars.start(&bob, json!({"character_id": kanna})).await;
    assert_eq!(refused(&theirs), (404, "not_found".into()));
    // His own limit is his.
    let his = answer(
        &chars.start(&bob, json!({"character_id": bobs})).await,
        202,
        STARTED,
    );
    assert_eq!(chars.settled(&id(&his["job"])).await, JobState::Done);
    let status = chars
        .portal
        .read(&bob, &format!("{ESTIMATES}?character_id={bobs}"), STATUS)
        .await;
    assert_eq!(status["job"]["character_ids"], json!([bobs]));

    chars.portal.harness.advance(TimeDelta::minutes(31));
    // Past the session's idle window too: a new sign-in, the same limit.
    let alice = chars.portal.sign_in(ALICE).await;
    answer(
        &chars
            .start(&alice, json!({"character_id": kanna, "refresh": true}))
            .await,
        202,
        STARTED,
    );
    // MapleScouter only ever got roster IGNs.
    let targets = chars.scouter.as_ref().unwrap().targets();
    assert!(
        targets
            .iter()
            .all(|target| ["FailedOne", "AsahiKanna", "BobKnight"]
                .iter()
                .any(|ign| target.contains(ign))),
        "{targets:?}"
    );
}

#[tokio::test]
async fn the_party_sees_roster_levels_and_estimates_never_another_members_nexon_data() {
    let chars = Chars::new(true, true).await;
    let kanna = chars
        .admin_add(
            ALICE,
            json!({"ign": "AsahiKanna", "class": "Kanna", "level": 250}),
        )
        .await;
    let knight = chars
        .admin_add(
            BOB,
            json!({"ign": "BobKnight", "class": "Hero", "level": 260}),
        )
        .await;
    for (member, character) in [("1001", &kanna), ("1002", &knight)] {
        let outcome = chars
            .service
            .confirm_link(member, character, NOTICE_VERSION, ConsentSurface::Discord)
            .await
            .unwrap();
        assert!(matches!(outcome, ConfirmOutcome::Linked(..)), "{outcome:?}");
    }
    let alice = chars.portal.sign_in(ALICE).await;
    let bob = chars.portal.sign_in(BOB).await;

    let party = chars.portal.read(&alice, PARTY, PARTY_VIEW).await;
    let slots = party["slots"].as_array().unwrap();
    assert_eq!(slots.len(), 3);
    assert_eq!(
        slots[0],
        json!({
            "member": {"id": "1001", "name": "Alice"},
            "you": true,
            "character": {"ign": "AsahiKanna", "class": "Kanna", "level": 271, "level_source": "nexon", "as_of": "2026-09-28"},
            "scouter": [],
        })
    );
    assert_eq!(
        slots[1],
        json!({
            "member": {"id": "1002", "name": "Bobby"},
            "you": false,
            "character": {"ign": "BobKnight", "class": "Hero", "level": 260, "level_source": "roster", "as_of": null},
            "scouter": [],
        })
    );
    assert_eq!(
        (
            &slots[2]["you"],
            &slots[2]["character"],
            &slots[2]["scouter"]
        ),
        (&json!(false), &Value::Null, &Value::Null)
    );
    assert_eq!(party["nexon_data"], true);
    // No combat power anywhere, nor link state.
    let text = party.to_string();
    for word in ["27182818", "combat", "linked", "consent", "pending"] {
        assert!(!text.contains(word), "{word} in {text}");
    }

    // Bob sees his own NEXON level and Alice's roster level.
    let party = chars.portal.read(&bob, PARTY, PARTY_VIEW).await;
    let slots = party["slots"].as_array().unwrap();
    assert_eq!(
        (
            &slots[0]["character"]["level"],
            &slots[0]["character"]["level_source"]
        ),
        (&json!(250), &json!("roster"))
    );
    assert_eq!(
        (
            &slots[1]["character"]["level"],
            &slots[1]["character"]["level_source"]
        ),
        (&json!(271), &json!("nexon"))
    );

    // Cached MapleScouter readings per boss, never looked up here.
    let reads = chars.reads();
    let started = reads
        .ok(
            "POST",
            "/api/admin/estimates",
            json!({"run_id": "r-kalos"}),
            STARTED,
        )
        .await;
    assert_eq!(chars.settled(&id(&started["job"])).await, JobState::Done);
    let seen = chars.scouter.as_ref().unwrap().targets().len();
    let party = chars.portal.read(&alice, PARTY, PARTY_VIEW).await;
    for slot in &party["slots"].as_array().unwrap()[..2] {
        let estimates = slot["scouter"].as_array().unwrap();
        assert!(!estimates.is_empty(), "{slot}");
        assert!(estimates.iter().all(|estimate| estimate["boss"] == "Kalos"));
    }
    assert_eq!(chars.scouter.as_ref().unwrap().targets().len(), seen);

    // Not on the run: the same 404 as a run that does not exist.
    let cara = chars.portal.sign_in(CARA).await;
    let outside = chars.portal.get(Some(&cara), PARTY).await;
    assert_eq!(refused(&outside), (404, "not_found".into()));
    let unknown = chars
        .portal
        .get(Some(&alice), "/api/public/runs/r-nope/characters")
        .await;
    assert_eq!(outside.json(), unknown.json());
}
