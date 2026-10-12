//! Module `character_data` over the seeded store of `reads.rs` and a fake
//! Open API on loopback with invented fixtures: off, every route is the
//! generic 404 and Config has no health block; on, the link status, the
//! admin request (pending, no call), the member's confirmation, the refresh
//! cap, unlink and the roster hooks, with History rows of status words only
//! and nothing written to the change history.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::Router;
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
    schemas::assert_valid,
    support::{ADMIN_HOST, send},
};

const LINK: &str = "character_data.json#/$defs/CharacterLink";
const MESSAGE: &str = "common.json#/$defs/Message";
const CONFIG: &str = "config.json#/$defs/ConfigView";
const IGN: &str = "Wisteria";
const OCID: &str = "ocid-invented-api-1";

fn link(id: &str) -> String {
    format!("/api/admin/characters/{id}/link")
}

fn refresh(id: &str) -> String {
    format!("/api/admin/characters/{id}/refresh")
}

/// A fake Open API answering every call with invented data; counts calls.
async fn fake_nexon() -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let app = Router::new().fallback(move |request: axum::extract::Request| {
        let counted = Arc::clone(&counted);
        async move {
            counted.fetch_add(1, Ordering::SeqCst);
            let path = request.uri().path().to_owned();
            if path.ends_with("/v1/id") {
                format!(r#"{{"ocid":"{OCID}"}}"#)
            } else if path.ends_with("/character/basic") {
                format!(
                    r#"{{"date":"2026-09-28T00:00+08:00","character_name":"{IGN}","world_name":"Aquila","character_class":"Bishop","character_level":271,"character_image":"https://open.api.nexon.com/static/invented"}}"#
                )
            } else {
                r#"{"final_stat":[{"stat_name":"Combat Power","stat_value":"27182818"}]}"#.into()
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

/// The module with a key (the fake's address) or without one; the service
/// is handed back for the member's confirmation (S3's Discord side).
fn module(
    nexon: Option<std::net::SocketAddr>,
    slot: Arc<std::sync::OnceLock<Arc<CharacterData>>>,
) -> CharacterDataMaker {
    Box::new(move |store, clock| {
        let service = Arc::new(CharacterData::new(
            Arc::new(MemoryCharData::new()),
            store as Arc<dyn RosterSide>,
            nexon.map(|address| NexonClient::plain(address, ApiKey::new("invented-key"))),
            clock,
            Timing {
                spacing: std::time::Duration::from_millis(1),
                rate_limit_unit: std::time::Duration::from_millis(1),
            },
        ));
        slot.set(Arc::clone(&service)).ok().unwrap();
        service
    })
}

async fn character(reads: &Reads, ign: &str) -> String {
    let reply = reads
        .call(
            "POST",
            "/api/admin/members/1001/characters",
            json!({"ign": ign}),
            &[],
        )
        .await;
    assert_eq!(reply.status, 201, "{}", reply.text());
    reply.json()["id"].as_str().unwrap().to_owned()
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
    let id = character(&reads, IGN).await;
    for (method, path) in [
        ("GET", link(&id)),
        ("POST", link(&id)),
        ("DELETE", link(&id)),
        ("POST", refresh(&id)),
    ] {
        let signed_in = reads.call(method, &path, json!({"version": 0}), &[]).await;
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
        assert_valid("error.json#/$defs/ApiError", &path, &anonymous.json());
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
    let id = character(&reads, IGN).await;
    let status = reads.read(&link(&id), LINK).await;
    assert_eq!(status["state"], "not_connected");
    assert_eq!(status["attribution"], "Data based on NEXON Open API");
    let refused = reads
        .call("POST", &link(&id), json!({"version": 0}), &[])
        .await;
    assert_eq!(
        (refused.status, refused.api_error()),
        (409, "not_connected".into())
    );
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
    let slot = Arc::new(std::sync::OnceLock::new());
    let reads =
        Reads::with_character_data(Some(config()), module(Some(address), Arc::clone(&slot))).await;
    let service = slot.get().unwrap().clone();
    let id = character(&reads, IGN).await;
    let head = kanade::api::state::ReadStore::head(reads.store.as_ref())
        .await
        .unwrap();

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
    assert_eq!(pending.status, 201, "{}", pending.text());
    let pending = pending.json();
    assert_valid(LINK, "link", &pending);
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
    assert_eq!(
        (taken.status, taken.api_error()),
        (409, "already_linked".into())
    );
    let stale = reads
        .call("POST", &link(&id), json!({"version": 0}), &[])
        .await;
    assert_eq!((stale.status, stale.api_error()), (409, "stale".into()));

    // The member confirms (S3's /character link): active with data.
    let ConfirmOutcome::Linked(..) = service
        .confirm_link("1001", &id, NOTICE_VERSION)
        .await
        .unwrap()
    else {
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

    // The refresh shares the 24-hour cap.
    let capped = reads.call("POST", &refresh(&id), json!(null), &[]).await;
    assert_eq!(
        (capped.status, capped.api_error()),
        (429, "refresh_cap".into())
    );

    // Unlink: gone at once.
    let unlinked = reads.call("DELETE", &link(&id), json!(null), &[]).await;
    assert_eq!(unlinked.status, 200);
    assert_valid(MESSAGE, "unlink", &unlinked.json());
    assert_eq!(reads.read(&link(&id), LINK).await["state"], "not_linked");
    let gone = reads.call("DELETE", &link(&id), json!(null), &[]).await;
    assert_eq!((gone.status, gone.api_error()), (404, "not_linked".into()));
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
    let slot = Arc::new(std::sync::OnceLock::new());
    let reads = Reads::with_character_data(None, module(Some(address), Arc::clone(&slot))).await;
    let service = slot.get().unwrap().clone();
    let id = character(&reads, IGN).await;
    service
        .confirm_link("1001", &id, NOTICE_VERSION)
        .await
        .unwrap();
    // A class edit keeps the link; an IGN edit removes it.
    let edited = reads
        .call(
            "PATCH",
            &format!("/api/admin/characters/{id}"),
            json!({"version": 1, "class": "Bishop"}),
            &[],
        )
        .await;
    assert_eq!(edited.status, 200, "{}", edited.text());
    assert_eq!(reads.read(&link(&id), LINK).await["state"], "linked");
    let renamed = reads
        .call(
            "PATCH",
            &format!("/api/admin/characters/{id}"),
            json!({"version": 2, "ign": "Lavender"}),
            &[],
        )
        .await;
    assert_eq!(renamed.status, 200, "{}", renamed.text());
    assert_eq!(reads.read(&link(&id), LINK).await["state"], "not_linked");

    let other = character(&reads, "Heather9").await;
    service
        .confirm_link("1001", &other, NOTICE_VERSION)
        .await
        .unwrap();
    let deleted = reads
        .call(
            "DELETE",
            &format!("/api/admin/characters/{other}"),
            Value::Null,
            &[],
        )
        .await;
    assert_eq!(deleted.status, 200, "{}", deleted.text());
    let rows = link_rows(&reads).await;
    assert_eq!(rows.iter().filter(|(_, to, _)| to == "unlinked").count(), 2);
}
