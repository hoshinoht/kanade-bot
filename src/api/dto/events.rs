//! `GET /api/admin/events` and `GET /api/public/events` frames: change hints
//! without data. A page that cares re-reads its own endpoint; nothing here
//! names a row, a value or (on the member stream) another member.

use serde::Serialize;

use crate::infrastructure::store::Written;

/// What kind of data changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "EventTopic"))]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// The history head moved (portal, Discord, reactions, applied proposals).
    Schedule,
    /// Proposals or member requests changed.
    Inbox,
    /// A chat interaction was logged.
    Chat,
    /// An extraction call was logged.
    Extraction,
    /// A rewrite attempt was logged.
    Rewrite,
    /// The delivery tick posted a card.
    Delivery,
    /// A Config section was saved.
    Settings,
    /// A re-read job moved on.
    Rescan,
    /// A member row changed (roster sync, departures, portal edits).
    Members,
}

impl From<Written> for Topic {
    fn from(written: Written) -> Self {
        match written {
            Written::Schedule => Self::Schedule,
            Written::Inbox => Self::Inbox,
            Written::Chat => Self::Chat,
            Written::Extraction => Self::Extraction,
            Written::Rewrite => Self::Rewrite,
            Written::Delivery => Self::Delivery,
            Written::Settings => Self::Settings,
            Written::Rescan => Self::Rescan,
            Written::Members => Self::Members,
        }
    }
}

/// The default (`message`) event: one hint. `seq` counts hints since the
/// process started, so a gap tells the client it missed some.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EventHint {
    pub topic: Topic,
    pub seq: u64,
}

/// The `ready` event that opens every stream: the last hint's `seq` (0 before
/// any), so a reconnecting client knows whether it missed something, and the
/// server process's `boot` id: a new one means a restart (perhaps after a
/// restore), when versions may go down and the client takes what it reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct EventReady {
    pub seq: u64,
    pub boot: String,
}

/// What changed for the signed-in member (`GET /api/public/events`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "MemberEventTopic"))]
#[serde(rename_all = "snake_case")]
pub enum MemberTopic {
    /// The history head moved: re-read the week.
    Schedule,
    /// The member's own runs, answers, requests or ownership asks changed.
    Mine,
    /// The member's chat allowance may have changed.
    Allowance,
}

/// The member stream's default (`message`) event: one hint, topic only. No
/// `seq`: it would count every write, other members' and admins' included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "MemberEventHint"))]
pub struct MemberHint {
    pub topic: MemberTopic,
}

/// The `ready` event that opens every member stream: the server process's
/// `boot` id only. Hints sent while a stream was down are not replayed, so
/// the client re-reads after every `ready` but the first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "MemberEventReady"))]
pub struct MemberReady {
    pub boot: String,
}
