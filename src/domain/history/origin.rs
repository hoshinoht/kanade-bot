//! Who made a change and through which surface.

use chrono::{DateTime, Utc};

use super::precondition::Expect;
use super::record::ChangeRef;
use crate::domain::schedule::Notice;

/// Who a change is attributed to. Always derived server-side, never taken
/// from a client.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Actor {
    /// A guild member, by Discord user id.
    Member { id: String },
    /// An administrator, by id or operator label.
    Admin { id: String },
    /// Kanade itself, by component (e.g. `delivery`).
    System { component: String },
}

impl Actor {
    pub fn member(id: impl Into<String>) -> Self {
        Self::Member { id: id.into() }
    }

    pub fn admin(id: impl Into<String>) -> Self {
        Self::Admin { id: id.into() }
    }

    pub fn system(component: impl Into<String>) -> Self {
        Self::System {
            component: component.into(),
        }
    }

    /// `member`, `admin` or `system`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Member { .. } => "member",
            Self::Admin { .. } => "admin",
            Self::System { .. } => "system",
        }
    }

    /// The member/admin id or system component.
    pub fn id(&self) -> &str {
        match self {
            Self::Member { id } | Self::Admin { id } => id,
            Self::System { component } => component,
        }
    }

    pub fn from_parts(kind: &str, id: &str) -> Option<Self> {
        match kind {
            "member" => Some(Self::member(id)),
            "admin" => Some(Self::admin(id)),
            "system" => Some(Self::system(id)),
            _ => None,
        }
    }
}

/// The surface a change arrived through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Surface {
    Discord,
    AdminPortal,
    PublicPortal,
    Cli,
    ChatApproval,
    ExtractionApproval,
    DeliveryTick,
    Rollback,
    Import,
    /// An administrator merged a draft.
    DraftMerge,
    /// An approved member request was merged (S3).
    RequestMerge,
    /// A change cherry-picked into another boss week (S4).
    CherryPick,
}

impl Surface {
    pub const ALL: &[Self] = &[
        Self::Discord,
        Self::AdminPortal,
        Self::PublicPortal,
        Self::Cli,
        Self::ChatApproval,
        Self::ExtractionApproval,
        Self::DeliveryTick,
        Self::Rollback,
        Self::Import,
        Self::DraftMerge,
        Self::RequestMerge,
        Self::CherryPick,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discord => "discord",
            Self::AdminPortal => "admin_portal",
            Self::PublicPortal => "public_portal",
            Self::Cli => "cli",
            Self::ChatApproval => "chat_approval",
            Self::ExtractionApproval => "extraction_approval",
            Self::DeliveryTick => "delivery_tick",
            Self::Rollback => "rollback",
            Self::Import => "import",
            Self::DraftMerge => "draft_merge",
            Self::RequestMerge => "request_merge",
            Self::CherryPick => "cherry_pick",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|surface| surface.as_str() == value)
    }
}

/// Actor, surface and the caller's optional request/idempotency id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub actor: Actor,
    pub surface: Surface,
    pub request_id: Option<String>,
}

impl Origin {
    pub fn new(actor: Actor, surface: Surface) -> Self {
        Self {
            actor,
            surface,
            request_id: None,
        }
    }

    #[must_use]
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Origin {
    /// Test support: an administrator acting through the admin portal.
    pub fn for_tests() -> Self {
        Self::new(Actor::admin("test"), Surface::AdminPortal)
    }
}

/// What a store records alongside one commit; the store derives the rows'
/// before/after values itself, inside the commit's transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeMeta {
    pub origin: Origin,
    /// The operation's single clock reading.
    pub at: DateTime<Utc>,
    /// Effect kinds of the notices the operation emitted.
    pub notices: Vec<String>,
    /// Records this change refers to (a revert's undone records).
    pub refs: Vec<ChangeRef>,
    /// Digest of the request (operation and its arguments) when
    /// `origin.request_id` is set; a reused id must carry the same digest.
    /// Stored beside the record, not part of its canonical body.
    pub request_digest: Option<String>,
    /// Checked by the store inside the commit's transaction; never recorded
    /// (overrides are recorded as `refs`).
    pub expect: Expect,
    /// The notices to deliver for this change: written to the notice outbox
    /// in the commit's transaction, keyed by the record's seq and position.
    /// Not part of the record's canonical body (`notices` holds their kinds).
    pub outbox: Vec<Notice>,
}
