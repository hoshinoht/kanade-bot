//! The character each party member brings to a weekly timing (modules
//! `roster` and `character_data`) over the seeded store of `reads.rs`
//! (`f-kalos` XKalos Tue 22:00 with Alice 1001, Bob 1002 and Cara 1003; its
//! runs `r-kalos` this week, amended to Alice, Bob and Dan 1004, and
//! `n-kalos` next week). Either module off, every route is the generic 404;
//! on, a pick saves directly with a slot version, History row and replay,
//! runs read it live under their own pick, and readiness follows the saved
//! timing or the editor's form. Invented IGNs.

use chrono::TimeDelta;
use kanade::domain::{
    characters::{CharacterStore, PickScope},
    settings::{SettingsChange, SettingsChangeQuery, SettingsStore},
};
use serde_json::{Value, json};

use crate::{
    readiness::{On, character, on, slot},
    reads::Reads,
    schemas::assert_valid,
    support::{ADMIN_HOST, send},
};

const TIMING: &str = "characters.json#/$defs/TimingCharacters";
const SLOT: &str = "characters.json#/$defs/RunCharacterSlot";
const RUN: &str = "characters.json#/$defs/RunCharacters";
const READY: &str = "character_data.json#/$defs/TimingReadiness";
const RUN_READY: &str = "character_data.json#/$defs/RunReadiness";
const KALOS: &str = "/api/admin/fixed/f-kalos/characters";
const KALOS_READY: &str = "/api/admin/fixed/f-kalos/readiness";
const PREVIEW: &str = "/api/admin/fixed/readiness";
const KEY: &str = "fixedpick.f-kalos.1001";

fn pick_path(fixed: &str, member: &str) -> String {
    format!("/api/admin/fixed/{fixed}/characters/{member}")
}

async fn records(reads: &Reads, key: &str) -> Vec<SettingsChange> {
    reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|change| change.values.contains_key(key))
        .collect()
}

async fn stored(reads: &Reads, fixed: &str) -> usize {
    reads
        .store
        .picks(PickScope::Timing, &[fixed.to_owned()])
        .await
        .unwrap()
        .len()
}

async fn brings(reads: &Reads, run: &str, member: &str) -> (Value, Value, Value) {
    let answer = reads
        .read(&format!("/api/admin/runs/{run}/characters"), RUN)
        .await;
    let found = slot(&answer, member);
    (
        found["brings"].clone(),
        found["basis"].clone(),
        found["picked"].clone(),
    )
}

#[tokio::test]
async fn every_route_is_the_generic_404_while_either_module_is_off() {
    let put = pick_path("f-kalos", "1001");
    let body = json!({"character_id": null, "version": 0});
    let routes = [
        ("GET", KALOS),
        ("PUT", put.as_str()),
        ("GET", KALOS_READY),
        ("POST", PREVIEW),
    ];
    // Neither module, then the roster alone (character data needs it).
    for reads in [Reads::new().await, Reads::with_characters().await] {
        for (method, path) in routes {
            let signed_in = reads.call(method, path, body.clone(), &[]).await;
            assert_eq!(
                (signed_in.status, signed_in.api_error()),
                (404, "not_found".into()),
                "{method} {path}"
            );
            let anonymous = send(
                reads.admin,
                method,
                ADMIN_HOST,
                path,
                &[("Origin", "https://kanade.test")],
                Some("{}"),
            )
            .await;
            assert_eq!(anonymous.status, 404, "{method} {path} signed out");
            assert_valid("error.json#/$defs/ApiError", path, &anonymous.json());
        }
    }
}

#[tokio::test]
async fn a_timing_pick_saves_with_a_version_a_history_row_and_a_replay() {
    let On { mut reads, .. } = on().await;
    let lilac = character(&reads, "1001", "Lilac", 280).await;
    let wisteria = character(&reads, "1001", "Wisteria", 270).await;
    let answer = reads.read(KALOS, TIMING).await;
    assert_eq!(answer["fixed_run_id"], "f-kalos");
    let members: Vec<&str> = answer["slots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|slot| slot["member_id"].as_str().unwrap())
        .collect();
    assert_eq!(members, ["1001", "1002", "1003"], "the saved party");
    let alice = slot(&answer, "1001");
    assert_eq!(
        (&alice["brings"], &alice["basis"], &alice["picked"]),
        (&json!(lilac), &json!("main"), &Value::Null)
    );
    assert_eq!(alice["warnings"], json!([]));
    assert_eq!(
        (
            &slot(&answer, "1002")["brings"],
            &slot(&answer, "1002")["basis"]
        ),
        (&Value::Null, &Value::Null),
        "no characters"
    );

    let head = reads.version().await;
    let path = pick_path("f-kalos", "1001");
    let body = json!({"character_id": wisteria, "version": 0});
    let key = [("Idempotency-Key", "fixed-pick-k")];
    let first = reads.call("PUT", &path, body.clone(), &key).await;
    assert_eq!(first.status, 200, "{}", first.text());
    let first = first.json();
    assert_valid(SLOT, &path, &first);
    assert_eq!(
        (
            &first["picked"],
            &first["brings"],
            &first["basis"],
            &first["version"]
        ),
        (
            &json!(wisteria),
            &json!(wisteria),
            &json!("timing_pick"),
            &json!(1)
        )
    );
    assert_eq!(first["picked_by"]["kind"], "admin");
    assert!(first["picked_at"].is_string());

    // The retry replays across a restart; another body under the key is refused.
    reads.restart(TimeDelta::zero(), None).await;
    let again = reads.call("PUT", &path, body, &key).await;
    assert_eq!((again.status, again.json()), (200, first.clone()));
    let other = reads
        .call(
            "PUT",
            &path,
            json!({"character_id": lilac, "version": 1}),
            &key,
        )
        .await;
    assert_eq!(
        (other.status, other.api_error()),
        (422, "idempotency_mismatch".into())
    );
    // A write from the version before is stale and writes nothing.
    assert_eq!(
        reads
            .refused("PUT", &path, json!({"character_id": lilac, "version": 0}))
            .await,
        (409, "stale".into())
    );
    assert_eq!(reads.version().await, head, "outside the history chain");
    let rows = records(&reads, KEY).await;
    assert_eq!(
        rows.len(),
        1,
        "one History row; the replay and refusals none"
    );
    assert_eq!(rows[0].section, "characters");
    assert_eq!(rows[0].values[KEY].from, "");
    assert!(rows[0].values[KEY].to.contains(r#""fixed":"f-kalos""#));
    assert!(rows[0].values[KEY].to.contains(r#""ign":"Wisteria""#));

    // Clearing goes back to the main and deletes the row.
    let cleared = reads
        .ok(
            "PUT",
            &path,
            json!({"character_id": null, "version": 1}),
            SLOT,
        )
        .await;
    assert_eq!(
        (
            &cleared["picked"],
            &cleared["brings"],
            &cleared["basis"],
            &cleared["version"]
        ),
        (&Value::Null, &json!(lilac), &json!("main"), &json!(0))
    );
    assert_eq!(stored(&reads, "f-kalos").await, 0);
    assert_eq!(records(&reads, KEY).await.len(), 2);
}

#[tokio::test]
async fn timing_pick_refusals_write_nothing() {
    let On { reads, .. } = on().await;
    let lilac = character(&reads, "1001", "Lilac", 280).await;
    let wisteria = character(&reads, "1001", "Wisteria", 270).await;
    let heather = character(&reads, "1002", "Heather", 275).await;
    reads
        .ok(
            "PATCH",
            &format!("/api/admin/characters/{wisteria}"),
            json!({"version": 1, "retired": true}),
            "characters.json#/$defs/CharacterRow",
        )
        .await;
    let alice = pick_path("f-kalos", "1001");
    for (path, body, want) in [
        (
            alice.clone(),
            json!({"character_id": lilac}),
            (422, "version_required"),
        ),
        (alice.clone(), json!({"version": 0}), (422, "invalid")),
        (
            alice.clone(),
            json!({"character_id": lilac, "version": 0, "x": 1}),
            (400, "invalid_body"),
        ),
        (
            pick_path("f-gone", "1001"),
            json!({"character_id": lilac, "version": 0}),
            (404, "not_found"),
        ),
        (
            pick_path("f-kalos", "1004"),
            json!({"character_id": null, "version": 0}),
            (422, "not_on_timing"),
        ),
        (
            alice.clone(),
            json!({"character_id": "c-missing", "version": 0}),
            (404, "not_found"),
        ),
        (
            alice.clone(),
            json!({"character_id": heather, "version": 0}),
            (422, "not_their_character"),
        ),
        (
            alice.clone(),
            json!({"character_id": wisteria, "version": 0}),
            (422, "character_retired"),
        ),
        (
            alice.clone(),
            json!({"character_id": lilac, "version": 2}),
            (409, "stale"),
        ),
    ] {
        assert_eq!(
            reads.refused("PUT", &path, body.clone()).await,
            (want.0, want.1.into()),
            "{path} {body}"
        );
    }
    assert_eq!(
        reads
            .refused("GET", "/api/admin/fixed/f-gone/characters", json!({}))
            .await,
        (404, "not_found".into())
    );
    let no_csrf = send(
        reads.admin,
        "PUT",
        ADMIN_HOST,
        &alice,
        &[("Cookie", &reads.cookie), ("Origin", "https://kanade.test")],
        Some(&json!({"character_id": lilac, "version": 0}).to_string()),
    )
    .await;
    assert_eq!((no_csrf.status, no_csrf.api_error()), (403, "csrf".into()));
    assert_eq!(stored(&reads, "f-kalos").await, 0, "nothing written");
    assert!(records(&reads, KEY).await.is_empty());
}

#[tokio::test]
async fn runs_read_the_timing_pick_live_under_their_own() {
    let On { reads, .. } = on().await;
    let lilac = character(&reads, "1001", "Lilac", 280).await;
    let wisteria = character(&reads, "1001", "Wisteria", 270).await;
    let hydrangea = character(&reads, "1001", "Hydrangea", 290).await;
    // Main beats the other characters with no pick anywhere.
    for run in ["r-kalos", "n-kalos"] {
        assert_eq!(
            brings(&reads, run, "1001").await,
            (json!(lilac), json!("main"), Value::Null)
        );
    }
    // A timing pick re-points both runs of the timing at once: nothing is
    // copied, and the runs keep no pick of their own.
    let timing = pick_path("f-kalos", "1001");
    reads
        .ok(
            "PUT",
            &timing,
            json!({"character_id": wisteria, "version": 0}),
            SLOT,
        )
        .await;
    for run in ["r-kalos", "n-kalos"] {
        assert_eq!(
            brings(&reads, run, "1001").await,
            (json!(wisteria), json!("timing_pick"), Value::Null),
            "{run}"
        );
    }
    let ready = reads
        .read("/api/admin/runs/r-kalos/readiness", RUN_READY)
        .await;
    assert_eq!(slot(&ready, "1001")["character"]["ign"], "Wisteria");
    assert_eq!(slot(&ready, "1001")["basis"], "timing_pick");
    // The run's own pick beats the timing's.
    reads
        .ok(
            "PUT",
            "/api/admin/runs/n-kalos/characters/1001",
            json!({"character_id": hydrangea, "version": 0}),
            SLOT,
        )
        .await;
    assert_eq!(
        brings(&reads, "n-kalos", "1001").await,
        (json!(hydrangea), json!("run_pick"), json!(hydrangea))
    );
    // Changing the timing's pick moves only the run with no pick of its own.
    reads
        .ok(
            "PUT",
            &timing,
            json!({"character_id": lilac, "version": 1}),
            SLOT,
        )
        .await;
    assert_eq!(
        brings(&reads, "r-kalos", "1001").await,
        (json!(lilac), json!("timing_pick"), Value::Null)
    );
    assert_eq!(
        brings(&reads, "n-kalos", "1001").await,
        (json!(hydrangea), json!("run_pick"), json!(hydrangea))
    );
    assert!(
        reads
            .store
            .picks(PickScope::Run, &["r-kalos".to_owned()])
            .await
            .unwrap()
            .is_empty(),
        "the timing pick is never copied onto the run"
    );

    // Retiring the timing keeps its pick rows but no read uses them.
    let retired = reads
        .call("DELETE", "/api/admin/fixed/f-kalos", json!(null), &[])
        .await;
    assert_eq!(retired.status, 200, "{}", retired.text());
    assert_eq!(stored(&reads, "f-kalos").await, 1);
    assert_eq!(
        reads.refused("GET", KALOS, json!({})).await,
        (404, "not_found".into())
    );
}

#[tokio::test]
async fn timing_readiness_reads_the_saved_timing_or_the_form() {
    let On { reads, .. } = on().await;
    character(&reads, "1001", "Lilac", 280).await;
    let wisteria = character(&reads, "1001", "Wisteria", 260).await;
    character(&reads, "1004", "Hollyhock", 265).await;
    reads
        .ok(
            "PUT",
            &pick_path("f-kalos", "1001"),
            json!({"character_id": wisteria, "version": 0}),
            SLOT,
        )
        .await;

    let ready = reads.read(KALOS_READY, READY).await;
    assert_eq!(ready["fixed_run_id"], "f-kalos");
    assert_eq!(ready["attribution"], "Data based on NEXON Open API");
    let members: Vec<&str> = ready["slots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|slot| slot["member_id"].as_str().unwrap())
        .collect();
    assert_eq!(members, ["1001", "1002", "1003"]);
    let alice = slot(&ready, "1001");
    assert_eq!(
        (
            &alice["character"]["ign"],
            &alice["basis"],
            &alice["candidates"]
        ),
        (&json!("Wisteria"), &json!("timing_pick"), &json!(2))
    );
    assert_eq!(
        alice["bosses"],
        json!([{"boss": "Kalos", "difficulty": "x", "entry_level": 265, "level": "under", "scouter": null}])
    );
    let bob = slot(&ready, "1002");
    assert_eq!(
        (&bob["character"], &bob["basis"], &bob["candidates"]),
        (&Value::Null, &Value::Null, &json!(0))
    );
    assert_eq!(bob["bosses"][0]["level"], "unknown");

    // The form: another boss and Dan added, not saved. Alice keeps her
    // pick; Dan, outside the saved party, brings his default.
    let form = reads
        .ok(
            "POST",
            PREVIEW,
            json!({
                "fixed_run_id": "f-kalos",
                "bosses": ["XKalos", "HMaleficStar", "Nope9"],
                "participants": ["1004", "1001"],
            }),
            READY,
        )
        .await;
    let members: Vec<&str> = form["slots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|slot| slot["member_id"].as_str().unwrap())
        .collect();
    assert_eq!(members, ["1004", "1001"], "the form's order");
    let dan = slot(&form, "1004");
    assert_eq!(
        (&dan["character"]["ign"], &dan["basis"]),
        (&json!("Hollyhock"), &json!("main"))
    );
    let levels: Vec<&str> = dan["bosses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|boss| boss["level"].as_str().unwrap())
        .collect();
    assert_eq!(levels, ["ok", "under"], "an unknown token is left out");
    assert_eq!(slot(&form, "1001")["basis"], "timing_pick");

    // A timing not added yet: no picks, only defaults.
    let new = reads
        .ok(
            "POST",
            PREVIEW,
            json!({"bosses": ["XKalos"], "participants": ["1001"]}),
            READY,
        )
        .await;
    assert_eq!(new["fixed_run_id"], Value::Null);
    assert_eq!(
        (
            &slot(&new, "1001")["character"]["ign"],
            &slot(&new, "1001")["basis"]
        ),
        (&json!("Lilac"), &json!("main"))
    );

    let many: Vec<String> = (0..65).map(|n| format!("{n}")).collect();
    for (body, want) in [
        (
            json!({"fixed_run_id": "f-gone", "bosses": [], "participants": []}),
            (404, "not_found"),
        ),
        (
            json!({"bosses": [], "participants": many}),
            (422, "invalid"),
        ),
        (json!({"bosses": []}), (400, "invalid_body")),
    ] {
        assert_eq!(
            reads.refused("POST", PREVIEW, body.clone()).await,
            (want.0, want.1.into()),
            "{body}"
        );
    }
}
