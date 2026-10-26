//! Module `character_data` over the seeded store of `reads.rs` and a fake
//! Open API on loopback with invented fixtures: off, every route is the
//! generic 404 and Config has no health block; on, the link status, the
//! admin's link (active at once, with its first fetch and the roster's copy
//! of NEXON's class and level), the refresh cap, unlink and the roster
//! hooks, with History rows of status words only and nothing written to
//! the change history.

use std::{
    net::SocketAddr,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::Router;
use chrono::TimeDelta;
use kanade::{
    api::{
        admin::config::{ConfigDesk, ConfigFacts, ConfigInputs},
        auth::Clock,
    },
    infrastructure::{
        character_data::{CharacterData, ConfirmOutcome, RosterSide, Timing},
        nexon::{ApiKey, NexonClient},
        scouter::{Estimates, ScouterClient},
        store::SqliteCharData,
    },
};
use kanade_domain::{
    chardata::NOTICE_VERSION,
    settings::{RuntimeSettings, SettingsChangeQuery, SettingsStore, keys},
};
use serde_json::{Value, json};

use crate::{
    reads::{CharacterDataMaker, ConfigMaker, Reads},
    support::{ADMIN_HOST, fields, send},
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
async fn module(
    nexon: Option<SocketAddr>,
    slot: Arc<OnceLock<Arc<CharacterData>>>,
) -> CharacterDataMaker {
    skewed(nexon, slot, Arc::default()).await
}

/// [`module`] on the pinned clock moved by `skew` (the lookup spacing).
pub(crate) async fn skewed(
    nexon: Option<SocketAddr>,
    slot: Arc<OnceLock<Arc<CharacterData>>>,
    skew: Arc<Mutex<TimeDelta>>,
) -> CharacterDataMaker {
    maker(
        Arc::new(SqliteCharData::open_temp().await),
        nexon,
        None,
        skew,
        slot,
    )
}

/// The module as serve composes it over `file`, handing its service to
/// `slot`: the fake Open API at `nexon` and the scouter at `scouter` (with
/// its client timeout) when given, on the pinned clock moved by `skew`.
pub(crate) fn maker(
    file: Arc<SqliteCharData>,
    nexon: Option<SocketAddr>,
    scouter: Option<(SocketAddr, Duration)>,
    skew: Arc<Mutex<TimeDelta>>,
    slot: Arc<OnceLock<Arc<CharacterData>>>,
) -> CharacterDataMaker {
    Box::new(move |store, clock| {
        let clock: Clock = Arc::new(move || clock() + *skew.lock().unwrap());
        let nexon = nexon.map(|address| NexonClient::plain(address, ApiKey::new("invented-key")));
        let timing = Timing {
            spacing: Duration::from_millis(1),
            rate_limit_unit: Duration::from_millis(1),
        };
        let roster = store as Arc<dyn RosterSide>;
        let mut service = CharacterData::new(file, roster, nexon, Arc::clone(&clock), timing);
        if let Some((address, timeout)) = scouter {
            let url = format!("http://{address}");
            let client = ScouterClient::new(&url).unwrap().with_timeout(timeout);
            service = service.with_estimates(Estimates::new(client, url, clock));
        }
        let service = Arc::new(service);
        slot.set(Arc::clone(&service)).ok().unwrap();
        service
    })
}

/// The module over the fake Open API at `nexon` (and the config API when
/// `config`): the reads and the service, for the member's confirmation.
async fn on(nexon: SocketAddr, config: Option<ConfigMaker>) -> (Reads, Arc<CharacterData>) {
    let slot = Arc::new(OnceLock::new());
    let module = module(Some(nexon), Arc::clone(&slot)).await;
    let reads = Reads::with_character_data(config, module).await;
    (reads, slot.get().unwrap().clone())
}

/// `member` confirms the link from Discord (S3's /character link).
pub(crate) async fn confirm(service: &CharacterData, member: &str, id: &str) -> ConfirmOutcome {
    let surface = kanade_domain::chardata::ConsentSurface::Discord;
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

/// `estimate_days` (default 7) shows in Config's module block, saves
/// through PATCH `character_data` with one History row, is published on
/// the settings channel, and refuses anything but whole days 1 to 30.
#[tokio::test]
async fn estimate_days_save_live_with_one_history_row_and_refuse_bad_values() {
    const PATH: &str = "/api/admin/config";
    let slot = Arc::new(OnceLock::new());
    let desk_slot = Arc::clone(&slot);
    let make: ConfigMaker = Box::new(move |store| {
        let desk = config()(store);
        desk_slot.set(Arc::clone(&desk)).ok().unwrap();
        desk
    });
    let reads = Reads::with_character_data(Some(make), module(None, Arc::default()).await).await;
    let desk: Arc<ConfigDesk> = Arc::clone(slot.get().unwrap());
    let view = reads.read(PATH, CONFIG).await;
    assert_eq!(view["character_data"]["estimate_days"], 7);
    let history = async || {
        let query = SettingsChangeQuery::default();
        reads.store.settings_changes(query).await.unwrap()
    };
    let before = history().await.len();
    let patch = async |body: Value| reads.call("PATCH", PATH, body, &[]).await;

    let mut changes = desk.subscribe();
    let body = json!({"character_data": {"estimate_days": 10}});
    let saved = patch(body.clone()).await.expect(200, CONFIG);
    assert_eq!(saved["character_data"]["estimate_days"], 10);
    let published = changes.borrow_and_update().settings.clone();
    assert_eq!(published.character_data.estimate_days, 10, "published live");
    let rows = reads.store.settings_rows().await.unwrap();
    assert_eq!(rows[keys::ESTIMATE_DAYS], "10");
    let recorded = history().await;
    assert_eq!(recorded.len(), before + 1);
    assert_eq!(recorded[0].section, "character_data");
    let values: Vec<_> = recorded[0].values.keys().collect();
    assert_eq!(values, [keys::ESTIMATE_DAYS], "only its row");
    let row = &recorded[0].values[keys::ESTIMATE_DAYS];
    assert_eq!((row.from.as_str(), row.to.as_str()), ("7", "10"));
    // The same value again changes and records nothing.
    patch(body).await.expect(200, CONFIG);
    assert_eq!(history().await.len(), before + 1, "a no-op records nothing");

    for bad in [json!(0), json!(31), json!(7.5), json!("7"), json!(null)] {
        let refused = patch(json!({"character_data": {"estimate_days": bad}})).await;
        assert_eq!(refused.error(), (422, "invalid".into()), "{bad}");
        assert_eq!(
            refused.json()["message"],
            "Estimates refresh every 1 to 30 days."
        );
    }
    let read_only = patch(json!({"character_data": {"budget": 5}})).await;
    assert_eq!(read_only.error(), (422, "read_only".into()));
    let unknown = patch(json!({"character_data": {"days": 5}})).await;
    assert_eq!(unknown.error(), (422, "unknown_field".into()));
    let view = reads.read(PATH, CONFIG).await;
    assert_eq!(
        view["character_data"]["estimate_days"], 10,
        "refusals keep it"
    );
    assert_eq!(history().await.len(), before + 1);
}

#[tokio::test]
async fn without_a_key_links_are_refused_and_health_says_no_key() {
    let slot = Arc::default();
    let reads = Reads::with_character_data(Some(config()), module(None, slot).await).await;
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

/// User decision 2026-10-13: an admin's link (a MapleSEA create, or
/// Link…) is active at once with no member confirmation, fetches, and
/// copies NEXON's class and level into the roster, which keeps them after
/// an unlink.
#[tokio::test]
async fn an_admin_link_is_active_at_once_and_fetches() {
    let (address, calls) = fake_nexon().await;
    let (reads, service) = on(address, Some(config())).await;
    let head = kanade::api::state::ReadStore::head(reads.store.as_ref())
        .await
        .unwrap();
    // A MapleSEA create links at once; its answer is the row after the
    // first fetch copied NEXON's class and level.
    let answer = created(&reads, json!({"ign": IGN})).await;
    assert_eq!(
        fields(&answer, &["link", "class", "level", "version"]),
        json!(["linked", "Bishop", 271, 2])
    );
    assert_eq!(
        answer["updated_by"],
        json!({"kind": "system", "id": "nexon"})
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3, "id + basic + stat");
    let id = answer["id"].as_str().unwrap().to_owned();
    let linked = reads.read(&link(&id), LINK).await;
    assert_eq!(
        fields(
            &linked,
            &[
                "state",
                "version",
                "requested_by/kind",
                "consent_version",
                "consented_at",
                "consent_surface"
            ]
        ),
        json!(["linked", 1, "admin", null, null, null])
    );
    assert_eq!(linked["data"]["name"], IGN);
    assert_eq!(linked["data"]["combat_power"], 27_182_818);
    assert_eq!(linked["data"]["as_of"], "2026-09-28");
    assert!(!linked.to_string().contains(OCID), "the ocid is never sent");
    let config = reads.read("/api/admin/config", CONFIG).await;
    assert_eq!(config["character_data"]["links"]["active"], 1);
    assert_eq!(config["character_data"]["calls_24h"], 3);
    // The member is not asked: their confirmation finds it linked.
    assert_eq!(
        confirm(&service, "1001", &id).await,
        ConfirmOutcome::AlreadyLinked
    );
    // Unlinked, so Link… below starts from no link; the roster keeps the
    // copied class and level.
    let unlinked = reads.call("DELETE", &link(&id), json!(null), &[]).await;
    assert_eq!(unlinked.status, 200, "{}", unlinked.text());
    assert_eq!(
        fields(&alice_row(&reads, &id).await, &["class", "level"]),
        json!(["Bishop", 271])
    );

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

    // Link…, keyed: active with data at once; a retry replays the answer.
    let key = [("Idempotency-Key", "link-1")];
    let linked = reads
        .call("POST", &link(&id), json!({"version": 0}), &key)
        .await;
    let linked = linked.expect(201, LINK);
    assert_eq!(
        fields(&linked, &["state", "version", "data/level"]),
        json!(["linked", 1, 271])
    );
    assert_eq!(calls.load(Ordering::SeqCst), 6, "a first fetch again");
    let again = reads
        .call("POST", &link(&id), json!({"version": 0}), &key)
        .await;
    assert_eq!((again.status, again.json()), (201, linked.clone()));
    assert_eq!(calls.load(Ordering::SeqCst), 6, "a replay calls nothing");
    let taken = reads
        .call("POST", &link(&id), json!({"version": 1}), &[])
        .await;
    assert_eq!(taken.error(), (409, "already_linked".into()));
    let stale = reads
        .call("POST", &link(&id), json!({"version": 0}), &[])
        .await;
    assert_eq!(stale.error(), (409, "stale".into()));

    // An admin refresh may run at any time; only the budget counts it.
    let refreshed = reads.call("POST", &refresh(&id), json!(null), &[]).await;
    refreshed.expect(200, LINK);
    assert_eq!(calls.load(Ordering::SeqCst), 8, "basic and stat again");

    // Unlink: gone at once.
    let unlinked = reads.call("DELETE", &link(&id), json!(null), &[]).await;
    unlinked.expect(200, MESSAGE);
    assert_eq!(reads.read(&link(&id), LINK).await["state"], "not_linked");
    let gone = reads.call("DELETE", &link(&id), json!(null), &[]).await;
    assert_eq!(gone.error(), (404, "not_linked".into()));
    let unlinked_refresh = reads.call("POST", &refresh(&id), json!(null), &[]).await;
    assert_eq!(unlinked_refresh.api_error(), "not_linked");
    assert_eq!(
        fields(&alice_row(&reads, &id).await, &["class", "level"]),
        json!(["Bishop", 271])
    );

    let rows: Vec<(String, String)> = link_rows(&reads)
        .await
        .into_iter()
        .map(|(from, to, _)| (from, to))
        .collect();
    assert_eq!(
        rows,
        [
            // The create's link and its unlink, then Link… and Unlink.
            ("unlinked".into(), "linked".into()),
            ("linked".into(), "unlinked".into()),
            ("unlinked".into(), "linked".into()),
            ("linked".into(), "unlinked".into()),
        ]
    );
    // No Nexon field in a link's History row, and the change history did
    // not move.
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
    // Roster writes are outside the chain too: nothing moved it.
    assert_eq!(head, after, "nothing in the change history");
}

/// Alice's roster row `id`, as the admin reads it.
async fn alice_row(reads: &Reads, id: &str) -> Value {
    let rows = reads
        .read(ALICE, "characters.json#/$defs/MemberCharacters")
        .await;
    crate::support::find(&rows["characters"], "id", id).clone()
}

#[tokio::test]
async fn roster_deletes_and_ign_edits_unlink() {
    let (address, _) = fake_nexon().await;
    let (reads, _) = on(address, None).await;
    // Created and linked at once: version 2 holds NEXON's copy.
    let id = character(&reads, IGN).await;
    // A class edit keeps the link; an IGN edit removes it.
    let path = format!("/api/admin/characters/{id}");
    let edited = reads
        .call(
            "PATCH",
            &path,
            json!({"version": 2, "class": "Cleric"}),
            &[],
        )
        .await;
    assert_eq!(edited.status, 200, "{}", edited.text());
    assert_eq!(reads.read(&link(&id), LINK).await["state"], "linked");
    let renamed = reads
        .call(
            "PATCH",
            &path,
            json!({"version": 3, "ign": "Lavender"}),
            &[],
        )
        .await;
    assert_eq!(renamed.status, 200, "{}", renamed.text());
    assert_eq!(reads.read(&link(&id), LINK).await["state"], "not_linked");

    let other = character(&reads, "Heather9").await;
    assert_eq!(reads.read(&link(&other), LINK).await["state"], "stale");
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
        skewed(Some(address), Arc::clone(&slot), Arc::clone(&skew)).await,
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
async fn a_create_links_only_maplesea_and_keeps_manual_values_elsewhere() {
    let (address, calls) = fake_nexon().await;
    let (reads, _) = on(address, Some(config())).await;

    // Another region: the manual class and level stay; no link is made.
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

    // MapleSEA without class or level: linked, NEXON's filled in; a keyed
    // retry replays it and links nothing twice.
    let key = [("Idempotency-Key", "create-link-1")];
    let body = json!({"ign": IGN});
    let first = reads.call("POST", ALICE, body.clone(), &key).await;
    let first = first.expect(201, CREATED);
    assert_eq!(
        (&first["class"], &first["level"], &first["link"]),
        (&json!("Bishop"), &json!(271), &json!("linked"))
    );
    let again = reads.call("POST", ALICE, body, &key).await;
    assert_eq!((again.status, again.json()), (201, first.clone()));
    let rows = link_rows(&reads).await;
    assert_eq!(rows.len(), 1, "one link: {rows:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 3, "one first fetch");
    let id = first["id"].as_str().unwrap();
    assert_eq!(reads.read(&link(id), LINK).await["state"], "linked");

    // A crash after the roster commit leaves the replay at `link: null`.
    // The retry resumes: with the link on file it answers `linked` and
    // links nothing; with none on file it links once now.
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
    assert_eq!(link_rows(&reads).await.len(), 1, "not linked again");
    crashed(&reads).await;
    let unlinked = reads.call("DELETE", &link(id), json!(null), &[]).await;
    assert_eq!(unlinked.status, 200, "{}", unlinked.text());
    let linked = reads.call("POST", ALICE, retry, &key).await;
    assert_eq!(
        (linked.status, linked.json()["link"].clone()),
        (201, json!("linked"))
    );
    assert_eq!(reads.read(&link(id), LINK).await["state"], "linked");
    assert_eq!(calls.load(Ordering::SeqCst), 6, "linked once more");
    // The listed row is the plain CharacterRow.
    let listed = reads
        .read(ALICE, "characters.json#/$defs/MemberCharacters")
        .await;
    assert!(listed["characters"][0].get("link").is_none());
}
