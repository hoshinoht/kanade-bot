//! `identity.json#/$defs/Me`: the signed-in admin as the guild sees them.

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
}
