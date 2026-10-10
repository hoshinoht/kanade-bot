//! History › Sign-ins: the stored security events of both realms
//! (`GET /api/admin/history/sign-ins`). Admin rows carry the client address,
//! member rows only its keyed tag; never a token, code or cookie.

use serde::Serialize;

use super::iso_instant;
use crate::infrastructure::store::auth_audit::AuditRow;

/// One audited event, newest first.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SignInRow {
    pub seq: i64,
    pub at: String,
    #[cfg_attr(test, ts(type = "SignInRealm"))]
    pub realm: &'static str,
    #[cfg_attr(test, ts(type = "SignInEvent"))]
    pub event: &'static str,
    /// `discord:<id>`, `tailscale:<login>`, `token`, or a refused user's id.
    pub actor: Option<String>,
    /// The roster name for a Discord actor, else a readable label; `null`
    /// when the row names nobody (a rate limit).
    pub name: Option<String>,
    pub method: Option<String>,
    pub reason: Option<String>,
    pub request: Option<String>,
    pub client: Option<String>,
    pub device: Option<String>,
    pub request_id: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SignInPage {
    pub rows: Vec<SignInRow>,
    /// The `before` for the next (older) page; `null` on the last.
    pub next_before: Option<i64>,
}

/// `name_of` resolves a Discord user id to its roster name.
pub fn row(row: &AuditRow, name_of: impl Fn(&str) -> Option<String>) -> SignInRow {
    let name = row
        .actor
        .as_deref()
        .map(|actor| match actor.split_once(':') {
            Some(("discord", id)) => name_of(id).unwrap_or_else(|| format!("Discord {id}")),
            Some(("tailscale", login)) => login.to_owned(),
            None if actor == "token" => "Admin token".to_owned(),
            // A refused sign-in names the bare Discord id it saw.
            None if actor.bytes().all(|byte| byte.is_ascii_digit()) => {
                name_of(actor).unwrap_or_else(|| format!("Discord {actor}"))
            }
            _ => actor.to_owned(),
        });
    SignInRow {
        seq: row.seq,
        at: iso_instant(row.at),
        realm: row.realm.as_str(),
        event: row.event.as_str(),
        actor: row.actor.clone(),
        name,
        method: row.method.clone(),
        reason: row.reason.clone(),
        request: row.request.clone(),
        client: row.client.clone(),
        device: row.device.clone(),
        request_id: row.request_id.clone(),
    }
}
