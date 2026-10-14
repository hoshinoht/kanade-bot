//! `identity.json#/$defs/Me`: the signed-in admin as the guild sees them,
//! plus their own sessions (`AccountSessions`).

use serde::Serialize;

use super::{RoleRow, limits::Allowance};

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Me {
    pub display: String,
    #[cfg_attr(test, ts(type = "SignInMethod"))]
    pub method: &'static str,
    /// The guild member behind a Discord sign-in; null for the admin token,
    /// Tailscale, and a Discord account with no member row.
    pub member: Option<MeMember>,
    /// The server's clock when this was read (ISO-8601 UTC).
    pub server_time: String,
    /// The server build (`Cargo.toml` version).
    pub version: &'static str,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MeMember {
    pub id: String,
    pub name: String,
    #[cfg_attr(test, ts(type = "'staff' | 'pilot' | 'none'"))]
    pub access: &'static str,
    /// Holds the bossing role (on the roster).
    pub bossing: bool,
    /// Current guild roles, highest first; ids the directory cannot name are
    /// left out. Null while the role directory is unavailable.
    pub roles: Option<Vec<RoleRow>>,
    /// The member's row exactly as Limits shows it; null without chatbot access.
    pub allowance: Option<Allowance>,
    /// How chat answers this member; null while no persona is loaded.
    pub reply_style: Option<ReplyStyle>,
}

/// The reply profile in effect and the member's own saved choice. A role
/// assignment (first match in Config → Persona) beats the saved choice.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReplyStyle {
    /// What chat uses now; null is the persona's default voice.
    pub in_effect: Option<ReplyStyleRef>,
    #[cfg_attr(test, ts(type = "'role' | 'saved' | 'default'"))]
    pub source: &'static str,
    /// The role whose assignment wins (source `role`), when the role
    /// directory can name it.
    pub role_name: Option<String>,
    /// The member's saved choice; null is the default voice.
    pub saved: Option<ReplyStyleRef>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReplyStyleRef {
    pub key: String,
    /// The profile's label; the key when its file is no longer readable.
    pub name: String,
    /// Members may choose it (Config → Persona visibility).
    pub public: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct AccountSessions {
    pub sessions: Vec<AccountSession>,
    /// The server's clock when this was read (ISO-8601 UTC).
    pub generated_at: String,
}

/// One live session of the caller's identity (same sign-in method and
/// subject). `handle` names it for sign-out; ids and hashes never leave.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct AccountSession {
    pub handle: String,
    #[cfg_attr(test, ts(type = "SignInMethod"))]
    pub method: &'static str,
    /// "Firefox · macOS"; null when the browser was not recognised.
    pub device: Option<String>,
    pub signed_in_at: String,
    pub last_seen_at: String,
    /// The session this request came with.
    pub current: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct SessionsEnded {
    pub ended: u64,
}
