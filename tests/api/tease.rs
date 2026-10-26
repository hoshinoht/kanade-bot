//! The admin side of "OK to be teased" (D9/D10): the member PATCH sets the
//! opt-in and the running-joke note, refuses a note that is long, not plain
//! text or names anyone on the roster, and keys both into its replay.

use kanade_domain::members::MemberStore;
use serde_json::json;

use crate::reads::Reads;

const ALICE: &str = "/api/admin/members/1001";
const MEMBER_ROW: &str = "members.json#/$defs/MemberRow";

#[tokio::test]
async fn admins_set_the_opt_in_and_a_plain_note_that_names_nobody() {
    let reads = Reads::with_characters().await;
    let listed = reads
        .read("/api/admin/members", "members.json#/$defs/MemberRows")
        .await;
    let alice = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "1001")
        .unwrap();
    assert_eq!(
        (&alice["tease_ok"], &alice["running_joke"]),
        (&json!(false), &json!(null)),
        "off with no note by default"
    );
    let ign = reads
        .call(
            "POST",
            "/api/admin/members/1002/characters",
            json!({"ign": "Wisteria"}),
            &[],
        )
        .await;
    assert_eq!(ign.status, 201, "{}", ign.text());

    let body = json!({"tease_ok": true, "running_joke": "  Still owes the guild a Lucid clear "});
    let row = reads.ok("PATCH", ALICE, body, MEMBER_ROW).await;
    assert_eq!(
        (&row["tease_ok"], &row["running_joke"]),
        (&json!(true), &json!("Still owes the guild a Lucid clear")),
        "stored trimmed"
    );
    for (note, code) in [
        ("x".repeat(81), "note_too_long"),
        ("ask <@1002> about it".into(), "note_not_plain"),
        ("ask @bob about it".into(), "note_not_plain"),
        ("lives in #general".into(), "note_not_plain"),
        ("see https://example.invalid".into(), "note_not_plain"),
        ("**loud** runs".into(), "note_not_plain"),
        ("Bob carries again".into(), "note_names_member"),
        ("bobby's sidekick".into(), "note_names_member"),
        ("Ali forgot the pot".into(), "note_names_member"),
        ("Lost to WISTERIA again".into(), "note_names_member"),
    ] {
        assert_eq!(
            reads
                .refused("PATCH", ALICE, json!({"running_joke": note}))
                .await,
            (422, code.into()),
            "{note}"
        );
    }
    let stored = reads.store.load_member("1001").await.unwrap().unwrap();
    assert_eq!(
        stored.running_joke.as_deref(),
        Some("Still owes the guild a Lucid clear"),
        "a refused note writes nothing"
    );
    // An opt-out keeps the note; "" clears it.
    let row = reads
        .ok("PATCH", ALICE, json!({"tease_ok": false}), MEMBER_ROW)
        .await;
    assert_eq!(row["running_joke"], "Still owes the guild a Lucid clear");
    let row = reads
        .ok("PATCH", ALICE, json!({"running_joke": ""}), MEMBER_ROW)
        .await;
    assert_eq!(row["running_joke"], json!(null));
    assert_eq!(
        reads
            .refused("PATCH", ALICE, json!({"tease_ok": "yes"}))
            .await
            .0,
        400,
        "a malformed body, as the other admin PATCH fields"
    );
}

#[tokio::test]
async fn a_key_reused_with_another_tease_choice_is_a_mismatch() {
    let reads = Reads::new().await;
    let first = reads
        .keyed("PATCH", ALICE, "tease-k", json!({"tease_ok": true}))
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    let again = reads
        .keyed("PATCH", ALICE, "tease-k", json!({"tease_ok": true}))
        .await;
    assert_eq!(again.status, 200, "{}", again.text());
    let other = reads
        .keyed("PATCH", ALICE, "tease-k", json!({"tease_ok": false}))
        .await;
    assert_eq!(other.error(), (422, "idempotency_mismatch".into()));
    let note = reads
        .keyed(
            "PATCH",
            ALICE,
            "tease-k",
            json!({"tease_ok": true, "running_joke": "Brings snacks"}),
        )
        .await;
    assert_eq!(note.error(), (422, "idempotency_mismatch".into()));
    let stored = reads.store.load_member("1001").await.unwrap().unwrap();
    assert!(stored.tease_ok && stored.running_joke.is_none());
}
