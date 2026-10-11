//! `kanade portal open|close|status` and `kanade sessions list|revoke
//! HANDLE|sign-out-all`: online commands against the running bot's admin
//! API (no store access), signed in with the break-glass token for one run
//! (`client.rs`). `--url http://127.0.0.1:PORT` overrides
//! `KANADE_ADMIN_BIND`. Sessions are the token's own: the API lists and
//! ends only the caller's identity.

mod client;

use std::collections::BTreeMap;

use hyper::Method;
use serde_json::{Value, json};

use crate::runtime::{config::AdminClientConfig, error::Error};

pub const USAGE: &str = "portal open|close|status [--url http://127.0.0.1:8080]|sessions list|revoke HANDLE|sign-out-all [--url http://127.0.0.1:8080]";

/// A session handle: what `GET /api/admin/me/sessions` names rows by.
const HANDLE_HEX: usize = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    PortalStatus,
    PortalOpen,
    PortalClose,
    SessionsList,
    SessionsRevoke(String),
    SessionsSignOutAll,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub action: Action,
    pub url: Option<String>,
}

fn usage() -> Error {
    Error::Usage(format!("usage: kanade {USAGE}"))
}

/// `[--url URL]` after the action's own words.
fn url(rest: &[String]) -> Result<Option<String>, Error> {
    match rest {
        [] => Ok(None),
        [flag, url] if flag == "--url" && !url.is_empty() => Ok(Some(url.clone())),
        _ => Err(usage()),
    }
}

pub fn parse_portal(arguments: &[String]) -> Result<Args, Error> {
    let (action, rest) = arguments.split_first().ok_or_else(usage)?;
    let action = match action.as_str() {
        "status" => Action::PortalStatus,
        "open" => Action::PortalOpen,
        "close" => Action::PortalClose,
        _ => return Err(usage()),
    };
    Ok(Args {
        action,
        url: url(rest)?,
    })
}

pub fn parse_sessions(arguments: &[String]) -> Result<Args, Error> {
    let (action, rest) = arguments.split_first().ok_or_else(usage)?;
    let (action, rest) = match (action.as_str(), rest) {
        ("list", rest) => (Action::SessionsList, rest),
        ("sign-out-all", rest) => (Action::SessionsSignOutAll, rest),
        ("revoke", [handle, rest @ ..]) => {
            if handle.len() != HANDLE_HEX
                || !handle
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(Error::Usage(format!(
                    "sessions revoke: HANDLE is the {HANDLE_HEX}-character handle `sessions list` prints"
                )));
            }
            (Action::SessionsRevoke(handle.clone()), rest)
        }
        _ => return Err(usage()),
    };
    Ok(Args {
        action,
        url: url(rest)?,
    })
}

/// Run one online command; results go to stdout.
///
/// # Errors
/// Configuration (78), the API unreachable or failing (69, nothing changed
/// unless a step already succeeded) or a refusal of the request (64).
pub async fn run(args: Args, environment: &BTreeMap<String, String>) -> Result<(), Error> {
    let config = AdminClientConfig::from_mapping(environment, args.url.as_deref())?;
    client::signed_in(&config, async |session| match &args.action {
        Action::PortalStatus => {
            let view = session
                .call(Method::GET, "/api/admin/config", None, "portal status")
                .await?;
            print_portal(portal_switch(&view, "portal status")?);
            Ok(())
        }
        Action::PortalOpen | Action::PortalClose => {
            let open = args.action == Action::PortalOpen;
            let what = if open { "portal open" } else { "portal close" };
            let view = session
                .call(
                    Method::PATCH,
                    "/api/admin/config",
                    Some(json!({"self_service": {"public_portal": open}})),
                    what,
                )
                .await?;
            let now = portal_switch(&view, what)?;
            if now != open {
                return Err(Error::Unavailable(format!(
                    "{what}: the saved switch did not change"
                )));
            }
            print_portal(now);
            Ok(())
        }
        Action::SessionsList => {
            let list = session
                .call(Method::GET, "/api/admin/me/sessions", None, "sessions list")
                .await?;
            print!("{}", sessions_table(&list)?);
            Ok(())
        }
        Action::SessionsRevoke(handle) => {
            session
                .call(
                    Method::DELETE,
                    &format!("/api/admin/me/sessions/{handle}"),
                    None,
                    "sessions revoke",
                )
                .await?;
            println!("session {handle} signed out");
            Ok(())
        }
        Action::SessionsSignOutAll => {
            let ended = session
                .call(
                    Method::POST,
                    "/api/admin/me/sessions/sign-out-others",
                    None,
                    "sessions sign-out-all",
                )
                .await?;
            let ended = ended["ended"].as_u64().ok_or_else(|| {
                Error::Unavailable("sessions sign-out-all: unexpected answer".into())
            })?;
            // This command's own session ends with the sign-out after it.
            println!("signed out {ended} other session(s) and this command's own");
            Ok(())
        }
    })
    .await
}

fn portal_switch(view: &Value, what: &str) -> Result<bool, Error> {
    view.pointer("/self_service/public_portal")
        .and_then(Value::as_bool)
        .ok_or_else(|| Error::Unavailable(format!("{what}: unexpected Config answer")))
}

fn print_portal(open: bool) {
    println!(
        "public portal switch: {}",
        if open { "open" } else { "closed" }
    );
}

/// One line per session, this command's own marked; ids never appear.
fn sessions_table(list: &Value) -> Result<String, Error> {
    let rows = list["sessions"]
        .as_array()
        .ok_or_else(|| Error::Unavailable("sessions list: unexpected answer".into()))?;
    let mut out = format!("{} live session(s) of the break-glass token:\n", rows.len());
    for row in rows {
        let text = |key: &str| row[key].as_str().unwrap_or("-").to_owned();
        out.push_str(&format!(
            "{}  {}  {}  signed in {}  last seen {}{}\n",
            text("handle"),
            text("method"),
            text("device"),
            text("signed_in_at"),
            text("last_seen_at"),
            if row["current"].as_bool() == Some(true) {
                "  (this command)"
            } else {
                ""
            }
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Result<Args, Error> {
        let words: Vec<String> = text.split(' ').map(str::to_owned).collect();
        match words[0].as_str() {
            "portal" => parse_portal(&words[1..]),
            _ => parse_sessions(&words[1..]),
        }
    }

    #[test]
    fn actions_take_an_optional_url() {
        assert_eq!(
            args("portal open").unwrap(),
            Args {
                action: Action::PortalOpen,
                url: None
            }
        );
        assert_eq!(
            args("sessions revoke 0123456789abcdef01234567 --url http://127.0.0.1:9").unwrap(),
            Args {
                action: Action::SessionsRevoke("0123456789abcdef01234567".into()),
                url: Some("http://127.0.0.1:9".into())
            }
        );
        assert_eq!(
            args("sessions sign-out-all").unwrap().action,
            Action::SessionsSignOutAll
        );
        for bad in [
            "portal",
            "portal toggle",
            "portal open --url",
            "portal open extra",
            "sessions",
            "sessions revoke",
            "sessions revoke ../../config",
            "sessions revoke 0123456789ABCDEF01234567",
            "sessions list --url http://127.0.0.1:9 extra",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_table_marks_this_command_and_shows_no_ids() {
        let table = sessions_table(&json!({"sessions": [
            {"handle": "aaa", "method": "token", "device": null, "signed_in_at": "2026-10-11T00:00:00Z", "last_seen_at": "2026-10-11T00:01:00Z", "current": true},
            {"handle": "bbb", "method": "token", "device": "Firefox · macOS", "signed_in_at": "2026-10-10T00:00:00Z", "last_seen_at": "2026-10-10T01:00:00Z", "current": false}
        ], "generated_at": "2026-10-11T00:01:00Z"}))
        .unwrap();
        assert_eq!(
            table,
            "2 live session(s) of the break-glass token:\n\
             aaa  token  -  signed in 2026-10-11T00:00:00Z  last seen 2026-10-11T00:01:00Z  (this command)\n\
             bbb  token  Firefox · macOS  signed in 2026-10-10T00:00:00Z  last seen 2026-10-10T01:00:00Z\n"
        );
    }
}
