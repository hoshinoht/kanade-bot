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
    /// The Discord avatar hash reported at sign-in (0025); `None` without one.
    pub avatar_hash: Option<String>,
    /// "Browser · system" read from the sign-in's User-Agent (0026); `None`
    /// when unrecognised.
    pub device: Option<String>,
    /// Keyed hash (64 lowercase hex) of the client address the session was
    /// issued to, never the address itself (0030); `None` for admin sessions
    /// and rows from before 0030.
    pub client_tag: Option<String>,
    /// Set only on an id rotated out by [`WebSessionStore::rotate_session`]
    /// (0030): [`WebSessionStore::load_superseded`] returns it until this
    /// instant. A live session has `None`.
    pub superseded_until: Option<DateTime<Utc>>,
}

pub type SessionFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StoreError>> + Send + 'a>>;

/// Constant SQL only; every write is one transaction. Admin and public
/// sessions share the table but never act on each other's rows.
pub trait WebSessionStore: Send + Sync {
    /// Insert `session`, deleting `replaces` in the same transaction (rotation)
    /// when it is a session of the same origin; another origin's row is left
    /// alone and the insert still happens. A duplicate id hash, or a
    /// `session` that is already superseded, is [`StoreError::Constraint`].
    fn put_session<'a>(
        &'a self,
        session: &'a WebSession,
        replaces: Option<&'a str>,
    ) -> SessionFuture<'a, ()>;

    /// Sign-in under a per-identity cap, in one write: prune `session`'s
    /// origin as [`Self::prune_sessions`] does (`now`, `idle_before`), delete
    /// `replaces` when it is a row of the same origin, end the oldest live
    /// sessions of the same origin, method and subject beyond `max - 1`
    /// (superseded ids are not counted) and insert `session`. Returns how
    /// many sessions the cap ended. A duplicate id hash, or a `session` that
    /// is already superseded, is [`StoreError::Constraint`] and writes nothing.
    fn put_capped_session<'a>(
        &'a self,
        session: &'a WebSession,
        replaces: Option<&'a str>,
        max: usize,
        now: DateTime<Utc>,
        idle_before: DateTime<Utc>,
    ) -> SessionFuture<'a, u64>;

    /// A live session; a superseded id is only [`Self::load_superseded`]'s.
    fn load_session<'a>(&'a self, id_hash: &'a str) -> SessionFuture<'a, Option<WebSession>>;

    /// `false` when no live session has this id. `last_seen_at` never moves
    /// backwards, so a delayed quiet re-check cannot undo a newer touch.
    fn touch_session<'a>(
        &'a self,
        id_hash: &'a str,
        last_seen_at: DateTime<Utc>,
        checked_at: DateTime<Utc>,
    ) -> SessionFuture<'a, bool>;

    /// `false` when the session (live or superseded) did not exist.
    fn delete_session<'a>(&'a self, id_hash: &'a str) -> SessionFuture<'a, bool>;

    /// Every live session of one identity, oldest first (expired ones
    /// included until pruned; callers apply the policy). Superseded ids are
    /// left out, so after a prune its length and first entry are the
    /// per-member session count and the oldest session.
    fn subject_sessions<'a>(
        &'a self,
        origin: SessionOrigin,
        method: LoginMethod,
        subject: &'a str,
    ) -> SessionFuture<'a, Vec<WebSession>>;

    /// Every session of one identity, superseded ids included, e.g. after it
    /// lost staff access.
    fn delete_subject_sessions<'a>(
        &'a self,
        origin: SessionOrigin,
        method: LoginMethod,
        subject: &'a str,
    ) -> SessionFuture<'a, u64>;

    /// Delete `origin`'s sessions past their absolute expiry
    /// (`expires_at <= now`), idle since `idle_before`
    /// (`last_seen_at <= idle_before`) or superseded with their grace over
    /// (`superseded_until <= now`). Each origin has its own idle policy, so
    /// the other origin's rows are never touched.
    fn prune_sessions(
        &self,
        origin: SessionOrigin,
        now: DateTime<Utc>,
        idle_before: DateTime<Utc>,
    ) -> SessionFuture<'_, u64>;

    /// Rotate `old` to `session` in one write, keeping `old` readable through
    /// [`Self::load_superseded`] until `superseded_until` (D9 grace) instead
    /// of deleting it. `false`, writing nothing, unless `old` is a live
    /// session of the same origin, method and subject as `session`. A
    /// duplicate id hash, or a `session` that is already superseded, is
    /// [`StoreError::Constraint`].
    fn rotate_session<'a>(
        &'a self,
        session: &'a WebSession,
        old: &'a str,
        superseded_until: DateTime<Utc>,
    ) -> SessionFuture<'a, bool>;

    /// A superseded id while its grace lasts (`now < superseded_until`).
    /// Which requests it may serve (safe methods only) is the caller's rule.
    fn load_superseded<'a>(
        &'a self,
        id_hash: &'a str,
        now: DateTime<Utc>,
    ) -> SessionFuture<'a, Option<WebSession>>;
}
