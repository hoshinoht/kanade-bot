//! Discord's authorization page for both sign-in realms, at
//! `/__e2e/discord/oauth2/authorize` on the harness origins. It approves at
//! once for the user the cookie [`USER_COOKIE`] names on `127.0.0.1` (cookies
//! ignore ports): `eligible` (default; "Mikan"), `partner` ("Yuzu", on
//! Mikan's weekly timing), `ineligible` (no bossing role), `bot` (a bot
//! account) or `deny` (cancelled at Discord). The code exchange and the user
//! read then go to the shared [`FakeDiscord`].

use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, Uri, header::COOKIE},
    response::{IntoResponse, Response},
    routing::get,
};
use kanade::api::auth::{
    crypto,
    discord::DiscordUser,
    fake::FakeDiscord,
    wire::{self, query_pairs, query_value},
};

pub const PATH: &str = "/__e2e/discord/oauth2/authorize";
pub const USER_COOKIE: &str = "kanade_fake_discord";
const ELIGIBLE: &str = "100000000000000001";
const INELIGIBLE: &str = "100000000000000002";
const BOT: &str = "100000000000000003";
const PARTNER: &str = "100000000000000004";

/// The users that are guild members, and whether each has the bossing role.
pub const MEMBERS: [(&str, bool); 3] =
    [("eligible", true), ("partner", true), ("ineligible", false)];

pub fn user(choice: &str) -> DiscordUser {
    let (id, name, bot) = match choice {
        "ineligible" => (INELIGIBLE, "Plain", false),
        "partner" => (PARTNER, "Yuzu", false),
        "bot" => (BOT, "Botty", true),
        _ => (ELIGIBLE, "Mikan", false),
    };
    DiscordUser {
        id: id.into(),
        username: name.to_lowercase(),
        global_name: Some(name.into()),
        bot,
        avatar: None,
    }
}

pub struct Page {
    pub discord: Arc<FakeDiscord>,
    /// Each realm's configured redirect URI, and where the browser goes back
    /// to: the harness listeners are plain HTTP, so the public realm's
    /// `https://` URI (the only kind its config accepts) returns over `http://`.
    pub redirects: Vec<(String, String)>,
}

pub fn routes<S>(page: Arc<Page>) -> Router<S> {
    Router::new().route(PATH, get(authorize)).with_state(page)
}

async fn authorize(State(page): State<Arc<Page>>, headers: HeaderMap, uri: Uri) -> Response {
    let pairs = query_pairs(uri.query());
    let value = |key| query_value(&pairs, key);
    let (Some(state), Some(challenge), Some(redirect)) = (
        value("state"),
        value("code_challenge"),
        value("redirect_uri"),
    ) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let back = page
        .redirects
        .iter()
        .find(|(configured, _)| *configured == redirect);
    let Some((_, back)) = back.filter(|_| value("scope").as_deref() == Some("identify")) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    // Not `wire::cookie`: that one reads only session-token-shaped values.
    let choice = headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == USER_COOKIE)
        .map_or("eligible", |(_, value)| value);
    let query = match choice {
        "deny" => wire::form(&[("error", "access_denied"), ("state", &state)]),
        choice => {
            let Some(code) = crypto::random_token() else {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            };
            page.discord.approve(&code, &challenge, user(choice));
            wire::form(&[("code", &code), ("state", &state)])
        }
    };
    wire::see_other(&format!("{back}?{query}"), [])
}
