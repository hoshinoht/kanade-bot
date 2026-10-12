//! Run readiness (module `character_data`) over the seeded store of
//! `reads.rs` (`r-kalos` XKalos with Alice 1001, Bob 1002 and Dan 1004;
//! `r-star` HMaleficStar + HLucid + Lucid9 with Alice) and the tracked boss
//! knowledge (Kalos entry level 265, Star 280). Off, the route is absent; on,
//! readiness shows each slot's level and power and judges its level per
//! boss, with NEXON values only from an active link's unexpired data.
//! Invented IGNs.

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

fn slot<'a>(answer: &'a Value, member: &str) -> &'a Value {
    answer["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["member_id"] == member)
        .unwrap_or_else(|| panic!("no slot for {member}: {answer}"))
}

#[tokio::test]
async fn off_the_route_is_the_generic_404() {
    let reads = Reads::with_config(|store| config()(store)).await;
    let signed_in = reads.call("GET", KALOS, json!(null), &[]).await;
    assert_eq!(
        (signed_in.status, signed_in.api_error()),
        (404, "not_found".into())
    );
    let anonymous = send(reads.admin, "GET", ADMIN_HOST, KALOS, &[], None).await;
    assert_eq!(anonymous.status, 404, "before the session check");
    assert_valid("error.json#/$defs/ApiError", KALOS, &anonymous.json());
}

#[tokio::test]
async fn readiness_judges_each_slot_per_boss_and_writes_nothing() {
    let On {
        reads,
        service,
        skew,
    } = on().await;
    // Alice: linked (NEXON Lv. 271, CP 27,182,818); her manual level is 260.
    // The create already asked her to link it (pending).
    let alice = character(&reads, "1001", "Wisteria", 260).await;
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
    // Estimates off (no scouter URL): never a MapleScouter reading.
    assert_eq!(
        linked["bosses"],
        json!([{"boss": "Kalos", "difficulty": "x", "entry_level": 265, "level": "ok", "scouter": null}])
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
    assert_eq!(none["bosses"][0]["level"], "unknown");

    let manual = slot(&ready, "1004");
    assert_eq!(manual["character"]["id"], dan.as_str());
    // The create asked Dan to link it; until he confirms, manual values.
    assert_eq!(
        (&manual["inferred"], &manual["link"]),
        (&json!(true), &json!("pending"))
    );
    assert_eq!(
        manual["level"],
        json!({"value": 265, "source": "manual", "as_of": null})
    );
    assert_eq!(manual["power"], Value::Null);
    assert_eq!(
        manual["bosses"][0]["level"], "ok",
        "equal to the entry level is ok"
    );

    // Several bosses, each judged: Lucid is not in this catalog (no
    // knowledge); Lucid9 is no boss.
    let star = reads.read(STAR, READY).await;
    assert_eq!(
        slot(&star, "1001")["bosses"],
        json!([
            {"boss": "MaleficStar", "difficulty": "h", "entry_level": 280, "level": "under", "scouter": null},
            {"boss": "Lucid", "difficulty": "h", "entry_level": null, "level": "unknown", "scouter": null},
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
    assert_eq!(alice_slot["bosses"][0]["level"], "under");
    let text = expired.to_string();
    assert!(!text.contains(&NEXON_CP.to_string()), "{text}");
    assert!(!text.contains("\"nexon\""), "{text}");
}
