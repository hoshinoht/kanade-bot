//! Web session storage (migration 0009). Only the SHA-256 of a session id is
//! stored, so a copied database or backup cannot be replayed as a cookie.
//! Boxed futures keep the port object-safe for the HTTP layer's shared state.

use std::{future::Future, pin::Pin};

use chrono::{DateTime, Utc};

use crate::domain::scheduler::StoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionOrigin {
    Admin,
    /// Reserved for member sessions once public exposure is authorized.
    Public,
}

impl SessionOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Public => "public",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "admin" => Some(Self::Admin),
            "public" => Some(Self::Public),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LoginMethod {
    Discord,
    Tailscale,
    /// Break-glass `ADMIN_TOKEN`.
    Token,
}

impl LoginMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discord => "discord",
            Self::Tailscale => "tailscale",
            Self::Token => "token",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "discord" => Some(Self::Discord),
            "tailscale" => Some(Self::Tailscale),
            "token" => Some(Self::Token),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSession {
    /// Lowercase hex SHA-256 of the cookie value.
    pub id_hash: String,
    pub origin: SessionOrigin,
    pub method: LoginMethod,
    /// Discord user id, Tailscale login, or `token`.
    pub subject: String,
    pub display: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    /// Last successful authorization re-check (staff gate or edge identity).
    pub checked_at: DateTime<Utc>,
    /// Absolute expiry.
    pub expires_at: DateTime<Utc>,
}

pub type SessionFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StoreError>> + Send + 'a>>;

/// Constant SQL only; every write is one transaction.
pub trait WebSessionStore: Send + Sync {
    /// Insert `session`, deleting `replaces` in the same transaction (rotation).
    /// A duplicate id hash is [`StoreError::Constraint`].
    fn put_session<'a>(
        &'a self,
        session: &'a WebSession,
        replaces: Option<&'a str>,
    ) -> SessionFuture<'a, ()>;

    fn load_session<'a>(&'a self, id_hash: &'a str) -> SessionFuture<'a, Option<WebSession>>;

    /// `false` when the session no longer exists.
    fn touch_session<'a>(
        &'a self,
        id_hash: &'a str,
        last_seen_at: DateTime<Utc>,
        checked_at: DateTime<Utc>,
    ) -> SessionFuture<'a, bool>;

    /// `false` when the session did not exist.
    fn delete_session<'a>(&'a self, id_hash: &'a str) -> SessionFuture<'a, bool>;

    /// Every session of one identity, e.g. after it lost staff access.
    fn delete_subject_sessions<'a>(
        &'a self,
        origin: SessionOrigin,
        method: LoginMethod,
        subject: &'a str,
    ) -> SessionFuture<'a, u64>;

    /// Delete sessions past their absolute expiry (`expires_at <= now`) or
    /// idle since `idle_before` (`last_seen_at <= idle_before`).
    fn prune_sessions(
        &self,
        now: DateTime<Utc>,
        idle_before: DateTime<Utc>,
    ) -> SessionFuture<'_, u64>;
}
