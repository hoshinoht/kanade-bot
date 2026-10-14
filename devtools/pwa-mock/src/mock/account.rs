//! Account (`/api/admin/me` extras and `/api/admin/me/sessions`) as the
//! server answers it: the reply style resolved like a chat turn (the first
//! held role assignment beats the saved choice) and the signed-in identity's
//! own live sessions. The sessions are seeded per sign-in method and sit
//! inside the server's policy (idle 60 min, absolute 12 h).

use super::clock::iso_z;
use super::config::{profile_label_voice, profile_visible};
use super::{MoveError, Store};
use serde_json::{Value, json};

/// (handle, device, signed in minutes ago, last seen minutes ago, current).
type Seeded = (&'static str, Option<&'static str>, i64, i64, bool);

const DISCORD: [Seeded; 3] = [
    (
        "a1c4e9f0b2d3a1c4e9f0b2d3",
        Some("Chrome · macOS"),
        150,
        0,
        true,
    ),
    (
        "5e7a0c2b9d4f5e7a0c2b9d4f",
        Some("Safari · iPhone"),
        480,
        40,
        false,
    ),
    (
        "c0ffee00b7e1c0ffee00b7e1",
        Some("Firefox · Windows"),
        95,
        12,
        false,
    ),
];
const TOKEN: [Seeded; 2] = [
    (
        "7b2d4f6a8c0e7b2d4f6a8c0e",
        Some("Chrome · macOS"),
        20,
        0,
        true,
    ),
    ("e3f5a7c9b1d3e3f5a7c9b1d3", None, 300, 25, false),
];
const TAILSCALE: [Seeded; 1] = [(
    "9a8b7c6d5e4f9a8b7c6d5e4f",
    Some("Chrome · macOS"),
    45,
    0,
    true,
)];

/// The server build the mock stands in for (root `Cargo.toml`).
pub fn server_version() -> &'static str {
    include_str!("../../../../Cargo.toml")
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or("0.0.0")
}

impl Store {
    fn seeded_sessions(&self) -> impl Iterator<Item = &'static Seeded> + '_ {
        let all: &'static [Seeded] = match self.session_method() {
            "token" => &TOKEN,
            "tailscale" => &TAILSCALE,
            _ => &DISCORD,
        };
        all.iter()
            .filter(|row| !self.ended_sessions.contains(&row.0))
    }

    pub fn own_sessions(&self) -> Value {
        let now = Self::now_minute();
        let sessions: Vec<Value> = self
            .seeded_sessions()
            .map(|(handle, device, signed, seen, current)| {
                json!({
                    "handle": handle, "method": self.session_method(), "device": device,
                    "signed_in_at": iso_z(now - signed), "last_seen_at": iso_z(now - seen),
                    "current": current,
                })
            })
            .collect();
        json!({ "sessions": sessions, "generated_at": super::clock::iso_now() })
    }

    pub fn end_own_session(&mut self, handle: &str) -> Result<(), MoveError> {
        let row = self
            .seeded_sessions()
            .find(|row| row.0 == handle)
            .ok_or(MoveError::Coded(
                404,
                "not_found",
                "That session has already ended.".into(),
            ))?;
        if row.4 {
            return Err(MoveError::Coded(
                409,
                "current_session",
                "This is the session you are using; sign out instead.".into(),
            ));
        }
        self.ended_sessions.push(row.0);
        Ok(())
    }

    pub fn end_other_sessions(&mut self) -> Value {
        let others: Vec<&'static str> = self
            .seeded_sessions()
            .filter(|row| !row.4)
            .map(|row| row.0)
            .collect();
        let ended = others.len();
        self.ended_sessions.extend(others);
        json!({ "ended": ended })
    }

    fn style_ref(&self, key: &str) -> Value {
        let name = profile_label_voice(key).map_or(key, |(name, _)| name);
        json!({ "key": key, "name": name, "public": profile_visible(&self.config, key) })
    }

    /// `held`: the member's role ids; `saved`: their saved profile key.
    pub fn reply_style(&self, held: &[&str], saved: Option<&str>) -> Value {
        let winner = self.config.role_profiles.iter().find(|assignment| {
            held.contains(&assignment.role_id.as_str())
                && profile_label_voice(&assignment.profile).is_some()
        });
        let saved_ok = saved
            .filter(|key| profile_label_voice(key).is_some() && profile_visible(&self.config, key));
        let (in_effect, source, role_name) = match (winner, saved_ok) {
            (Some(assignment), _) => (
                self.style_ref(&assignment.profile),
                "role",
                Self::roles()
                    .as_array()
                    .and_then(|roles| {
                        roles
                            .iter()
                            .find(|r| r["id"] == assignment.role_id.as_str())
                    })
                    .map_or(Value::Null, |role| role["name"].clone()),
            ),
            (None, Some(key)) => (self.style_ref(key), "saved", Value::Null),
            (None, None) => (Value::Null, "default", Value::Null),
        };
        json!({
            "in_effect": in_effect, "source": source, "role_name": role_name,
            "saved": saved.map_or(Value::Null, |key| self.style_ref(key)),
        })
    }
}
