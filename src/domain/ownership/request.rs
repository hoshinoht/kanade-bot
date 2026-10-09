//! A stored ownership request (`owner_requests`, migration 0034) and its
//! store port: a party member asks to own a timing; its owner or staff
//! decide within [`OWNER_REQUEST_TTL`].

use chrono::{DateTime, TimeDelta, Utc};

use crate::domain::scheduler::StoreError;

/// How long a request stays open.
pub const OWNER_REQUEST_TTL: TimeDelta = TimeDelta::hours(24);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OwnerRequestStatus {
    Open,
    Accepted,
    Declined,
    Expired,
    Withdrawn,
    /// The timing's owner changed another way while it was open.
    Superseded,
}

impl OwnerRequestStatus {
    pub const ALL: &[Self] = &[
        Self::Open,
        Self::Accepted,
        Self::Declined,
        Self::Expired,
        Self::Withdrawn,
        Self::Superseded,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Expired => "expired",
            Self::Withdrawn => "withdrawn",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|status| status.as_str() == text)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerRequest {
    pub id: String,
    pub fixed_run_id: String,
    pub requester: String,
    /// Where its Accept/Decline message is (or will be) posted.
    pub channel_id: Option<String>,
    pub message_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: OwnerRequestStatus,
    /// `<actor kind>:<id>` of who closed it (`system:expiry` for the tick).
    pub decided_by: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    /// Its posted message now shows the decision (or is gone).
    pub message_settled: bool,
}

impl OwnerRequest {
    /// A new open request at `now`, expiring [`OWNER_REQUEST_TTL`] later.
    pub fn open(
        id: String,
        fixed_run_id: String,
        requester: String,
        channel_id: Option<String>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            fixed_run_id,
            requester,
            channel_id,
            message_id: None,
            created_at: now,
            expires_at: now + OWNER_REQUEST_TTL,
            status: OwnerRequestStatus::Open,
            decided_by: None,
            decided_at: None,
            message_settled: false,
        }
    }

    /// Open and not yet past its expiry at `now`.
    pub fn live(&self, now: DateTime<Utc>) -> bool {
        self.status == OwnerRequestStatus::Open && now < self.expires_at
    }

    /// The shape both stores enforce (SQLite by CHECK as well).
    ///
    /// # Errors
    /// [`StoreError::Constraint`] naming the first bad field.
    pub fn check(&self) -> Result<(), StoreError> {
        let refuse = |what: &str| Err(StoreError::Constraint(format!("owner request {what}")));
        let bounded = |text: &str, max: usize| !text.is_empty() && text.len() <= max;
        if !bounded(&self.id, 64) || !bounded(&self.requester, 32) {
            return refuse("id and requester are bounded");
        }
        if self.expires_at <= self.created_at {
            return refuse("expires after it is created");
        }
        if (self.status == OwnerRequestStatus::Open) != self.decided_at.is_none() {
            return refuse("is decided exactly when closed");
        }
        Ok(())
    }
}

pub trait OwnerRequestStore {
    /// Store a new open request. Another open request by the same member on
    /// the same timing is [`StoreError::Constraint`] and nothing is written.
    fn create_owner_request(
        &self,
        request: OwnerRequest,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    fn owner_request(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<OwnerRequest>, StoreError>> + Send;

    /// Every open request (expired ones included until the tick closes
    /// them), oldest first.
    fn open_owner_requests(
        &self,
    ) -> impl Future<Output = Result<Vec<OwnerRequest>, StoreError>> + Send;

    /// Close `id` if it is still open; `false` when it was already decided.
    fn close_owner_request(
        &self,
        id: &str,
        status: OwnerRequestStatus,
        decided_by: &str,
        at: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Closed requests whose posted message still shows Accept/Decline,
    /// oldest decision first.
    fn unsettled_owner_requests(
        &self,
    ) -> impl Future<Output = Result<Vec<OwnerRequest>, StoreError>> + Send;

    /// Its message now shows the decision (or is gone).
    fn settle_owner_request_message(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Record where its message was posted.
    fn set_owner_request_message(
        &self,
        id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}
