//! The member's own preferences on the public origin (D9): the "OK to be
//! teased" opt-in they set, the admins' note read-only and whether teasing
//! is switched on; never another member's, the note never writable, keyed
//! and bucketed as the other member writes, and allowed while teasing is off.

use kanade_domain::{
    history::{Actor, Surface},
    members::MemberStore,
    settings::{SettingsChangeQuery, SettingsStore},
};
use serde_json::{Value, json};

use super::{ALICE, BOB, Portal, keys, refused, set};
use crate::{auth::member_support::PUB_ORIGIN, character_data::config, reads::Reads};

const PREFS: &str = "/api/public/me/preferences";
const MY_PREFERENCES: &str = "public.json#/$defs/MyPreferences";

async fn portal() -> Portal {
    Portal::over(Reads::with_config(config()).await).await
}

fn patch(tease_ok: impl Into<Value>) -> Value {
    json!({ "tease_ok": tease_ok.into() })
}

/// The admin Config switches, saved as an admin would.
async fn switches(portal: &Portal, body: Value) {
    let reply = portal
        .reads
        .call(
            "PATCH",
            "/api/admin/config",
            json!({ "notifications": body }),
            &[],
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
}

#[tokio::test]
async fn members_read_and_set_their_own_opt_in_while_teasing_is_off() {
    let portal = portal().await;
    let alice = portal.sign_in(ALICE).await;
    let bob = portal.sign_in(BOB).await;
    let read = portal.read(&alice, PREFS, MY_PREFERENCES).await;
    assert_eq!(
        read,
        json!({"tease_ok": false, "running_joke": null, "teasing_enabled": false})
    );

    // Teasing is off globally; the member's own choice is still theirs.
    let reply = portal
        .write(&alice, "PATCH", PREFS, "p-1", Some(&patch(true)))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let value = reply.json();
    crate::schemas::assert_valid(MY_PREFERENCES, PREFS, &value);
    assert_eq!(value["tease_ok"], true);
    let stored = portal
        .reads
        .store
        .load_member("1001")
        .await
        .unwrap()
        .unwrap();
    assert!(stored.tease_ok);
    let saves = portal
        .reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap();
    let change = saves
        .iter()
        .find(|change| change.section == "members")
        .expect("recorded");
    assert_eq!(
        (&change.actor, change.surface),
        (&Actor::member("1001"), Surface::PublicPortal)
    );
    assert!(
        change.values["member.1001"]
            .to
            .contains(r#""tease_ok":true"#)
    );

    // The admins' note shows read-only, and teasing once switched on; Bob
    // sees only his own preferences.
    let body = json!({"running_joke": "Still owes the guild a Lucid clear"});
    let note = portal
        .reads
        .call("PATCH", "/api/admin/members/1001", body, &[])
        .await;
    assert_eq!(note.status, 200, "{}", note.text());
    switches(
        &portal,
        json!({"header_estimates": true, "header_teasing": true}),
    )
    .await;
    let read = portal.read(&alice, PREFS, MY_PREFERENCES).await;
    assert_eq!(
        read,
        json!({
            "tease_ok": true,
            "running_joke": "Still owes the guild a Lucid clear",
            "teasing_enabled": true
        })
    );
    let theirs = portal.read(&bob, PREFS, MY_PREFERENCES).await;
    assert_eq!(keys(&theirs), set("tease_ok running_joke teasing_enabled"));
    assert_eq!(
        theirs,
        json!({"tease_ok": false, "running_joke": null, "teasing_enabled": true}),
        "never another member's"
    );
    // Estimates off turns teasing off with them.
    switches(&portal, json!({"header_estimates": false})).await;
    let read = portal.read(&alice, PREFS, MY_PREFERENCES).await;
    assert_eq!(read["teasing_enabled"], false);
    assert_eq!(read["tease_ok"], true, "the opt-in is kept");
}

#[tokio::test]
async fn the_note_is_not_writable_and_bodies_are_strict() {
    let portal = portal().await;
    let alice = portal.sign_in(ALICE).await;
    for (key, body) in [
        ("b-1", json!({"tease_ok": true, "running_joke": "mine now"})),
        ("b-2", json!({"running_joke": "mine now"})),
        ("b-3", json!({})),
        ("b-4", patch("yes")),
    ] {
        let reply = portal.write(&alice, "PATCH", PREFS, key, Some(&body)).await;
        assert_eq!(refused(&reply), (422, "invalid_body".into()), "{body}");
    }
    let stored = portal
        .reads
        .store
        .load_member("1001")
        .await
        .unwrap()
        .unwrap();
    assert!(!stored.tease_ok && stored.running_joke.is_none());
}

#[tokio::test]
async fn a_key_replays_and_refuses_another_choice() {
    let portal = portal().await;
    let alice = portal.sign_in(ALICE).await;
    let first = portal
        .write(&alice, "PATCH", PREFS, "k-1", Some(&patch(true)))
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    let again = portal
        .write(&alice, "PATCH", PREFS, "k-1", Some(&patch(true)))
        .await;
    assert_eq!(again.status, 200, "{}", again.text());
    assert_eq!(again.json()["tease_ok"], true);
    let other = portal
        .write(&alice, "PATCH", PREFS, "k-1", Some(&patch(false)))
        .await;
    assert_eq!(refused(&other), (422, "idempotency_mismatch".into()));
    assert!(
        portal
            .reads
            .store
            .load_member("1001")
            .await
            .unwrap()
            .unwrap()
            .tease_ok,
        "the mismatch wrote nothing"
    );
}

#[tokio::test]
async fn preferences_need_a_session_csrf_a_key_and_the_write_bucket() {
    let portal = portal().await;
    let signed_out = portal.get(None, PREFS).await;
    assert_eq!(refused(&signed_out), (401, "unauthenticated".into()));
    let body = patch(true).to_string();
    let no_session = portal
        .send(None, "PATCH", PREFS, &[PUB_ORIGIN], Some(&body))
        .await;
    assert_eq!(refused(&no_session), (401, "unauthenticated".into()));

    let alice = portal.sign_in(ALICE).await;
    let no_csrf = portal
        .send(
            Some(&alice),
            "PATCH",
            PREFS,
            &[PUB_ORIGIN, ("Idempotency-Key", "c-1")],
            Some(&body),
        )
        .await;
    assert_eq!(refused(&no_csrf).0, 403);
    let no_key = portal
        .send(
            Some(&alice),
            "PATCH",
            PREFS,
            &[PUB_ORIGIN, ("X-Kanade-CSRF", &alice.csrf)],
            Some(&body),
        )
        .await;
    assert_eq!(refused(&no_key).0, 400);

    let alice = portal.sign_in(ALICE).await;
    for n in 0..19 {
        let key = format!("w-{n}");
        let reply = portal
            .write(&alice, "PATCH", PREFS, &key, Some(&patch(n % 2 == 0)))
            .await;
        assert_eq!(reply.status, 200, "write {n}: {}", reply.text());
    }
    let limited = portal
        .write(&alice, "PATCH", PREFS, "w-x", Some(&patch(true)))
        .await;
    assert_eq!(refused(&limited), (429, "rate_limited".into()));
    // Reads have their own bucket.
    portal.read(&alice, PREFS, MY_PREFERENCES).await;
}
