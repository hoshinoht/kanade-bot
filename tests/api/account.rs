//! A02 Account: `GET /api/admin/me` keeps token and Tailscale sessions
//! neutral, and gives a Discord session its access, named roles and the same
//! allowance row Limits shows. Admin-only: the public listener never mounts it.

use kanade::domain::members::MemberStore;
use serde_json::{Value, json};

use crate::{
    reads::{EDGE_HEADERS, Reads},
    schemas::assert_valid,
    support::{ADMIN_HOST, Fixture, PUBLIC_HOST, public, request},
};

const ME: &str = "/api/admin/me";
const SCHEMA: &str = "identity.json#/$defs/Me";
const TOKEN: &str = "break-glass-token-with-at-least-32-bytes!";

async fn me(reads: &Reads, headers: &[(&str, &str)]) -> Value {
    let reply = request(reads.admin, "GET", ADMIN_HOST, ME, headers).await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert!(!reply.dump().contains(TOKEN), "no secret in {ME}");
    let value = reply.json();
    assert_valid(SCHEMA, ME, &value);
    value
}

/// Cara is staff through the admin role `20`, which the directory cannot name.
async fn cara_with_roles(reads: &Reads) {
    let mut cara = reads.store.load_member("1003").await.unwrap().unwrap();
    cara.roles = vec!["701".into(), "20".into(), "700".into()];
    reads.store.put_member(cara).await.unwrap();
}

#[tokio::test]
async fn token_sessions_are_neutral_and_anonymous_callers_are_refused() {
    let reads = Reads::new().await;
    let anonymous = request(reads.admin, "GET", ADMIN_HOST, ME, &[]).await;
    assert_eq!(anonymous.status, 401);
    assert_valid("error.json#/$defs/ApiError", ME, &anonymous.json());

    let value = me(&reads, &[("Cookie", &reads.cookie)]).await;
    assert_eq!(value["method"], "token");
    assert_eq!(value["member"], Value::Null);
}

#[tokio::test]
async fn tailscale_sessions_are_neutral() {
    let reads = Reads::with_logins().await;
    let (cookie, _) = reads.tailscale_session().await;
    let mut headers = vec![("Cookie", cookie.as_str())];
    headers.extend_from_slice(&EDGE_HEADERS);
    let value = me(&reads, &headers).await;
    assert_eq!(value["method"], "tailscale");
    assert_eq!(value["member"], Value::Null);
}

#[tokio::test]
async fn discord_sessions_show_named_roles_access_and_the_limits_row() {
    let reads = Reads::with_logins().await;
    cara_with_roles(&reads).await;
    let (cookie, _) = reads.discord_session(1003, "Cara").await;
    let value = me(&reads, &[("Cookie", &cookie)]).await;
    assert_eq!(value["method"], "discord");
    let member = &value["member"];
    assert_eq!(member["id"], "1003");
    assert_eq!(member["name"], "Cara");
    assert_eq!(member["access"], "staff");
    assert_eq!(member["bossing"], false);
    // Guild order (highest first), names only: the unnamed admin role is left out.
    assert_eq!(
        member["roles"],
        json!([
            {"id": "700", "name": "Officer", "color": "#0a0bff"},
            {"id": "701", "name": "Bossing"},
        ])
    );

    let limits = reads
        .read("/api/admin/limits", "limits.json#/$defs/Limits")
        .await;
    let row = limits["allowances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["member"]["id"] == "1003")
        .unwrap();
    assert_eq!(&member["allowance"], row, "the Limits row, unchanged");
    assert_eq!(member["allowance"]["staff"], true);
}

#[tokio::test]
async fn an_unavailable_role_directory_is_null_not_an_empty_list() {
    let reads = Reads::with_logins_role_directory_connected(false).await;
    cara_with_roles(&reads).await;
    let (cookie, _) = reads.discord_session(1003, "Cara").await;
    let value = me(&reads, &[("Cookie", &cookie)]).await;
    assert_eq!(value["member"]["roles"], Value::Null);
    assert_eq!(value["member"]["access"], "staff");
}

#[tokio::test]
async fn me_is_not_mounted_on_the_public_listener() {
    let fixture = Fixture::new();
    let address = public(&fixture.http()).await;
    let reply = request(address, "GET", PUBLIC_HOST, ME, &[]).await;
    assert_eq!(reply.status, 404, "{}", reply.text());
    assert_valid("error.json#/$defs/ApiError", ME, &reply.json());
}
