//! Member boss guides (catalog C7/C8) on the public origin: the catalog list
//! and event bosses exactly as admin reads them, and knowledge pages without
//! the repository path or any bullet's chatbot-only `detail` (Q8), behind the
//! member session, over the admin reads' seeded store and tracked knowledge.

use std::collections::BTreeSet;

use serde_json::Value;

use super::member_writes::{ALICE, IP, Portal, all_keys, keys, set};
use crate::support::items;

const LIST: &str = "/api/public/bosses";
const EVENTS: &str = "/api/public/bosses/events";
const CARLING: &str = "/api/public/bosses/Carling/knowledge";
const KNOWLEDGE_KEYS: &str =
    "animated doc hue in_use key level missions name portrait researched_as_of";

fn has_key(value: &Value, key: &str) -> bool {
    let mut keys = BTreeSet::new();
    all_keys(value, &mut keys);
    keys.contains(key)
}

/// The admin document with every `detail` removed, as the member should see it.
fn without_detail(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| key.as_str() != "detail")
                .map(|(key, inner)| (key.clone(), without_detail(inner)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_detail).collect()),
        other => other.clone(),
    }
}

#[tokio::test]
async fn the_list_and_event_bosses_are_the_admin_reads() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let list = portal
        .read(&alice, LIST, "bosses.json#/$defs/BossRows")
        .await;
    let admin = portal
        .reads
        .read("/api/admin/bosses", "bosses.json#/$defs/BossRows")
        .await;
    assert!(!items(&list).is_empty());
    assert_eq!(list, admin);

    let events = portal
        .read(&alice, EVENTS, "bosses.json#/$defs/EventBosses")
        .await;
    let admin_events = portal
        .reads
        .read("/api/admin/bosses/events", "bosses.json#/$defs/EventBosses")
        .await;
    // Out-of-season event bosses are listed, marked by their `event` (Q9).
    assert!(
        items(&events)
            .iter()
            .any(|boss| boss["key"] == "Kai" && boss["event"]["availability"].is_string())
    );
    assert_eq!(events, admin_events);
}

#[tokio::test]
async fn knowledge_drops_the_path_and_every_detail() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    for key in ["Carling", "MaleficStar", "Kai", "StarWyrm"] {
        let path = format!("/api/public/bosses/{key}/knowledge");
        let member = portal
            .read(&alice, &path, "public.json#/$defs/PublicKnowledge")
            .await;
        let admin = portal
            .reads
            .read(
                &format!("/api/admin/bosses/{key}/knowledge"),
                "bosses.json#/$defs/Knowledge",
            )
            .await;
        assert_eq!(keys(&member), set(KNOWLEDGE_KEYS), "{key}");
        assert!(!has_key(&member, "detail"), "{key}");
        assert!(!has_key(&member, "path"), "{key}");
        // Everything else is the admin page's.
        assert_eq!(member["doc"], without_detail(&admin["doc"]), "{key}");
        for field in set(KNOWLEDGE_KEYS)
            .into_iter()
            .filter(|field| *field != "doc")
        {
            assert_eq!(member[field], admin[field], "{key}.{field}");
        }
    }
    // The tracked guides do carry chatbot wording, so the strip is exercised.
    let admin = portal
        .reads
        .read(
            "/api/admin/bosses/Carling/knowledge",
            "bosses.json#/$defs/Knowledge",
        )
        .await;
    assert!(has_key(&admin["doc"], "detail"));
    assert_eq!(admin["path"], "boss/knowledge/carling.yaml");
}

#[tokio::test]
async fn unknown_keys_are_not_found() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    for path in [
        "/api/public/bosses/Nobody/knowledge",
        "/api/public/bosses/..%2F..%2Fetc/knowledge",
        "/api/public/bosses/_meta/knowledge",
        "/api/public/bosses/Carling",
    ] {
        let reply = portal.get(Some(&alice), path).await;
        assert_eq!(reply.error(), (404, "not_found".into()), "{path}");
    }
}

#[tokio::test]
async fn boss_reads_need_an_open_portal_and_a_session() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    for path in [LIST, EVENTS, CARLING] {
        let reply = portal.get(None, path).await;
        assert_eq!(reply.error(), (401, "unauthenticated".into()), "{path}");
        assert_eq!(reply.header("cache-control"), Some("no-store"));
    }
    // An admin session means nothing here.
    let admin_cookie = portal.reads.cookie.clone();
    let reply = portal
        .harness
        .get(LIST, &[("Cookie", &admin_cookie), ("CF-Connecting-IP", IP)])
        .await;
    assert_eq!(reply.status, 401);

    portal.harness.set_open(false);
    for browser in [None, Some(&alice)] {
        for path in [LIST, EVENTS, CARLING, "/api/public/bosses/Nobody/knowledge"] {
            let reply = portal.get(browser, path).await;
            assert_eq!(reply.error(), (503, "closed".into()), "{path}");
        }
    }
    portal.harness.set_open(true);
    assert_eq!(portal.get(Some(&alice), LIST).await.status, 200);
}
