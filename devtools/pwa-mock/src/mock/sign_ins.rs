//! History › Sign-ins (`GET /api/admin/history/sign-ins`): a fixed, invented
//! audit log of both realms with the server's filters and refusals.

use serde_json::{Value, json};

use super::rewrites::filters;
use super::{MoveError, Store};

const KEYS: [&str; 6] = ["realm", "event", "actor", "from", "to", "before"];
const REALMS: [&str; 2] = ["admin", "member"];
const EVENTS: [&str; 7] = [
    "login_succeeded",
    "login_refused",
    "break_glass_used",
    "session_ended",
    "session_rotated",
    "rate_limited",
    "revoke_failed",
];

struct Row {
    hour: i64,
    realm: &'static str,
    event: &'static str,
    actor: Option<&'static str>,
    name: Option<&'static str>,
    method: Option<&'static str>,
    reason: Option<&'static str>,
    request: Option<&'static str>,
    client: Option<&'static str>,
    device: Option<&'static str>,
}

const TAG: &str = "5f2c9a1d7e3b40c6a8d1f0b2e4c6a8d0f1b3c5e7a9d2f4b6c8e0a1d3f5b7c9e2";

fn row(hour: i64, realm: &'static str, event: &'static str) -> Row {
    Row {
        hour,
        realm,
        event,
        actor: None,
        name: None,
        method: None,
        reason: None,
        request: None,
        client: None,
        device: None,
    }
}

/// Newest last; `seq` is the position.
fn rows() -> Vec<Row> {
    vec![
        Row {
            actor: Some("token"),
            name: Some("Admin token"),
            method: Some("login"),
            request: Some("POST /api/admin/auth/token"),
            client: Some("100.64.0.9"),
            ..row(2, "admin", "break_glass_used")
        },
        Row {
            method: Some("token"),
            reason: Some("bad_token"),
            client: Some("100.64.0.9"),
            ..row(3, "admin", "login_refused")
        },
        Row {
            actor: Some("discord:2001"),
            name: Some("Hoshino"),
            method: Some("discord"),
            client: Some("100.64.0.7"),
            device: Some("Firefox on macOS"),
            ..row(5, "admin", "login_succeeded")
        },
        Row {
            actor: Some("discord:2002"),
            name: Some("Mikan"),
            method: Some("discord"),
            client: Some(TAG),
            device: Some("Safari on iOS"),
            ..row(7, "member", "login_succeeded")
        },
        Row {
            actor: Some("3999"),
            name: Some("Discord 3999"),
            method: Some("discord"),
            reason: Some("not_eligible"),
            client: Some(TAG),
            ..row(8, "member", "login_refused")
        },
        Row {
            reason: Some("discord_start"),
            client: Some(TAG),
            ..row(9, "member", "rate_limited")
        },
        Row {
            actor: Some("discord:2002"),
            name: Some("Mikan"),
            client: Some(TAG),
            ..row(10, "member", "session_rotated")
        },
        Row {
            actor: Some("discord:2001"),
            name: Some("Hoshino"),
            reason: Some("logout"),
            client: Some("100.64.0.7"),
            ..row(11, "admin", "session_ended")
        },
    ]
}

impl Store {
    pub fn sign_ins(&self, raw: Option<&str>) -> Result<Value, MoveError> {
        let query = filters(raw, &KEYS)?;
        let get = |key: &str| {
            query
                .get(key)
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
        };
        let invalid = |message: String| MoveError::Coded(422, "invalid_filter", message);
        for (key, allowed) in [("realm", &REALMS[..]), ("event", &EVENTS[..])] {
            if let Some(value) = get(key)
                && !allowed.contains(&value.as_str())
            {
                return Err(invalid(format!("Unknown {key} “{value}”.")));
            }
        }
        for key in ["from", "to"] {
            if let Some(day) = get(key)
                && !super::clock::valid_date(&day)
            {
                return Err(invalid(format!("Dates are YYYY-MM-DD, not “{day}”.")));
            }
        }
        let before = match get("before") {
            None => None,
            Some(text) => Some(
                text.parse::<i64>()
                    .ok()
                    .filter(|seq| *seq > 0 && text.bytes().all(|b| b.is_ascii_digit()))
                    .ok_or_else(|| invalid(format!("“before” is a row number, not “{text}”.")))?,
            ),
        };
        let (realm, event, actor) = (get("realm"), get("event"), get("actor"));
        let rows: Vec<Value> = rows()
            .iter()
            .enumerate()
            .rev()
            .map(|(index, row)| (index as i64 + 1, row))
            .filter(|(seq, row)| {
                realm.as_deref().is_none_or(|r| row.realm == r)
                    && event.as_deref().is_none_or(|e| row.event == e)
                    && actor.as_deref().is_none_or(|a| row.actor == Some(a))
                    && before.is_none_or(|b| *seq < b)
            })
            .map(|(seq, row)| {
                json!({
                    "seq": seq,
                    "at": super::clock::iso_z(Store::hour_minute(row.hour)),
                    "realm": row.realm,
                    "event": row.event,
                    "actor": row.actor,
                    "name": row.name,
                    "method": row.method,
                    "reason": row.reason,
                    "request": row.request,
                    "client": row.client,
                    "device": row.device,
                    "request_id": format!("mock-{seq}"),
                })
            })
            .collect();
        Ok(json!({ "rows": rows, "next_before": null }))
    }
}
