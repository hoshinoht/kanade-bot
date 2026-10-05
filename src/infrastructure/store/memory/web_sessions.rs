//! In-memory `WebSessionStore` mirroring migration 0009's CHECKs.

use chrono::{DateTime, Utc};

use super::{MemoryScheduleStore, micros};
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::web_sessions::{
    LoginMethod, SessionFuture, SessionOrigin, WebSession, WebSessionStore,
};

fn check(session: &WebSession) -> Result<(), StoreError> {
    let hash_ok = session.id_hash.len() == 64
        && session
            .id_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    let subject = session.subject.chars().count();
    // 0025: 32 lowercase hex digits, or `a_` and 32 (animated).
    let hex = |text: &str| {
        text.len() == 32
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    let avatar_ok = session
        .avatar_hash
        .as_deref()
        .is_none_or(|avatar| hex(avatar.strip_prefix("a_").unwrap_or(avatar)));
    if !hash_ok
        || !avatar_ok
        || !(1..=320).contains(&subject)
        || session.display.chars().count() > 200
    {
        return Err(StoreError::Constraint("web_sessions CHECK".into()));
    }
    Ok(())
}

fn normalised(session: &WebSession) -> WebSession {
    WebSession {
        created_at: micros(session.created_at),
        last_seen_at: micros(session.last_seen_at),
        checked_at: micros(session.checked_at),
        expires_at: micros(session.expires_at),
        ..session.clone()
    }
}

impl MemoryScheduleStore {
    fn sessions(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::BTreeMap<String, WebSession>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl WebSessionStore for MemoryScheduleStore {
    fn put_session<'a>(
        &'a self,
        session: &'a WebSession,
        replaces: Option<&'a str>,
    ) -> SessionFuture<'a, ()> {
        Box::pin(async move {
            check(session)?;
            let mut sessions = self.sessions();
            let collides = sessions.contains_key(&session.id_hash)
                && replaces != Some(session.id_hash.as_str());
            if collides {
                return Err(StoreError::Constraint("web_sessions.id_hash UNIQUE".into()));
            }
            if let Some(old) = replaces {
                sessions.remove(old);
            }
            sessions.insert(session.id_hash.clone(), normalised(session));
            Ok(())
        })
    }

    fn load_session<'a>(&'a self, id_hash: &'a str) -> SessionFuture<'a, Option<WebSession>> {
        Box::pin(async move { Ok(self.sessions().get(id_hash).cloned()) })
    }

    fn touch_session<'a>(
        &'a self,
        id_hash: &'a str,
        last_seen_at: DateTime<Utc>,
        checked_at: DateTime<Utc>,
    ) -> SessionFuture<'a, bool> {
        Box::pin(async move {
            Ok(match self.sessions().get_mut(id_hash) {
                Some(session) => {
                    session.last_seen_at = micros(last_seen_at);
                    session.checked_at = micros(checked_at);
                    true
                }
                None => false,
            })
        })
    }

    fn delete_session<'a>(&'a self, id_hash: &'a str) -> SessionFuture<'a, bool> {
        Box::pin(async move { Ok(self.sessions().remove(id_hash).is_some()) })
    }

    fn delete_subject_sessions<'a>(
        &'a self,
        origin: SessionOrigin,
        method: LoginMethod,
        subject: &'a str,
    ) -> SessionFuture<'a, u64> {
        Box::pin(async move {
            let mut sessions = self.sessions();
            let before = sessions.len();
            sessions.retain(|_, session| {
                !(session.origin == origin
                    && session.method == method
                    && session.subject == subject)
            });
            Ok((before - sessions.len()) as u64)
        })
    }

    fn prune_sessions(
        &self,
        now: DateTime<Utc>,
        idle_before: DateTime<Utc>,
    ) -> SessionFuture<'_, u64> {
        Box::pin(async move {
            let (now, idle_before) = (micros(now), micros(idle_before));
            let mut sessions = self.sessions();
            let before = sessions.len();
            sessions.retain(|_, session| {
                session.expires_at > now && session.last_seen_at > idle_before
            });
            Ok((before - sessions.len()) as u64)
        })
    }
}
