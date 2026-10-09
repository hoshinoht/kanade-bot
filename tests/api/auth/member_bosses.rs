//! Member boss guides (catalog C7/C8) on the public origin: the catalog list
//! and event bosses exactly as admin reads them, and knowledge pages without
//! the repository path or any bullet's chatbot-only `detail` (Q8), behind the
//! member session, over the admin reads' seeded store and tracked knowledge.

use std::collections::BTreeSet;

use serde_json::Value;

use super::member_support::{Browser, MemberHarness, Options, discord_user};
use crate::{reads::Reads, schemas::assert_valid, support::Reply};

const IP: &str = "198.51.100.10";
const ALICE: u64 = 1001;
const LIST: &str = "/api/public/bosses";
const EVENTS: &str = "/api/public/bosses/events";
const CARLING: &str = "/api/public/bosses/Carling/knowledge";
const KNOWLEDGE_KEYS: [&str; 10] = [
    "animated",
    "doc",
    "hue",
    "in_use",
    "key",
    "level",
    "missions",
    "name",
    "portrait",
    "researched_as_of",
];

struct Portal {
    reads: Reads,
    harness: MemberHarness,
}

impl Portal {
    async fn new() -> Self {
        let reads = Reads::new().await;
        let harness = MemberHarness::with(Options {
            state: reads.site.state.clone(),
            boss_dir: reads.site.boss_dir.clone(),
            ..Options::default()
        })
        .await;
        harness.roster(ALICE, true).await;
        Self { reads, harness }
    }

    async fn sign_in(&self) -> Browser {
        self.harness
            .sign_in_from(discord_user(ALICE, "Member"), IP)
            .await
    }

    async fn get(&self, browser: Option<&Browser>, path: &str) -> Reply {
        let cookie = browser.map(Browser::cookie);
        let mut headers = vec![("CF-Connecting-IP", IP)];
        if let Some((name, value)) = &cookie {
            headers.push((name, value));
        }
        self.harness.get(path, &headers).await
    }

    /// A signed-in JSON read: 200, `no-store`, valid against `target`.
    async fn read(&self, browser: &Browser, path: &str, target: &str) -> Value {
        let reply = self.get(Some(browser), path).await;
        assert_eq!(reply.status, 200, "{path}: {}", reply.text());
        assert_eq!(reply.header("cache-control"), Some("no-store"), "{path}");
        let value = reply.json();
        assert_valid(target, path, &value);
        value
    }
}

fn all_keys<'a>(value: &'a Value, out: &mut BTreeSet<&'a str>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                out.insert(key);
                all_keys(inner, out);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| all_keys(item, out)),
        _ => {}
    }
}

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
    let alice = portal.sign_in().await;
    let list = portal
        .read(&alice, LIST, "bosses.json#/$defs/BossRows")
        .await;
    let admin = portal
        .reads
        .read("/api/admin/bosses", "bosses.json#/$defs/BossRows")
        .await;
    assert!(!list.as_array().unwrap().is_empty());
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
        events
            .as_array()
            .unwrap()
            .iter()
            .any(|boss| boss["key"] == "Kai" && boss["event"]["availability"].is_string())
    );
    assert_eq!(events, admin_events);
}

#[tokio::test]
async fn knowledge_drops_the_path_and_every_detail() {
    let portal = Portal::new().await;
    let alice = portal.sign_in().await;
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
        let keys: BTreeSet<&str> = member
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, KNOWLEDGE_KEYS.into_iter().collect(), "{key}");
        assert!(!has_key(&member, "detail"), "{key}");
        assert!(!has_key(&member, "path"), "{key}");
        // Everything else is the admin page's.
        assert_eq!(member["doc"], without_detail(&admin["doc"]), "{key}");
        for field in KNOWLEDGE_KEYS.into_iter().filter(|field| *field != "doc") {
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
    let alice = portal.sign_in().await;
    for path in [
        "/api/public/bosses/Nobody/knowledge",
        "/api/public/bosses/..%2F..%2Fetc/knowledge",
        "/api/public/bosses/_meta/knowledge",
        "/api/public/bosses/Carling",
    ] {
        let reply = portal.get(Some(&alice), path).await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (404, "not_found".into()),
            "{path}"
        );
    }
}

#[tokio::test]
async fn boss_reads_need_an_open_portal_and_a_session() {
    let portal = Portal::new().await;
    let alice = portal.sign_in().await;
    for path in [LIST, EVENTS, CARLING] {
        let reply = portal.get(None, path).await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (401, "unauthenticated".into()),
            "{path}"
        );
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
            assert_eq!(
                (reply.status, reply.api_error()),
                (503, "closed".into()),
                "{path}"
            );
        }
    }
    portal.harness.set_open(true);
    assert_eq!(portal.get(Some(&alice), LIST).await.status, 200);
}
