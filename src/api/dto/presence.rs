//! `presence.json`: who else has an admin page open
//! (`/api/admin/presence`). Held in memory only and never a record of
//! anything; the caller is never listed.

use serde::{Deserialize, Serialize};

use super::Named;

/// The kind of page a place is: an Inbox member request or a run sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PresenceKind"))]
#[serde(rename_all = "snake_case")]
pub enum PlaceKind {
    Request,
    Run,
}

impl PlaceKind {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "request" => Some(Self::Request),
            "run" => Some(Self::Run),
            _ => None,
        }
    }
}

/// What the admin is doing there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PresenceMode"))]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Viewing,
    Editing,
}

/// A place as a check-in names it.
#[derive(Clone, Debug, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PresencePlace"))]
#[serde(deny_unknown_fields)]
pub struct PlaceBody {
    pub kind: PlaceKind,
    pub id: String,
}

/// `PUT /api/admin/presence` body. `seq` orders one tab's calls: it grows
/// with every check-in and leave, and a check-in no newer than the tab's
/// leave (`DELETE …&seq=`) is ignored, so a late one cannot bring it back.
#[derive(Clone, Debug, Deserialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "PresenceCheckIn"))]
#[serde(deny_unknown_fields)]
pub struct CheckIn {
    pub at: PlaceBody,
    pub mode: Mode,
    pub seq: u64,
}

/// Another admin at the place: `seen_s` is the seconds since their latest
/// check-in there (an admin with several tabs is listed once, editing if
/// any tab is).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PresenceEntry {
    pub admin: Named,
    pub mode: Mode,
    pub seen_s: u64,
}

/// `PUT /api/admin/presence`: who else is there, and the cadence the client
/// keeps (check in every `heartbeat_s`; an entry is gone `ttl_s` after its
/// last check-in).
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PresenceHere {
    pub heartbeat_s: u64,
    pub ttl_s: u64,
    pub here: Vec<PresenceEntry>,
}

/// `GET /api/admin/presence?at=<kind>:<id>`: who else is there, read without
/// checking in.
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PresenceList {
    pub here: Vec<PresenceEntry>,
}
