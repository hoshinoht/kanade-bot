//! The member's own public sessions (D5-A): list them, end one, or end every
//! one including the caller's. Rows are named by [`own::handle`], so neither
//! cookie values nor their hashes reach the client; no address or location
//! is kept to show.

use super::{MemberAuth, MemberSession};
use crate::{
    api::auth::{
        actor_id,
        audit::{AuditContext, AuditEvent},
        own,
    },
    infrastructure::store::web_sessions::{LoginMethod, SessionOrigin, WebSession},
};

pub struct MemberDevice {
    pub handle: String,
    pub session: WebSession,
    /// The session this request came with.
    pub current: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EndError {
    /// No live session of this member has that handle.
    NotFound,
    /// The caller's own session ends through sign-out, which clears its cookie.
    Current,
    Unavailable,
}

impl MemberAuth {
    /// Live sessions of the caller, oldest first.
    pub(crate) async fn devices(
        &self,
        session: &MemberSession,
    ) -> Result<Vec<MemberDevice>, EndError> {
        let now = self.now();
        let stored = self
            .sessions()
            .subject_sessions(
                SessionOrigin::Public,
                LoginMethod::Discord,
                &session.user_id,
            )
            .await
            .map_err(|_| EndError::Unavailable)?;
        Ok(stored
            .into_iter()
            .filter(|row| self.live(row, now))
            .map(|row| MemberDevice {
                handle: own::handle(&row.id_hash),
                current: row.id_hash == session.id_hash(),
                session: row,
            })
            .collect())
    }

    /// End one of the caller's other sessions.
    pub(crate) async fn end_device(
        &self,
        context: &AuditContext,
        session: &MemberSession,
        handle: &str,
    ) -> Result<(), EndError> {
        let found = self
            .devices(session)
            .await?
            .into_iter()
            .find(|device| device.handle == handle)
            .ok_or(EndError::NotFound)?;
        if found.current {
            return Err(EndError::Current);
        }
        let deleted = self
            .sessions()
            .delete_session(&found.session.id_hash)
            .await
            .map_err(|_| EndError::Unavailable)?;
        if !deleted {
            return Err(EndError::NotFound);
        }
        self.audit(
            context,
            AuditEvent::SessionEnded {
                actor: actor_id(LoginMethod::Discord, &session.user_id),
                reason: "logout_other",
            },
        );
        Ok(())
    }

    /// Sign out everywhere: every public session of the caller, this one
    /// included. Returns how many live sessions ended.
    pub(crate) async fn end_everywhere(
        &self,
        context: &AuditContext,
        session: &MemberSession,
    ) -> Result<u64, EndError> {
        let live = self.devices(session).await?.len() as u64;
        self.end_all(context, &session.user_id, "logout_all")
            .await
            .map_err(|()| EndError::Unavailable)?;
        Ok(live)
    }
}
