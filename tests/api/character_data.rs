//! Module `character_data` over the seeded store of `reads.rs` and a fake
//! Open API on loopback with invented fixtures: off, every route is the
//! generic 404 and Config has no health block; on, the link status, the
//! admin request (pending, no call), the member's confirmation, the refresh
//! cap, unlink and the roster hooks, with History rows of status words only
//! and nothing written to the change history.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axum::Router;
use chrono::TimeDelta;
use kanade::{
    api::admin::config::{ConfigDesk, ConfigFacts, ConfigInputs},
    domain::{
        chardata::NOTICE_VERSION,
        settings::{RuntimeSettings, SettingsChangeQuery, SettingsStore},
    },
    infrastructure::{
        character_data::{CharacterData, ConfirmOutcome, RosterSide, Timing},
        nexon::{ApiKey, NexonClient},
        store::MemoryCharData,
    },
};
use serde_json::{Value, json};

use crate::{
    reads::{CharacterDataMaker, ConfigMaker, Reads},
    support::{ADMIN_HOST, send},
};

const LINK: &str = "character_data.json#/$defs/CharacterLink";
const LOOKUP: &str = "character_data.json#/$defs/CharacterLookup";
const LOOKUP_STATUS: &str = "character_data.json#/$defs/LookupStatus";
const CREATED: &str = "characters.json#/$defs/CharacterCreated";
const MESSAGE: &str = "common.json#/$defs/Message";
const CONFIG: &str = "config.json#/$defs/ConfigView";
const IGN: &str = "Wisteria";
const OCID: &str = "ocid-invented-api-1";
/// Names the fake Open API does not know, or answers with maintenance.
const NOBODY: &str = "Nobody42";
const DOWNTIME: &str = "Downtime";
const LOOKUP_PATH: &str = "/api/admin/characters/lookup";
const ALICE: &str = "/api/admin/members/1001/characters";

fn link(id: &str) -> String {
    format!("/api/admin/characters/{id}/link")
}

fn refresh(id: &str) -> String {
    format!("/api/admin/characters/{id}/refresh")
}

/// A fake Open API answering every call with invented data; counts calls.
/// `/id` answers [`NOBODY`] as live MapleSEA answers an unknown name
/// (`400 OPENAPI00004`) and [`DOWNTIME`] with maintenance.
pub(crate) async fn fake_nexon() -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    use axum::{http::StatusCode, response::IntoResponse};
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let app = Router::new().fallback(move |request: axum::extract::Request| {
        let counted = Arc::clone(&counted);
        async move {
            counted.fetch_add(1, Ordering::SeqCst);
            let path = request.uri().path().to_owned();
            let query = request.uri().query().unwrap_or_default().to_owned();
            let failed = |code: &str| {
                let body = format!(r#"{{"error":{{"name":"{code}","message":"invented"}}}}"#);
                (StatusCode::BAD_REQUEST, body).into_response()
            };
            if path.ends_with("/v1/id") && query.contains(NOBODY) {
                failed("OPENAPI00004")
            } else if path.ends_with("/v1/id") && query.contains(DOWNTIME) {
                failed("OPENAPI00011")
            } else if path.ends_with("/v1/id") {
                format!(r#"{{"ocid":"{OCID}"}}"#).into_response()
            } else if path.ends_with("/character/basic") {
                format!(
                    r#"{{"date":"2026-09-28T00:00+08:00","character_name":"{IGN}","world_name":"Aquila","character_class":"Bishop","character_level":271,"character_image":"https://open.api.nexon.com/static/invented"}}"#
                ).into_response()
            } else {
                r#"{"final_stat":[{"stat_name":"Combat Power","stat_value":"27182818"}]}"#
                    .into_response()
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (address, calls)
}

pub(crate) fn config() -> ConfigMaker {
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

/// The module with a key (the fake's address) or without one; the service
/// is handed back for the member's confirmation (S3's Discord side).
fn module(
    nexon: Option<std::net::SocketAddr>,
    slot: Arc<std::sync::OnceLock<Arc<CharacterData>>>,
) -> CharacterDataMaker {
    skewed(nexon, slot, Arc::default())
}

/// [`module`] on the pinned clock moved by `skew` (the lookup spacing).
pub(crate) fn skewed(
    nexon: Option<std::net::SocketAddr>,
    slot: Arc<std::sync::OnceLock<Arc<CharacterData>>>,
    skew: Arc<Mutex<TimeDelta>>,
) -> CharacterDataMaker {
    Box::new(move |store, clock| {
        let service = Arc::new(CharacterData::new(
            Arc::new(MemoryCharData::new()),
            store as Arc<dyn RosterSide>,
            nexon.map(|address| NexonClient::plain(address, ApiKey::new("invented-key"))),
            Arc::new(move || clock() + *skew.lock().unwrap()),
            Timing {
                spacing: std::time::Duration::from_millis(1),
                rate_limit_unit: std::time::Duration::from_millis(1),
            },
        ));
        slot.set(Arc::clone(&service)).ok().unwrap();
        service
    })
}

/// The module over the fake Open API at `nexon` (and the config API when
/// `config`): the reads and the service, for the member's confirmation.
async fn on(
    nexon: std::net::SocketAddr,
    config: Option<ConfigMaker>,
) -> (Reads, Arc<CharacterData>) {
    let slot = Arc::new(std::sync::OnceLock::new());
    let reads = Reads::with_character_data(config, module(Some(nexon), Arc::clone(&slot))).await;
    (reads, slot.get().unwrap().clone())
}

/// `member` confirms the link from Discord (S3's /character link).
pub(crate) async fn confirm(service: &CharacterData, member: &str, id: &str) -> ConfirmOutcome {
    let surface = kanade::domain::chardata::ConsentSurface::Discord;
    let confirmed = service.confirm_link(member, id, NOTICE_VERSION, surface);
    confirmed.await.unwrap()
}

/// Create a character for Alice: its `CharacterCreated` answer.
async fn created(reads: &Reads, body: Value) -> Value {
    let reply = reads.call("POST", ALICE, body, &[]).await;
    reply.expect(201, CREATED)
}

async fn character(reads: &Reads, ign: &str) -> String {
    created(reads, json!({"ign": ign})).await["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn link_rows(reads: &Reads) -> Vec<(String, String, String)> {
    let mut rows: Vec<_> = reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .into_iter()
        .flat_map(|change| change.values.into_iter())
        .filter(|(key, _)| key.starts_with("link."))
        .map(|(_, diff)| (diff.from, diff.to, String::new()))
        .collect();
    rows.reverse();
    rows
}

#[tokio::test]
async fn every_route_is_the_generic_404_while_the_module_is_off() {
    let reads = Reads::with_characters().await;
    let answer = created(&reads, json!({"ign": IGN})).await;
    assert_eq!(answer["link"], Value::Null, "no link asked while off");
    let id = answer["id"].as_str().unwrap().to_owned();
    for (method, path) in [
        ("GET", link(&id)),
        ("POST", link(&id)),
        ("DELETE", link(&id)),
        ("POST", refresh(&id)),
        ("GET", LOOKUP_PATH.to_owned()),
        ("POST", LOOKUP_PATH.to_owned()),
    ] {
        reads
            .assert_unmounted(method, &path, json!({"version": 0}))
            .await;
    }
}

#[tokio::test]
async fn config_has_no_health_block_while_off() {
    let reads = Reads::with_config(|store| config()(store)).await;
    let view = reads.read("/api/admin/config", CONFIG).await;
    assert!(view.get("character_data").is_none());
}

#[tokio::test]
async fn without_a_key_links_are_refused_and_health_says_no_key() {
    let slot = Arc::default();
    let reads = Reads::with_character_data(Some(config()), module(None, slot)).await;
    let answer = created(&reads, json!({"ign": IGN})).await;
    assert_eq!(answer["link"], Value::Null, "no link asked without a key");
    let id = answer["id"].as_str().unwrap().to_owned();
    let lookup = reads.read(LOOKUP_PATH, LOOKUP_STATUS).await;
    assert_eq!(lookup, json!({"connected": false}));
    let refused_lookup = reads
        .call("POST", LOOKUP_PATH, json!({"ign": IGN}), &[])
        .await;
    assert_eq!(refused_lookup.error(), (409, "not_connected".into()));
    let status = reads.read(&link(&id), LINK).await;
    assert_eq!(status["state"], "not_connected");
    assert_eq!(status["attribution"], "Data based on NEXON Open API");
    let refused = reads
        .call("POST", &link(&id), json!({"version": 0}), &[])
        .await;
    assert_eq!(refused.error(), (409, "not_connected".into()));
    assert_eq!(
        refused.json()["message"],
        "Character data isn't connected yet."
    );
    let view = reads.read("/api/admin/config", CONFIG).await;
    assert_eq!(view["character_data"]["nexon"], "no_key");
    assert_eq!(view["character_data"]["budget"], 800);
    // Never part of /healthz.
    let health = send(reads.admin, "GET", ADMIN_HOST, "/healthz", &[], None).await;
    assert_eq!(health.status, 200);
}

#[tokio::test]
async fn an_admin_request_is_pending_until_the_member_confirms() {
    let (address, calls) = fake_nexon().await;
    let (reads, service) = on(address, Some(config())).await;
    // A MapleSEA create asks the member at once: pending, nothing called.
    let answer = created(&reads, json!({"ign": IGN})).await;
    assert_eq!(answer["link"], "pending");
    let id = answer["id"].as_str().unwrap().to_owned();
    let head = kanade::api::state::ReadStore::head(reads.store.as_ref())
        .await
        .unwrap();
    let asked = reads.read(&link(&id), LINK).await;
    assert_eq!(
        (
            &asked["state"],
            &asked["version"],
            &asked["requested_by"]["kind"]
        ),
        (&json!("pending"), &json!(1), &json!("admin"))
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0, "a create calls nothing");
    // Withdrawn, so the explicit request below starts from no link.
    let withdrawn = reads.call("DELETE", &link(&id), json!(null), &[]).await;
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.text());

    let status = reads.read(&link(&id), LINK).await;
    assert_eq!(
        (&status["state"], &status["version"]),
        (&json!("not_linked"), &json!(0))
    );
    assert_eq!(
        reads
            .call("POST", &link("nope"), json!({"version": 0}), &[])
            .await
            .status,
        404
    );
    let missing = reads.call("POST", &link(&id), json!({}), &[]).await;
    assert_eq!(missing.api_error(), "version_required");

    // Keyed: a retry replays the first answer.
    let key = [("Idempotency-Key", "link-1")];
    let pending = reads
        .call("POST", &link(&id), json!({"version": 0}), &key)
        .await;
    let pending = pending.expect(201, LINK);
    assert_eq!(
        (&pending["state"], &pending["version"]),
        (&json!("pending"), &json!(1))
    );
    let again = reads
        .call("POST", &link(&id), json!({"version": 0}), &key)
        .await;
    assert_eq!((again.status, again.json()), (201, pending.clone()));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "pending calls nothing");
    let taken = reads
        .call("POST", &link(&id), json!({"version": 1}), &[])
        .await;
    assert_eq!(taken.error(), (409, "already_linked".into()));
    let stale = reads
        .call("POST", &link(&id), json!({"version": 0}), &[])
        .await;
    assert_eq!(stale.error(), (409, "stale".into()));

    // The member confirms (S3's /character link): active with data.
    let ConfirmOutcome::Linked(..) = confirm(&service, "1001", &id).await else {
        panic!("not linked");
    };
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    let linked = reads.read(&link(&id), LINK).await;
    assert_eq!(linked["state"], "linked");
    assert_eq!(linked["data"]["name"], IGN);
    assert_eq!(linked["data"]["combat_power"], 27_182_818);
    assert_eq!(linked["data"]["as_of"], "2026-09-28");
    assert_eq!(linked["consent_version"], NOTICE_VERSION);
    assert!(!linked.to_string().contains(OCID), "the ocid is never sent");
    let config = reads.read("/api/admin/config", CONFIG).await;
    assert_eq!(config["character_data"]["links"]["active"], 1);
    assert_eq!(config["character_data"]["calls_24h"], 3);

    // An admin refresh may run at any time; only the budget counts it.
    let refreshed = reads.call("POST", &refresh(&id), json!(null), &[]).await;
    refreshed.expect(200, LINK);
    assert_eq!(calls.load(Ordering::SeqCst), 5, "basic and stat again");

    // Unlink: gone at once.
    let unlinked = reads.call("DELETE", &link(&id), json!(null), &[]).await;
    unlinked.expect(200, MESSAGE);
    assert_eq!(reads.read(&link(&id), LINK).await["state"], "not_linked");
    let gone = reads.call("DELETE", &link(&id), json!(null), &[]).await;
    assert_eq!(gone.error(), (404, "not_linked".into()));
    let unlinked_refresh = reads.call("POST", &refresh(&id), json!(null), &[]).await;
    assert_eq!(unlinked_refresh.api_error(), "not_linked");

    let rows: Vec<(String, String)> = link_rows(&reads)
        .await
        .into_iter()
        .map(|(from, to, _)| (from, to))
        .collect();
    assert_eq!(
        rows,
        [
            // The create's request and its withdrawal, then Link….
            ("unlinked".into(), "pending".into()),
            ("pending".into(), "unlinked".into()),
            ("unlinked".into(), "pending".into()),
            ("pending".into(), "linked".into()),
            ("linked".into(), "unlinked".into()),
        ]
    );
    // No Nexon field in History, and the change history did not move.
    for change in reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|change| change.values.keys().all(|key| key.starts_with("link.")))
    {
        let text = format!("{change:?}");
        for fixture in [OCID, "27182818", "Aquila"] {
            assert!(!text.contains(fixture), "{fixture} in {text}");
        }
    }
    let after = kanade::api::state::ReadStore::head(reads.store.as_ref())
        .await
        .unwrap();
    // The roster create is outside the chain too: nothing moved it.
    assert_eq!(head, after, "nothing in the change history");
}

#[tokio::test]
async fn roster_deletes_and_ign_edits_unlink() {
    let (address, _) = fake_nexon().await;
    let (reads, service) = on(address, None).await;
    let id = character(&reads, IGN).await;
    confirm(&service, "1001", &id).await;
    // A class edit keeps the link; an IGN edit removes it.
    let path = format!("/api/admin/characters/{id}");
    let edited = reads
        .call(
            "PATCH",
            &path,
            json!({"version": 1, "class": "Bishop"}),
            &[],
        )
        .await;
    assert_eq!(edited.status, 200, "{}", edited.text());
    assert_eq!(reads.read(&link(&id), LINK).await["state"], "linked");
    let renamed = reads
        .call(
            "PATCH",
            &path,
            json!({"version": 2, "ign": "Lavender"}),
            &[],
        )
        .await;
    assert_eq!(renamed.status, 200, "{}", renamed.text());
    assert_eq!(reads.read(&link(&id), LINK).await["state"], "not_linked");

    let other = character(&reads, "Heather9").await;
    confirm(&service, "1001", &other).await;
    let path = format!("/api/admin/characters/{other}");
    let deleted = reads.call("DELETE", &path, Value::Null, &[]).await;
    assert_eq!(deleted.status, 200, "{}", deleted.text());
    let rows = link_rows(&reads).await;
    assert_eq!(rows.iter().filter(|(_, to, _)| to == "unlinked").count(), 2);
}

async fn history_len(reads: &Reads) -> usize {
    reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn a_lookup_shows_nexon_data_once_and_stores_nothing() {
    let (address, calls) = fake_nexon().await;
    let skew = Arc::new(Mutex::new(TimeDelta::zero()));
    let slot = Arc::new(std::sync::OnceLock::new());
    let reads = Reads::with_character_data(
        Some(config()),
        skewed(Some(address), Arc::clone(&slot), Arc::clone(&skew)),
    )
    .await;
    let later = |seconds| *skew.lock().unwrap() += TimeDelta::seconds(seconds);
    let rows = history_len(&reads).await;
    assert_eq!(
        reads.read(LOOKUP_PATH, LOOKUP_STATUS).await,
        json!({"connected": true})
    );

    // Refused before any call (and before the spacing counts).
    for (body, want) in [
        (json!({"ign": "ab"}), (422, "invalid")),
        (json!({"ign": IGN, "world": "gms"}), (422, "region")),
        (json!({"ign": IGN, "level": 1}), (400, "invalid_body")),
    ] {
        let reply = reads.call("POST", LOOKUP_PATH, body.clone(), &[]).await;
        assert_eq!(reply.error(), (want.0, want.1.into()), "{body}");
    }
    let no_csrf = reads
        .without_csrf("POST", LOOKUP_PATH, &json!({"ign": IGN}))
        .await;
    assert_eq!(no_csrf.error(), (403, "csrf".into()));
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    // Found: `id` + `basic` only (no `stat`), attributed, no combat power.
    let found = reads
        .call("POST", LOOKUP_PATH, json!({"ign": IGN.to_lowercase()}), &[])
        .await;
    let found = found.expect(200, LOOKUP);
    assert_eq!(
        found,
        json!({
            "name": IGN, "world": "Aquila", "class": "Bishop", "level": 271,
            "image_url": "https://open.api.nexon.com/static/invented",
            "as_of": "2026-09-28", "attribution": "Data based on NEXON Open API",
        })
    );
    assert!(!found.to_string().contains(OCID), "the ocid is never sent");
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // Counted against the shared budget; nothing linked, kept or recorded.
    let config = reads.read("/api/admin/config", CONFIG).await;
    assert_eq!(config["character_data"]["calls_24h"], 2);
    assert_eq!(
        config["character_data"]["links"],
        json!({"pending": 0, "active": 0, "stale": 0})
    );
    assert_eq!(history_len(&reads).await, rows, "no History row");
    assert_eq!(
        reads
            .read(ALICE, "characters.json#/$defs/MemberCharacters")
            .await["characters"],
        json!([]),
        "no roster row"
    );

    // The same admin waits a few seconds between lookups.
    let soon = reads
        .call("POST", LOOKUP_PATH, json!({"ign": IGN}), &[])
        .await;
    assert_eq!(soon.error(), (429, "lookup_cooldown".into()));
    assert_eq!(calls.load(Ordering::SeqCst), 2, "no call while waiting");

    // Unknown: live MapleSEA's 400 OPENAPI00004 on `/id` is "no such name".
    later(3);
    let unknown = reads
        .call("POST", LOOKUP_PATH, json!({"ign": NOBODY}), &[])
        .await;
    assert_eq!(unknown.error(), (404, "not_found".into()));
    assert_eq!(calls.load(Ordering::SeqCst), 3, "`id` only");

    // Maintenance holds every call: the next lookup calls nothing.
    later(3);
    let down = reads
        .call("POST", LOOKUP_PATH, json!({"ign": DOWNTIME}), &[])
        .await;
    assert_eq!(down.error(), (503, "nexon_unavailable".into()));
    later(3);
    let held = reads
        .call("POST", LOOKUP_PATH, json!({"ign": IGN}), &[])
        .await;
    assert_eq!(held.error(), (503, "nexon_unavailable".into()));
    assert!(
        held.json()["message"]
            .as_str()
            .unwrap()
            .contains("maintenance")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn a_create_asks_only_for_maplesea_and_keeps_manual_values_elsewhere() {
    let (address, calls) = fake_nexon().await;
    let (reads, _) = on(address, Some(config())).await;

    // Another region: the manual class and level stay; no link is asked.
    let other = created(
        &reads,
        json!({"ign": "Primrose", "world": "gms", "class": "Hero", "level": 250}),
    )
    .await;
    assert_eq!(
        (&other["class"], &other["level"], &other["link"]),
        (&json!("Hero"), &json!(250), &Value::Null)
    );
    let id = other["id"].as_str().unwrap();
    assert_eq!(reads.read(&link(id), LINK).await["state"], "not_linked");

    // MapleSEA without class or level: pending; a keyed retry replays it
    // and asks nothing twice.
    let key = [("Idempotency-Key", "create-link-1")];
    let body = json!({"ign": IGN});
    let first = reads.call("POST", ALICE, body.clone(), &key).await;
    let first = first.expect(201, CREATED);
    assert_eq!(
        (&first["class"], &first["level"], &first["link"]),
        (&Value::Null, &Value::Null, &json!("pending"))
    );
    let again = reads.call("POST", ALICE, body, &key).await;
    assert_eq!((again.status, again.json()), (201, first.clone()));
    let rows = link_rows(&reads).await;
    assert_eq!(rows.len(), 1, "one request: {rows:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "pending calls nothing");
    let id = first["id"].as_str().unwrap();
    assert_eq!(reads.read(&link(id), LINK).await["state"], "pending");

    // A crash after the roster commit leaves the replay at `link: null`.
    // The retry resumes: with the request on file it answers `pending`
    // and asks nothing; with none on file it asks once now.
    use kanade::api::state::ReadStore;
    use kanade::infrastructure::store::replays::ReplayScope;
    let crashed = |reads: &Reads| {
        let store = reads.store.clone();
        async move {
            let stored = ReadStore::replay(
                store.as_ref(),
                ReplayScope::Config,
                "admin:token".into(),
                "create-link-1".into(),
                // The API's pinned clock (reads.rs `now`).
                chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 9, 29, 4, 0, 0).unwrap(),
            )
            .await
            .unwrap()
            .expect("the keyed create's replay");
            let mut body: Value = serde_json::from_str(&stored.body).unwrap();
            body["link"] = Value::Null;
            let mut stored = stored;
            stored.body = body.to_string();
            ReadStore::put_replay(store.as_ref(), stored).await.unwrap();
        }
    };
    let retry = json!({"ign": IGN});
    crashed(&reads).await;
    let resumed = reads.call("POST", ALICE, retry.clone(), &key).await;
    assert_eq!((resumed.status, resumed.json()), (201, first.clone()));
    assert_eq!(link_rows(&reads).await.len(), 1, "not asked again");
    crashed(&reads).await;
    let withdrawn = reads.call("DELETE", &link(id), json!(null), &[]).await;
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.text());
    let asked = reads.call("POST", ALICE, retry, &key).await;
    assert_eq!(
        (asked.status, asked.json()["link"].clone()),
        (201, json!("pending"))
    );
    assert_eq!(reads.read(&link(id), LINK).await["state"], "pending");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "still no NEXON call");
    // The listed row is the plain CharacterRow.
    let listed = reads
        .read(ALICE, "characters.json#/$defs/MemberCharacters")
        .await;
    assert!(listed["characters"][0].get("link").is_none());
}
