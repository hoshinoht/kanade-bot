//! The manual character roster (module `roster`) over the seeded store of
//! `reads.rs`: off, every route is the generic 404; on, CRUD with CSRF,
//! version preconditions, IGN and main refusals, History rows (section
//! `characters`) and `Idempotency-Key` replays that survive a restart.
//! Invented IGNs only.

use chrono::TimeDelta;
use kanade::domain::{
    characters::CharacterStore,
    members::MemberStore,
    settings::{SettingsChange, SettingsChangeQuery, SettingsStore},
};
use serde_json::{Value, json};

use crate::{
    reads::Reads,
    schemas::assert_valid,
    support::{ADMIN_HOST, Reply, find, request, strs},
};

const ROW: &str = "characters.json#/$defs/CharacterRow";
const CREATED: &str = "characters.json#/$defs/CharacterCreated";
const LIST: &str = "characters.json#/$defs/MemberCharacters";
const MESSAGE: &str = "common.json#/$defs/Message";
const ALICE: &str = "/api/admin/members/1001/characters";

fn character(id: &str) -> String {
    format!("/api/admin/characters/{id}")
}

async fn edit(reads: &Reads, id: &str, body: Value) -> Value {
    reads.ok("PATCH", &character(id), body, ROW).await
}

async fn records(reads: &Reads) -> Vec<SettingsChange> {
    reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|change| change.section == "characters")
        .collect()
}

async fn keyed(reads: &Reads, method: &str, path: &str, key: &str, body: Value) -> Reply {
    reads
        .call(method, path, body, &[("Idempotency-Key", key)])
        .await
}

async fn created(reads: &Reads, path: &str, body: Value) -> Value {
    let reply = reads.call("POST", path, body, &[]).await;
    assert_eq!(reply.status, 201, "{}", reply.text());
    let value = reply.json();
    assert_valid(CREATED, path, &value);
    // Roster alone (module `character_data` off): no link is asked.
    assert_eq!(value["link"], Value::Null);
    value
}

#[tokio::test]
async fn every_route_is_the_generic_404_while_the_module_is_off() {
    let reads = Reads::new().await;
    // The admin app learns the module is off from its session and draws no tab.
    let session = reads
        .read("/api/admin/session", "identity.json#/$defs/Session")
        .await;
    assert_eq!(session["roster"], false);
    let id = character("c1");
    for (method, path) in [
        ("GET", ALICE),
        ("POST", ALICE),
        ("PATCH", id.as_str()),
        ("DELETE", id.as_str()),
    ] {
        let body = json!({"ign": "Wisteria", "version": 1});
        reads.assert_unmounted(method, path, body).await;
    }
    assert!(records(&reads).await.is_empty());
}

#[tokio::test]
async fn characters_are_created_listed_edited_retired_and_deleted() {
    let reads = Reads::with_characters().await;
    let session = reads
        .read("/api/admin/session", "identity.json#/$defs/Session")
        .await;
    assert_eq!(session["roster"], true);
    let empty = reads.read(ALICE, LIST).await;
    assert_eq!(empty, json!({"member_id": "1001", "characters": []}));

    let main = created(
        &reads,
        ALICE,
        json!({"ign": " Wisteria ", "class": "Bishop", "level": 260, "role": "main"}),
    )
    .await;
    assert_eq!(
        (
            &main["ign"],
            &main["world"],
            &main["class"],
            &main["level"],
            &main["role"],
            &main["version"],
            &main["retired_at"],
        ),
        (
            &json!("Wisteria"),
            &json!("msea"),
            &json!("Bishop"),
            &json!(260),
            &json!("main"),
            &json!(1),
            &Value::Null
        )
    );
    assert_eq!(main["created_at"], "2026-09-29T04:00:00Z");
    let alt = created(&reads, ALICE, json!({"ign": "Heather9"})).await;
    assert_eq!((&alt["role"], &alt["class"]), (&json!("alt"), &Value::Null));
    let listed = reads.read(ALICE, LIST).await;
    // The pinned clock gives both one creation time: the id breaks the tie.
    let mut igns = strs(&listed["characters"], "ign");
    igns.sort_unstable();
    assert_eq!(igns, ["Heather9", "Wisteria"]);

    let alt_id = alt["id"].as_str().unwrap();
    let edited = edit(
        &reads,
        alt_id,
        json!({"version": 1, "class": "Shadower", "level": 210}),
    )
    .await;
    assert_eq!(
        (&edited["class"], &edited["level"], &edited["version"]),
        (&json!("Shadower"), &json!(210), &json!(2))
    );
    let cleared = edit(
        &reads,
        alt_id,
        json!({"version": 2, "class": null, "level": null}),
    )
    .await;
    assert_eq!(
        (&cleared["class"], &cleared["level"], &cleared["version"]),
        (&Value::Null, &Value::Null, &json!(3))
    );
    let retired = edit(&reads, alt_id, json!({"version": 3, "retired": true})).await;
    assert_eq!(retired["retired_at"], "2026-09-29T04:00:00Z");
    // A no-op answers the row and records nothing.
    let same = edit(&reads, alt_id, json!({"version": 4, "retired": true})).await;
    assert_eq!(same, retired);

    let deleted = reads
        .ok("DELETE", &character(alt_id), json!({}), MESSAGE)
        .await;
    assert_eq!(deleted["message"], "Heather9 is deleted.");
    assert_eq!(
        reads.refused("DELETE", &character(alt_id), json!({})).await,
        (404, "not_found".into())
    );
    assert_eq!(
        reads.read(ALICE, LIST).await["characters"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // History: create ×2, edit ×2, retire and delete, attributed to the session.
    let saves = records(&reads).await;
    assert_eq!(saves.len(), 6);
    let newest = &saves[0];
    assert_eq!(newest.actor.kind(), "admin");
    let row = &newest.values[&format!("character.{alt_id}")];
    assert!(row.from.contains(r#""retired":true"#), "{row:?}");
    assert_eq!(row.to, "");
    let history = reads
        .read("/api/admin/history", "history.json#/$defs/HistoryPage")
        .await;
    assert_eq!(strs(&history["settings"], "section"), ["characters"; 6]);
}

#[tokio::test]
async fn writes_need_the_csrf_token_and_a_session() {
    let reads = Reads::with_characters().await;
    let body = json!({"ign": "Wisteria"});
    let no_csrf = reads.without_csrf("POST", ALICE, &body).await;
    assert_eq!(no_csrf.error(), (403, "csrf".into()));
    let signed_out = request(reads.admin, "GET", ADMIN_HOST, ALICE, &[]).await;
    assert_eq!(signed_out.status, 401);
    assert!(
        reads
            .store
            .member_characters("1001")
            .await
            .unwrap()
            .is_empty(),
        "nothing written"
    );
}

#[tokio::test]
async fn refusals_follow_the_write_mapping() {
    let reads = Reads::with_characters().await;
    let main = created(&reads, ALICE, json!({"ign": "Wisteria"})).await;
    let main_id = main["id"].as_str().unwrap();
    let bob = "/api/admin/members/1002/characters";
    for (path, body, want) in [
        (bob, json!({"ign": "WISTERIA"}), (422, "ign_taken")),
        (
            bob,
            json!({"ign": "wisteria", "world": "MSEA"}),
            (422, "ign_taken"),
        ),
        (ALICE, json!({"ign": "abc"}), (422, "invalid")),
        (ALICE, json!({"ign": "two words"}), (422, "invalid")),
        (
            ALICE,
            json!({"ign": "Lilac", "level": 301}),
            (422, "invalid"),
        ),
        (
            ALICE,
            json!({"ign": "Lilac", "class": "x".repeat(33)}),
            (422, "invalid"),
        ),
        (
            ALICE,
            json!({"ign": "Lilac", "role": "boss"}),
            (422, "invalid"),
        ),
        (
            ALICE,
            json!({"ign": "Lilac", "world": "a b"}),
            (422, "invalid"),
        ),
        (
            ALICE,
            json!({"ign": "Lilac", "guild": "x"}),
            (400, "invalid_body"),
        ),
        (
            "/api/admin/members/9999/characters",
            json!({"ign": "Lilac"}),
            (404, "not_found"),
        ),
    ] {
        assert_eq!(
            reads.refused("POST", path, body.clone()).await,
            (want.0, want.1.into()),
            "{path} {body}"
        );
    }
    // The same IGN on another world (region) is another character.
    let elsewhere = created(&reads, bob, json!({"ign": "WISTERIA", "world": "gms"})).await;
    assert_eq!(elsewhere["world"], "gms");
    let path = character(main_id);
    for (body, want) in [
        (json!({"level": 200}), (422, "version_required")),
        (json!({"version": 9, "level": 200}), (409, "stale")),
        (json!({"version": 1, "ign": "x"}), (422, "invalid")),
    ] {
        assert_eq!(
            reads.refused("PATCH", &path, body.clone()).await,
            (want.0, want.1.into()),
            "{body}"
        );
    }
    let carol = "/api/admin/members/1003/characters";
    let theirs = created(&reads, carol, json!({"ign": "Heather"})).await;
    assert_eq!(
        reads
            .refused(
                "PATCH",
                &character(theirs["id"].as_str().unwrap()),
                json!({"version": 1, "ign": "wisteria"}),
            )
            .await,
        (422, "ign_taken".into())
    );
    assert_eq!(
        reads
            .refused("PATCH", &character("missing"), json!({"version": 1}))
            .await,
        (404, "not_found".into())
    );
    assert_eq!(
        reads
            .refused("GET", "/api/admin/members/9999/characters", json!({}))
            .await,
        (404, "not_found".into())
    );
    assert_eq!(records(&reads).await.len(), 3, "the three creates only");
}

#[tokio::test]
async fn the_first_character_is_the_main_and_the_crown_moves() {
    let reads = Reads::with_characters().await;
    let first = created(&reads, ALICE, json!({"ign": "Wisteria", "role": "alt"})).await;
    assert_eq!(
        first["role"], "main",
        "a member's first character is their main"
    );
    let second = created(&reads, ALICE, json!({"ign": "Heather"})).await;
    assert_eq!(second["role"], "alt");
    let (first_id, second_id) = (
        first["id"].as_str().unwrap().to_owned(),
        second["id"].as_str().unwrap().to_owned(),
    );
    // The main stays while another character is active.
    for (method, body) in [
        ("PATCH", json!({"version": 1, "retired": true})),
        ("PATCH", json!({"version": 1, "role": "alt"})),
        ("DELETE", json!({})),
    ] {
        let reply = reads
            .call(method, &character(&first_id), body.clone(), &[])
            .await;
        assert_eq!(
            reply.error(),
            (409, "main_required".into()),
            "{method} {body}"
        );
        assert!(
            reply.json()["message"]
                .as_str()
                .unwrap()
                .contains("main first")
        );
    }
    // Make another main first: the crown moves in one write.
    let promoted = edit(&reads, &second_id, json!({"version": 1, "role": "main"})).await;
    assert_eq!(promoted["role"], "main");
    let listed = reads.read(ALICE, LIST).await;
    let mut roles: Vec<(&str, &str)> = listed["characters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| (row["ign"].as_str().unwrap(), row["role"].as_str().unwrap()))
        .collect();
    roles.sort_unstable();
    assert_eq!(roles, [("Heather", "main"), ("Wisteria", "alt")]);
    let demoted = find(&listed["characters"], "ign", "Wisteria");
    assert_eq!(demoted["version"], 2);
    assert_eq!(demoted["updated_by"]["kind"], "admin");
    let saves = records(&reads).await;
    assert_eq!(
        saves[0].values.len(),
        2,
        "one History row names both characters"
    );
    // Now the old main retires.
    edit(&reads, &first_id, json!({"version": 2, "retired": true})).await;
}

#[tokio::test]
async fn keyed_writes_replay_across_a_restart() {
    let mut reads = Reads::with_characters().await;
    let first = keyed(&reads, "POST", ALICE, "char-k", json!({"ign": "Wisteria"})).await;
    assert_eq!(first.status, 201, "{}", first.text());
    let row = first.json();
    let id = row["id"].as_str().unwrap().to_owned();

    reads.restart(TimeDelta::zero(), None).await;
    let again = keyed(&reads, "POST", ALICE, "char-k", json!({"ign": "Wisteria"})).await;
    assert_eq!(
        (again.status, again.json()),
        (201, row.clone()),
        "the first answer"
    );
    assert_eq!(
        reads.store.member_characters("1001").await.unwrap().len(),
        1,
        "created once"
    );
    let other = keyed(&reads, "POST", ALICE, "char-k", json!({"ign": "Heather"})).await;
    assert_eq!(other.error(), (422, "idempotency_mismatch".into()));

    let patch = json!({"version": 1, "level": 250});
    let edited = keyed(&reads, "PATCH", &character(&id), "char-p", patch.clone()).await;
    assert_eq!(edited.status, 200, "{}", edited.text());
    // The retry carries the version it was made from; it replays, not 409.
    let replay = keyed(&reads, "PATCH", &character(&id), "char-p", patch).await;
    assert_eq!((replay.status, replay.json()), (200, edited.json()));

    // A refused write stores no answer: the key applies once it can.
    let clash = keyed(
        &reads,
        "POST",
        "/api/admin/members/1002/characters",
        "char-b",
        json!({"ign": "wisteria"}),
    )
    .await;
    assert_eq!(clash.error(), (422, "ign_taken".into()));

    let gone = keyed(&reads, "DELETE", &character(&id), "char-d", json!({})).await;
    assert_eq!(gone.status, 200, "{}", gone.text());
    let gone_again = keyed(&reads, "DELETE", &character(&id), "char-d", json!({})).await;
    assert_eq!((gone_again.status, gone_again.json()), (200, gone.json()));
    let clash = keyed(
        &reads,
        "POST",
        "/api/admin/members/1002/characters",
        "char-b",
        json!({"ign": "wisteria"}),
    )
    .await;
    assert_eq!(clash.status, 201, "{}", clash.text());
    assert_eq!(
        records(&reads).await.len(),
        4,
        "create, edit, delete, create"
    );
}

#[tokio::test]
async fn a_departed_member_keeps_their_characters() {
    let reads = Reads::with_characters().await;
    created(&reads, ALICE, json!({"ign": "Wisteria"})).await;
    assert!(reads.store.member_departed("1001").await.unwrap());
    let listed = reads.read(ALICE, LIST).await;
    assert_eq!(listed["characters"][0]["ign"], "Wisteria");
}
