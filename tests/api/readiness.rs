//! Run readiness and CP floors (module `character_data`) over the seeded
//! store of `reads.rs` (`r-kalos` XKalos with Alice 1001, Bob 1002 and Dan
//! 1004; `r-star` HMaleficStar + HLucid + Lucid9 with Alice) and the
//! tracked boss knowledge (Kalos entry level 265, Star 280). Off, the route
//! and the Config section are absent; on, floors save through Config with
//! History rows and replays, and readiness judges each slot per boss with
//! NEXON values only from an active link's unexpired data. Invented IGNs.

use std::sync::{Arc, Mutex};

use chrono::TimeDelta;
use kanade::{
    domain::{
        chardata::NOTICE_VERSION,
        settings::{SettingsChangeQuery, SettingsStore},
    },
    infrastructure::{
        character_data::{CharacterData, ConfirmOutcome, RosterSide, Timing},
        nexon::{ApiKey, NexonClient},
        store::MemoryCharData,
    },
};
use serde_json::{Value, json};

use crate::{
    character_data::{config, fake_nexon},
    reads::{CharacterDataMaker, Reads},
    schemas::assert_valid,
    support::{ADMIN_HOST, send},
};

const READY: &str = "character_data.json#/$defs/RunReadiness";
const CONFIG: &str = "config.json#/$defs/ConfigView";
const KALOS: &str = "/api/admin/runs/r-kalos/readiness";
const STAR: &str = "/api/admin/runs/r-star/readiness";
/// The fake Open API's invented answer (`character_data.rs`).
const NEXON_CP: u64 = 27_182_818;

/// The module over the fake Open API, its clock the pinned one moved by
/// `skew`; the service is handed back for the member's confirmation.
fn module(
    nexon: std::net::SocketAddr,
    skew: Arc<Mutex<TimeDelta>>,
    slot: Arc<std::sync::OnceLock<Arc<CharacterData>>>,
) -> CharacterDataMaker {
    Box::new(move |store, clock| {
        let service = Arc::new(CharacterData::new(
            Arc::new(MemoryCharData::new()),
            store as Arc<dyn RosterSide>,
            Some(NexonClient::plain(nexon, ApiKey::new("invented-key"))),
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

struct On {
    reads: Reads,
    service: Arc<CharacterData>,
    skew: Arc<Mutex<TimeDelta>>,
}

async fn on() -> On {
    let (address, _) = fake_nexon().await;
    let skew = Arc::new(Mutex::new(TimeDelta::zero()));
    let slot = Arc::new(std::sync::OnceLock::new());
    let reads = Reads::with_character_data(
        Some(config()),
        module(address, Arc::clone(&skew), Arc::clone(&slot)),
    )
    .await;
    On {
        reads,
        service: slot.get().unwrap().clone(),
        skew,
    }
}

async fn character(reads: &Reads, member: &str, ign: &str, level: u16) -> String {
    let path = format!("/api/admin/members/{member}/characters");
    let reply = reads
        .call("POST", &path, json!({"ign": ign, "level": level}), &[])
        .await;
    assert_eq!(reply.status, 201, "{}", reply.text());
    reply.json()["id"].as_str().unwrap().to_owned()
}

fn floors(floors: Value) -> Value {
    json!({"cp_floors": {"floors": floors}})
}

fn slot<'a>(answer: &'a Value, member: &str) -> &'a Value {
    answer["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["member_id"] == member)
        .unwrap_or_else(|| panic!("no slot for {member}: {answer}"))
}

async fn cp_floor_rows(reads: &Reads) -> Vec<(String, String)> {
    reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|change| change.section == "cp_floors")
        .flat_map(|change| change.values.into_iter())
        .map(|(key, diff)| {
            assert_eq!(key, "v5.character_data.cp_floors");
            (diff.from, diff.to)
        })
        .collect()
}

#[tokio::test]
async fn off_the_route_is_the_generic_404_and_config_has_no_floors() {
    let reads = Reads::with_config(|store| config()(store)).await;
    let signed_in = reads.call("GET", KALOS, json!(null), &[]).await;
    assert_eq!(
        (signed_in.status, signed_in.api_error()),
        (404, "not_found".into())
    );
    let anonymous = send(reads.admin, "GET", ADMIN_HOST, KALOS, &[], None).await;
    assert_eq!(anonymous.status, 404, "before the session check");
    assert_valid("error.json#/$defs/ApiError", KALOS, &anonymous.json());

    let view = reads.read("/api/admin/config", CONFIG).await;
    assert!(view.get("cp_floors").is_none());
    let refused = reads
        .call(
            "PATCH",
            "/api/admin/config",
            floors(json!({"Kalos": {"x": 70_000_000}})),
            &[],
        )
        .await;
    assert_eq!(
        (refused.status, refused.api_error()),
        (404, "not_found".into())
    );
    assert!(cp_floor_rows(&reads).await.is_empty());
    let stored = reads.store.settings_rows().await.unwrap();
    assert!(!stored.contains_key("v5.character_data.cp_floors"));
}

#[tokio::test]
async fn floors_save_through_config_with_history_and_replays() {
    let On { mut reads, .. } = on().await;
    let view = reads.read("/api/admin/config", CONFIG).await;
    assert_eq!(view["cp_floors"]["floors"], json!({}));
    // Every catalog boss difficulty, with the tracked knowledge's levels.
    assert_eq!(
        view["cp_floors"]["entry_levels"],
        json!({
            "Kalos": {"e": 265, "n": 265, "c": 265, "x": 265},
            "MaleficStar": {"n": 280, "h": 280},
        })
    );

    let wanted = json!({"Kalos": {"x": 70_000_000}, "MaleficStar": {"h": 45_000_000}});
    let key = [("Idempotency-Key", "floors-1")];
    let saved = reads
        .call("PATCH", "/api/admin/config", floors(wanted.clone()), &key)
        .await;
    assert_eq!(saved.status, 200, "{}", saved.text());
    let saved = saved.json();
    assert_valid(CONFIG, "PATCH cp_floors", &saved);
    assert_eq!(saved["cp_floors"]["floors"], wanted);
    let again = reads
        .call("PATCH", "/api/admin/config", floors(wanted.clone()), &key)
        .await;
    assert_eq!(
        (again.status, &again.json()["cp_floors"]),
        (200, &saved["cp_floors"])
    );
    let other = reads
        .call("PATCH", "/api/admin/config", floors(json!({})), &key)
        .await;
    assert_eq!(
        (other.status, other.api_error()),
        (422, "idempotency_mismatch".into())
    );
    let rows = cp_floor_rows(&reads).await;
    assert_eq!(rows.len(), 1, "one row per effective save: {rows:?}");
    assert_eq!(rows[0].0, "{}");
    assert_eq!(serde_json::from_str::<Value>(&rows[0].1).unwrap(), wanted);
    // History lists it as a settings save.
    let history = reads
        .read("/api/admin/history", "history.json#/$defs/HistoryPage")
        .await;
    assert!(
        history["settings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["section"] == "cp_floors"),
        "{history}"
    );

    // Refusals write nothing.
    for (body, code) in [
        (floors(json!({"Lucid": {"h": 1}})), "invalid"),
        (floors(json!({"kalos": {"x": 1}})), "invalid"),
        (floors(json!({"Kalos": {"h": 1}})), "invalid"),
        (floors(json!({"Kalos": {"x": 0}})), "invalid"),
        (
            floors(json!({"Kalos": {"x": 10_000_000_001_u64}})),
            "invalid",
        ),
        (floors(json!({"Kalos": {"x": 1.5}})), "invalid"),
        (floors(json!({"Kalos": {"x": "70000000"}})), "invalid"),
        (floors(json!({"Kalos": {}})), "invalid"),
        (floors(json!([])), "invalid"),
        (json!({"cp_floors": {}}), "invalid"),
        (json!({"cp_floors": {"entry_levels": {}}}), "read_only"),
        (json!({"cp_floors": {"floor": {}}}), "unknown_field"),
        (
            json!({"cp_floors": {"floors": {}}, "run_lengths": {"default_minutes": 20}}),
            "invalid",
        ),
    ] {
        assert_eq!(
            reads
                .refused("PATCH", "/api/admin/config", body.clone())
                .await,
            (422, code.into()),
            "{body}"
        );
    }
    let no_csrf = send(
        reads.admin,
        "PATCH",
        ADMIN_HOST,
        "/api/admin/config",
        &[("Cookie", &reads.cookie), ("Origin", "https://kanade.test")],
        Some(&floors(json!({})).to_string()),
    )
    .await;
    assert_eq!((no_csrf.status, no_csrf.api_error()), (403, "csrf".into()));
    assert_eq!(cp_floor_rows(&reads).await.len(), 1);

    // The largest floor saves; clearing every floor is a save too.
    let largest = json!({"Kalos": {"x": 10_000_000_000_u64}});
    let saved = reads
        .ok(
            "PATCH",
            "/api/admin/config",
            floors(largest.clone()),
            CONFIG,
        )
        .await;
    assert_eq!(saved["cp_floors"]["floors"], largest);
    let cleared = reads
        .ok("PATCH", "/api/admin/config", floors(json!({})), CONFIG)
        .await;
    assert_eq!(cleared["cp_floors"]["floors"], json!({}));
    assert_eq!(cp_floor_rows(&reads).await.len(), 3);

    // Switched off, even a live keyed replay answers the generic 404.
    reads.restart_without_character_data().await;
    let replay = reads
        .call("PATCH", "/api/admin/config", floors(wanted), &key)
        .await;
    assert_eq!(
        (replay.status, replay.api_error()),
        (404, "not_found".into())
    );
}

#[tokio::test]
async fn readiness_judges_each_slot_per_boss_and_writes_nothing() {
    let On {
        reads,
        service,
        skew,
    } = on().await;
    // Alice: linked (NEXON Lv. 271, CP 27,182,818); her manual level is 260.
    let alice = character(&reads, "1001", "Wisteria", 260).await;
    reads
        .ok(
            "POST",
            &format!("/api/admin/characters/{alice}/link"),
            json!({"version": 0}),
            "character_data.json#/$defs/CharacterLink",
        )
        .await;
    let ConfirmOutcome::Linked(..) = service
        .confirm_link("1001", &alice, NOTICE_VERSION)
        .await
        .unwrap()
    else {
        panic!("not linked");
    };
    // Bob: two characters, nothing picked. Dan: one, so inferred.
    character(&reads, "1002", "Gardenia", 280).await;
    character(&reads, "1002", "Camellia", 250).await;
    let dan = character(&reads, "1004", "Hollyhock", 265).await;
    reads
        .ok(
            "PATCH",
            "/api/admin/config",
            floors(json!({"Kalos": {"x": 27_000_000}})),
            CONFIG,
        )
        .await;

    let changes = reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .len();
    let head = kanade::api::state::ReadStore::head(reads.store.as_ref())
        .await
        .unwrap();
    let ready = reads.read(KALOS, READY).await;
    assert_eq!(ready["run_id"], "r-kalos");
    assert_eq!(ready["nexon_data"], true);
    assert_eq!(ready["attribution"], "Data based on NEXON Open API");
    let members: Vec<&Value> = ready["slots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|slot| &slot["member_id"])
        .collect();
    assert_eq!(members, [&json!("1001"), &json!("1002"), &json!("1004")]);

    let linked = slot(&ready, "1001");
    assert_eq!(linked["character"]["id"], alice.as_str());
    assert_eq!(
        (&linked["inferred"], &linked["link"]),
        (&json!(true), &json!("linked"))
    );
    assert_eq!(
        linked["level"],
        json!({"value": 271, "source": "nexon", "as_of": "2026-09-28"})
    );
    assert_eq!(
        linked["power"],
        json!({"kind": "nexon_cp", "value": NEXON_CP, "as_of": "2026-09-28"})
    );
    assert_eq!(
        linked["bosses"],
        json!([{
            "boss": "Kalos", "difficulty": "x", "entry_level": 265, "cp_floor": 27_000_000,
            "level": "ok", "power": "ok",
        }])
    );

    let none = slot(&ready, "1002");
    assert_eq!(
        (
            &none["character"],
            &none["link"],
            &none["level"],
            &none["power"]
        ),
        (&Value::Null, &Value::Null, &Value::Null, &Value::Null)
    );
    assert_eq!(none["inferred"], false);
    assert_eq!(
        (&none["bosses"][0]["level"], &none["bosses"][0]["power"]),
        (&json!("unknown"), &json!("unknown"))
    );

    let manual = slot(&ready, "1004");
    assert_eq!(manual["character"]["id"], dan.as_str());
    assert_eq!(
        (&manual["inferred"], &manual["link"]),
        (&json!(true), &json!("not_linked"))
    );
    assert_eq!(
        manual["level"],
        json!({"value": 265, "source": "manual", "as_of": null})
    );
    assert_eq!(manual["power"], Value::Null);
    assert_eq!(
        (&manual["bosses"][0]["level"], &manual["bosses"][0]["power"]),
        (&json!("ok"), &json!("unknown")),
        "equal to the entry level is ok"
    );

    // Several bosses, each judged: Star has knowledge but no floor; Lucid
    // is not in this catalog (no knowledge, no floor); Lucid9 is no boss.
    let star = reads.read(STAR, READY).await;
    assert_eq!(
        slot(&star, "1001")["bosses"],
        json!([
            {"boss": "MaleficStar", "difficulty": "h", "entry_level": 280, "cp_floor": null,
             "level": "under", "power": "no_floor"},
            {"boss": "Lucid", "difficulty": "h", "entry_level": null, "cp_floor": null,
             "level": "unknown", "power": "no_floor"},
        ])
    );

    // A read writes nothing.
    assert_eq!(
        reads
            .store
            .settings_changes(SettingsChangeQuery::default())
            .await
            .unwrap()
            .len(),
        changes
    );
    assert_eq!(
        kanade::api::state::ReadStore::head(reads.store.as_ref())
            .await
            .unwrap(),
        head
    );

    // Past 30 days the NEXON data is gone: manual level, no power.
    *skew.lock().unwrap() = TimeDelta::days(31);
    let expired = reads.read(KALOS, READY).await;
    assert_eq!(expired["nexon_data"], false);
    let alice_slot = slot(&expired, "1001");
    assert_eq!(alice_slot["link"], "expired");
    assert_eq!(
        alice_slot["level"],
        json!({"value": 260, "source": "manual", "as_of": null})
    );
    assert_eq!(alice_slot["power"], Value::Null);
    assert_eq!(
        (
            &alice_slot["bosses"][0]["level"],
            &alice_slot["bosses"][0]["power"]
        ),
        (&json!("under"), &json!("unknown"))
    );
    let text = expired.to_string();
    assert!(!text.contains(&NEXON_CP.to_string()), "{text}");
    assert!(!text.contains("\"nexon\""), "{text}");
}
