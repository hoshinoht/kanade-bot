//! What one Discord request did, as far as the caller may rely on it.

/// Every transport call ends in exactly one of these.
///
/// Only [`Outcome::DefinitelyRejected`] proves nothing happened remotely
/// (callers deleting a message treat `UnknownMessage` as already gone). An
/// [`Outcome::Ambiguous`] create may have posted a message; a journal must
/// record it indeterminate and never replay it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    Delivered(T),
    DefinitelyRejected(RejectionKind),
    Ambiguous(AmbiguousKind),
}

impl<T> Outcome<T> {
    pub fn is_delivered(&self) -> bool {
        matches!(self, Self::Delivered(_))
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Outcome<U> {
        match self {
            Self::Delivered(value) => Outcome::Delivered(f(value)),
            Self::DefinitelyRejected(kind) => Outcome::DefinitelyRejected(kind),
            Self::Ambiguous(kind) => Outcome::Ambiguous(kind),
        }
    }
}

/// Why Discord (or the client, before sending) refused a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectionKind {
    /// Discord code 50013.
    MissingPermissions,
    /// Discord code 50001: the bot cannot see the channel.
    MissingAccess,
    /// Discord code 10003.
    UnknownChannel,
    /// Discord code 10008.
    UnknownMessage,
    /// HTTP 401, or a token the client already knows is invalid (not sent).
    Unauthorized,
    /// Refused by client-side validation; never sent.
    Invalid,
    /// Never handed to a connection: connect/TLS failure, deadline passed
    /// while waiting for a rate-limit permit, or cancelled pre-flight.
    NotSent,
    /// Every send was answered 429 and the send cap or deadline stopped
    /// further re-sends; Discord processed none of them.
    RateLimited,
    /// Any other 4xx except 429.
    Http { status: u16, code: Option<u64> },
}

impl RejectionKind {
    /// A content-free label for logs, e.g. `http_400`.
    pub fn label(&self) -> String {
        match self {
            Self::MissingPermissions => "missing_permissions".into(),
            Self::MissingAccess => "missing_access".into(),
            Self::UnknownChannel => "unknown_channel".into(),
            Self::UnknownMessage => "unknown_message".into(),
            Self::Unauthorized => "unauthorized".into(),
            Self::Invalid => "invalid".into(),
            Self::NotSent => "not_sent".into(),
            Self::RateLimited => "rate_limited".into(),
            Self::Http { status, .. } => format!("http_{status}"),
        }
    }
}

/// Why the result of a request is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmbiguousKind {
    /// The per-attempt timeout or the overall deadline elapsed.
    Timeout,
    /// The connection failed; the request may have been written.
    Connection,
    /// HTTP 5xx.
    ServerError { status: u16 },
    /// A response arrived but its status or body could not be read.
    UnreadableResponse,
}

impl AmbiguousKind {
    /// A content-free label for logs, e.g. `ambiguous_timeout`.
    pub fn label(&self) -> String {
        match self {
            Self::Timeout => "ambiguous_timeout".into(),
            Self::Connection => "ambiguous_connection".into(),
            Self::ServerError { status } => format!("ambiguous_http_{status}"),
            Self::UnreadableResponse => "ambiguous_unreadable".into(),
        }
    }
}

impl<T> Outcome<T> {
    /// Why nothing (certainly) arrived: `None` when delivered.
    pub fn failure_label(&self) -> Option<String> {
        match self {
            Self::Delivered(_) => None,
            Self::DefinitelyRejected(kind) => Some(kind.label()),
            Self::Ambiguous(kind) => Some(kind.label()),
        }
    }
}

/// Discord error codes the adapter distinguishes.
pub mod codes {
    pub const UNKNOWN_CHANNEL: u64 = 10003;
    pub const UNKNOWN_MESSAGE: u64 = 10008;
    pub const MISSING_ACCESS: u64 = 50001;
    pub const MISSING_PERMISSIONS: u64 = 50013;
}

/// Classify a non-success HTTP status with its Discord error code, if any.
///
/// 429 is never passed here in practice (Twilight re-queues it through its
/// rate limiter because Discord did not process the request); should it
/// arrive, it is treated as ambiguous rather than definitive.
pub fn classify_status<T>(status: u16, code: Option<u64>) -> Outcome<T> {
    let kind = match (status, code) {
        (500..=599, _) => return Outcome::Ambiguous(AmbiguousKind::ServerError { status }),
        (429, _) => return Outcome::Ambiguous(AmbiguousKind::UnreadableResponse),
        (_, Some(codes::MISSING_PERMISSIONS)) => RejectionKind::MissingPermissions,
        (_, Some(codes::MISSING_ACCESS)) => RejectionKind::MissingAccess,
        (_, Some(codes::UNKNOWN_CHANNEL)) => RejectionKind::UnknownChannel,
        (_, Some(codes::UNKNOWN_MESSAGE)) => RejectionKind::UnknownMessage,
        (401, _) => RejectionKind::Unauthorized,
        (400..=499, _) => RejectionKind::Http { status, code },
        _ => return Outcome::Ambiguous(AmbiguousKind::UnreadableResponse),
    };
    Outcome::DefinitelyRejected(kind)
}
