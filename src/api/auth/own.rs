//! The signed-in identity's own sessions (Account → Sessions): list them,
//! end one, or end every one but the caller's. "Own" means the same origin,
//! sign-in method and subject; nothing here reaches another identity. Rows
//! are named by an opaque handle derived from the stored hash, so neither
//! cookie values nor their hashes ever reach the client.

use super::{
    AdminAuth, AdminSession,
    audit::{AuditContext, AuditEvent},
    crypto,
};
use crate::infrastructure::store::web_sessions::{LoginMethod, SessionOrigin, WebSession};

/// Domain separation for [`handle`]; the hash itself never leaves the server.
const HANDLE_CONTEXT: &str = "kanade.own-session-handle.v1\0";
const HANDLE_HEX: usize = 24;

/// The client-facing name of a stored session.
pub fn handle(id_hash: &str) -> String {
    crypto::sha256_hex(format!("{HANDLE_CONTEXT}{id_hash}").as_bytes())[..HANDLE_HEX].to_owned()
}

pub struct OwnSession {
    pub handle: String,
    pub session: WebSession,
    /// The session the request came with (never set for bearer calls).
    pub current: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EndError {
    /// No live session of this identity has that handle.
    NotFound,
    /// The caller's own session ends through sign-out, which clears its cookie.
    Current,
    Unavailable,
}

impl AdminAuth {
    /// The stored subject behind a request: the Discord user id, the
    /// Tailscale login, or the break-glass fingerprint.
    fn subject(&self, session: &AdminSession) -> Option<String> {
        match session.method {
            LoginMethod::Discord => session.discord_user().map(str::to_owned),
            LoginMethod::Tailscale => session
                .actor
                .id()
                .strip_prefix("tailscale:")
                .map(str::to_owned),
            LoginMethod::Token => self.breakglass_fingerprint().map(str::to_owned),
        }
    }

    /// Live sessions of the caller's identity, oldest first. Expired and
    /// idle rows are left out (pruning removes them at the next sign-in).
    pub(crate) async fn own_sessions(
        &self,
        session: &AdminSession,
    ) -> Result<Vec<OwnSession>, EndError> {
        let Some(subject) = self.subject(session) else {
            return Ok(Vec::new());
        };
        let current = session
            .session_id()
            .map(|id| crypto::sha256_hex(id.as_bytes()));
        let now = self.now();
        let idle = self.policy().idle;
        let stored = self
            .sessions()
            .subject_sessions(SessionOrigin::Admin, session.method, &subject)
            .await
            .map_err(|_| EndError::Unavailable)?;
        Ok(stored
            .into_iter()
            .filter(|row| now < row.expires_at && now - row.last_seen_at < idle)
            .map(|row| OwnSession {
                handle: handle(&row.id_hash),
                current: current.as_deref() == Some(row.id_hash.as_str()),
                session: row,
            })
            .collect())
    }

    /// End one of the caller's other sessions, audited as a sign-out.
    pub(crate) async fn end_own_session(
        &self,
        context: &AuditContext,
        session: &AdminSession,
        target: &str,
    ) -> Result<(), EndError> {
        let found = self
            .own_sessions(session)
            .await?
            .into_iter()
            .find(|own| own.handle == target)
            .ok_or(EndError::NotFound)?;
        if found.current {
            return Err(EndError::Current);
        }
        self.end_other(context, session, &found.session).await
    }

    /// End every live session of the caller's identity except the caller's
    /// own; returns how many ended.
    pub(crate) async fn end_other_sessions(
        &self,
        context: &AuditContext,
        session: &AdminSession,
    ) -> Result<u64, EndError> {
        let mut ended = 0;
        for own in self.own_sessions(session).await? {
            if own.current {
                continue;
            }
            match self.end_other(context, session, &own.session).await {
                Ok(()) => ended += 1,
                // Ended meanwhile (its own sign-out, or expiry): nothing to do.
                Err(EndError::NotFound) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(ended)
    }

    async fn end_other(
        &self,
        context: &AuditContext,
        session: &AdminSession,
        target: &WebSession,
    ) -> Result<(), EndError> {
        let deleted = self
            .sessions()
            .delete_session(&target.id_hash)
            .await
            .map_err(|_| EndError::Unavailable)?;
        if !deleted {
            return Err(EndError::NotFound);
        }
        self.audit(
            context,
            AuditEvent::SessionEnded {
                actor: session.actor.id().to_owned(),
                reason: "logout_other",
            },
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_short_stable_and_not_the_hash() {
        let hash = "a".repeat(64);
        let one = handle(&hash);
        assert_eq!(one.len(), HANDLE_HEX);
        assert_eq!(one, handle(&hash));
        assert!(!hash.contains(&one));
        assert_ne!(one, handle(&"b".repeat(64)));
    }
}
