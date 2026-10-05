//! Account → Sessions: the caller lists, and signs out, only their own live
//! sessions; handles are opaque; the current session ends only through
//! sign-out; unsafe calls need CSRF; each ending is audited.

use chrono::TimeDelta;
use kanade::api::auth::{audit::AuditEvent, wire};
use serde_json::Value;

use super::{ADMIN_ROLE, Harness, ORIGIN, TOKEN, cookie, member, user};
use crate::{
    schemas::assert_valid,
    support::{ADMIN_HOST, Reply, send},
};

const LIST: &str = "/api/admin/me/sessions";
const OTHERS: &str = "/api/admin/me/sessions/sign-out-others";
const FIREFOX_MAC: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:131.0) Gecko/20100101 Firefox/131.0";
const SAFARI_IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";

/// A signed-in browser: its cookie value and CSRF token.
struct Browser {
    id: String,
    csrf: String,
}

impl Browser {
    fn cookie(&self) -> (&'static str, String) {
        cookie(&self.id)
    }
}

async fn token_browser(harness: &Harness, agent: Option<&str>) -> Browser {
    let mut headers = vec![ORIGIN];
    if let Some(agent) = agent {
        headers.push(("User-Agent", agent));
    }
    let reply = harness
        .post(
            "/api/admin/auth/token",
            &headers,
            Some(&format!(r#"{{"token":"{TOKEN}"}}"#)),
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    Browser {
        id: reply.cookie(wire::SESSION_COOKIE).unwrap(),
        csrf: reply.header("x-kanade-csrf").unwrap().to_owned(),
    }
}

async fn discord_browser(harness: &Harness, id: u64, name: &str, agent: &str) -> Browser {
    harness.guild.put(member(id, &[ADMIN_ROLE], false));
    let (path, login) = harness.discord_approved(user(id, name), "%2F").await;
    let callback = harness
        .get(&path, &[("Cookie", &login), ("User-Agent", agent)])
        .await;
    let id = callback.cookie(wire::SESSION_COOKIE).unwrap();
    let (name, value) = cookie(&id);
    let session = harness.get("/api/admin/session", &[(name, &value)]).await;
    Browser {
        csrf: session.header("x-kanade-csrf").unwrap().to_owned(),
        id,
    }
}

async fn list(harness: &Harness, browser: &Browser) -> Value {
    let (name, value) = browser.cookie();
    let reply = harness.get(LIST, &[(name, &value)]).await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let body = reply.json();
    assert_valid("identity.json#/$defs/AccountSessions", LIST, &body);
    body
}

async fn unsafe_call(
    harness: &Harness,
    browser: &Browser,
    method: &str,
    path: &str,
    csrf: bool,
) -> Reply {
    let (name, value) = browser.cookie();
    let mut headers = vec![(name, value.as_str()), ("Sec-Fetch-Site", "same-origin")];
    if csrf {
        headers.push(("X-Kanade-CSRF", &browser.csrf));
    }
    send(harness.admin, method, ADMIN_HOST, path, &headers, None).await
}

async fn alive(harness: &Harness, browser: &Browser) -> bool {
    let (name, value) = browser.cookie();
    harness
        .get("/api/admin/session", &[(name, &value)])
        .await
        .status
        == 200
}

fn handles(body: &Value) -> Vec<(String, bool, Option<String>)> {
    body["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["handle"].as_str().unwrap().to_owned(),
                row["current"].as_bool().unwrap(),
                row["device"].as_str().map(str::to_owned),
            )
        })
        .collect()
}

#[tokio::test]
async fn lists_only_live_sessions_of_the_same_identity_without_ids_or_hashes() {
    let harness = Harness::new().await;
    let mac = token_browser(&harness, Some(FIREFOX_MAC)).await;
    harness.advance(TimeDelta::minutes(5));
    let phone = token_browser(&harness, Some(SAFARI_IPHONE)).await;
    let curl = token_browser(&harness, None).await;
    let alice = discord_browser(&harness, 111, "Alice", FIREFOX_MAC).await;
    let bob = discord_browser(&harness, 222, "Bob", SAFARI_IPHONE).await;

    let body = list(&harness, &mac).await;
    let text = body.to_string();
    for browser in [&mac, &phone, &curl, &alice, &bob] {
        assert!(!text.contains(&browser.id), "no cookie value");
        assert!(
            !text.contains(&kanade::api::auth::crypto::sha256_hex(
                browser.id.as_bytes()
            )),
            "no stored hash"
        );
    }
    let rows = handles(&body);
    assert_eq!(rows.len(), 3, "the token's three sessions only: {body}");
    assert_eq!(
        (rows[0].1, rows[0].2.as_deref()),
        (true, Some("Firefox · macOS")),
        "this one first"
    );
    let devices: Vec<_> = rows[1..].iter().map(|row| row.2.as_deref()).collect();
    assert!(devices.contains(&Some("Safari · iPhone")) && devices.contains(&None));
    assert_eq!(body["sessions"][0]["method"], "token");

    let alices = handles(&list(&harness, &alice).await);
    assert_eq!(alices.len(), 1, "Bob's Discord session is not Alice's");
    assert!(alices[0].1);

    // Idle sessions drop out of the list (the other two stay active).
    harness.advance(TimeDelta::minutes(59));
    assert!(alive(&harness, &mac).await);
    assert!(alive(&harness, &phone).await);
    harness.advance(TimeDelta::minutes(2));
    assert_eq!(
        handles(&list(&harness, &mac).await).len(),
        2,
        "curl idled out"
    );
}

#[tokio::test]
async fn signs_out_one_other_session_or_every_other_one() {
    let harness = Harness::new().await;
    let mac = token_browser(&harness, Some(FIREFOX_MAC)).await;
    let phone = token_browser(&harness, Some(SAFARI_IPHONE)).await;
    let third = token_browser(&harness, None).await;
    let alice = discord_browser(&harness, 111, "Alice", FIREFOX_MAC).await;
    let rows = handles(&list(&harness, &mac).await);
    let own = rows.iter().find(|row| row.1).unwrap().0.clone();
    let phone_handle = rows
        .iter()
        .find(|row| row.2.as_deref() == Some("Safari · iPhone"))
        .unwrap()
        .0
        .clone();

    let path = format!("{LIST}/{phone_handle}");
    let refused = unsafe_call(&harness, &mac, "DELETE", &path, false).await;
    assert_eq!(
        (refused.status, refused.api_error()),
        (403, "csrf".into()),
        "CSRF as on every unsafe admin route"
    );
    assert!(alive(&harness, &phone).await);

    // Alice cannot reach the token's sessions by handle.
    let foreign = unsafe_call(&harness, &alice, "DELETE", &path, true).await;
    assert_eq!(foreign.status, 404);
    assert!(alive(&harness, &phone).await);

    let ended = unsafe_call(&harness, &mac, "DELETE", &path, true).await;
    assert_eq!(ended.status, 204, "{}", ended.text());
    assert!(!alive(&harness, &phone).await);
    let again = unsafe_call(&harness, &mac, "DELETE", &path, true).await;
    assert_eq!((again.status, again.api_error()), (404, "not_found".into()));
    assert_valid("error.json#/$defs/ApiError", &path, &again.json());

    let current = unsafe_call(&harness, &mac, "DELETE", &format!("{LIST}/{own}"), true).await;
    assert_eq!(
        (current.status, current.api_error()),
        (409, "current_session".into())
    );
    assert!(alive(&harness, &mac).await);

    let everywhere = unsafe_call(&harness, &mac, "POST", OTHERS, true).await;
    assert_eq!(everywhere.status, 200, "{}", everywhere.text());
    let body = everywhere.json();
    assert_valid("identity.json#/$defs/SessionsEnded", OTHERS, &body);
    assert_eq!(body["ended"], 1);
    assert!(alive(&harness, &mac).await, "this one stays");
    assert!(!alive(&harness, &third).await);
    assert!(
        alive(&harness, &alice).await,
        "another identity is untouched"
    );

    let audited = harness
        .audit
        .events()
        .into_iter()
        .filter(|event| {
            matches!(event, AuditEvent::SessionEnded { reason, actor } if *reason == "logout_other" && actor == "token")
        })
        .count();
    assert_eq!(audited, 2, "each ended session is audited");

    let none = unsafe_call(&harness, &mac, "POST", OTHERS, true).await;
    assert_eq!(none.json()["ended"], 0);
}

#[tokio::test]
async fn anonymous_and_public_callers_get_nothing() {
    let harness = Harness::new().await;
    assert_eq!(harness.get(LIST, &[]).await.status, 401);
    let reply = crate::support::request(
        harness.public,
        "GET",
        crate::support::PUBLIC_HOST,
        LIST,
        &[],
    )
    .await;
    assert_eq!(reply.status, 404, "never mounted on the public listener");
}
