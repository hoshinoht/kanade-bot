//! Sign-in audit log (migration 0032, user decision 2026-10-09): the
//! security events of the admin and member realms (`api::auth::audit`),
//! kept [`AUDIT_RETENTION`]. There is no scheduled store maintenance, so
//! every append deletes the rows past retention in its own transaction.

use chrono::{DateTime, TimeDelta, Utc};

use crate::domain::scheduler::StoreError;

/// How long a row is kept.
pub const AUDIT_RETENTION: TimeDelta = TimeDelta::days(90);

/// Rows an append deletes at most, so one write never holds the writer long.
pub const AUDIT_PRUNE_BATCH: i64 = 500;

/// The longest page a list returns.
pub const AUDIT_PAGE_MAX: u32 = 200;

macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }

            pub fn parse(text: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|value| value.as_str() == text)
            }
        }
    };
}

text_enum!(
    /// Which sign-in realm a row belongs to.
    AuditRealm {
        Admin => "admin",
        Member => "member",
    }
);

text_enum!(
    /// The audited event.
    AuditKind {
        LoginSucceeded => "login_succeeded",
        LoginRefused => "login_refused",
        BreakGlassUsed => "break_glass_used",
        SessionEnded => "session_ended",
        SessionRotated => "session_rotated",
        RateLimited => "rate_limited",
        RevokeFailed => "revoke_failed",
    }
);

/// One audited event. `seq` is assigned by the store (ignored on append).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditRow {
    pub seq: i64,
    pub at: DateTime<Utc>,
    pub realm: AuditRealm,
    pub event: AuditKind,
    /// `discord:<id>`, `tailscale:<login>`, `token`, or the refused user.
    pub actor: Option<String>,
    /// Sign-in method, or how break-glass was used.
    pub method: Option<String>,
    /// Refusal, end or revoke reason, or the rate-limited route.
    pub reason: Option<String>,
    /// Break-glass request line.
    pub request: Option<String>,
    /// Admin rows: the client address; member rows: its keyed tag only.
    pub client: Option<String>,
    /// Browser and system label of a sign-in.
    pub device: Option<String>,
    pub request_id: String,
}

impl AuditRow {
    /// The shape both stores enforce (SQLite by CHECK as well).
    ///
    /// # Errors
    /// [`StoreError::Constraint`] naming the first bad field.
    pub fn check(&self) -> Result<(), StoreError> {
        let bounded = |name: &str, value: &Option<String>, max: usize| match value {
            Some(text) if text.is_empty() || text.len() > max => Err(StoreError::Constraint(
                format!("audit {name} is 1-{max} bytes"),
            )),
            _ => Ok(()),
        };
        bounded("actor", &self.actor, 256)?;
        bounded("method", &self.method, 32)?;
        bounded("reason", &self.reason, 64)?;
        bounded("request", &self.request, 256)?;
        bounded("client", &self.client, 64)?;
        bounded("device", &self.device, 64)?;
        if self.request_id.len() > 128 {
            return Err(StoreError::Constraint(
                "audit request_id is at most 128 bytes".into(),
            ));
        }
        crate::domain::time::to_iso(&self.at)
            .map_err(|error| StoreError::Constraint(format!("{:?}: {error}", self.at)))?;
        Ok(())
    }
}

/// A page of the log, newest first; every field narrows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuditFilter {
    pub realm: Option<AuditRealm>,
    pub event: Option<AuditKind>,
    /// Exact `actor` text.
    pub actor: Option<String>,
    /// At or after.
    pub from: Option<DateTime<Utc>>,
    /// Strictly before.
    pub to: Option<DateTime<Utc>>,
    /// Keyset cursor: rows with a lower `seq` than this.
    pub before_seq: Option<i64>,
    /// 1..=[`AUDIT_PAGE_MAX`]; other values are clamped.
    pub limit: u32,
}

impl AuditFilter {
    pub fn page_size(&self) -> u32 {
        self.limit.clamp(1, AUDIT_PAGE_MAX)
    }
}

pub trait AuthAuditStore {
    /// Append `row` (its `seq` is assigned) after deleting up to
    /// [`AUDIT_PRUNE_BATCH`] rows older than [`AUDIT_RETENTION`] before
    /// `row.at`, in one transaction. Returns the new `seq`.
    fn append_audit(&self, row: AuditRow) -> impl Future<Output = Result<i64, StoreError>> + Send;

    /// Rows matching `filter`, newest (highest `seq`) first.
    fn audit_page(
        &self,
        filter: &AuditFilter,
    ) -> impl Future<Output = Result<Vec<AuditRow>, StoreError>> + Send;
}
