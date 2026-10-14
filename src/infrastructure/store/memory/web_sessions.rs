//! In-memory `WebSessionStore` mirroring migrations 0009/0025/0026/0030 and
//! their CHECKs.

use chrono::{DateTime, Utc};

use super::{MemoryScheduleStore, micros};
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::web_sessions::{
    LoginMethod, SessionFuture, SessionOrigin, WebSession, WebSessionStore,
};

/// 64 lowercase hex digits, as `id_hash` and 0030's `client_tag` are CHECKed.
fn hex64(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn check(session: &WebSession) -> Result<(), StoreError> {
    if session.superseded_until.is_some() {
        return Err(StoreError::Constraint(
            "web_sessions: a new session cannot start superseded".into(),
        ));
    }
    let hash_ok = hex64(&session.id_hash);
    let tag_ok = session.client_tag.as_deref().is_none_or(hex64);
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
    // 0026: 1 to 64 characters.
    let device_ok = session
        .device
        .as_deref()
        .is_none_or(|device| (1..=64).contains(&device.chars().count()));
    if !hash_ok
        || !tag_ok
        || !avatar_ok
        || !device_ok
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
            // Only the same origin's row is replaced; another origin's stays.
            let replaced = replaces.filter(|old| {
                sessions
                    .get(*old)
                    .is_some_and(|row| row.origin == session.origin)
            });
            let collides = sessions.contains_key(&session.id_hash)
                && replaced != Some(session.id_hash.as_str());
            if collides {
                return Err(StoreError::Constraint("web_sessions.id_hash UNIQUE".into()));
            }
            if let Some(old) = replaced {
                sessions.remove(old);
            }
            sessions.insert(session.id_hash.clone(), normalised(session));
            Ok(())
        })
    }

    fn put_capped_session<'a>(
        &'a self,
        session: &'a WebSession,
        replaces: Option<&'a str>,
        max: usize,
        now: DateTime<Utc>,
        idle_before: DateTime<Utc>,
    ) -> SessionFuture<'a, u64> {
        Box::pin(async move {
            check(session)?;
            let (now, idle_before) = (micros(now), micros(idle_before));
            let mut sessions = self.sessions();
            // Plan every deletion first, so a refused insert writes nothing.
            let mut ended: Vec<String> = sessions
                .values()
                .filter(|row| {
                    row.origin == session.origin
                        && (row.expires_at <= now
                            || row.last_seen_at <= idle_before
                            || row.superseded_until.is_some_and(|until| until <= now))
                })
                .map(|row| row.id_hash.clone())
                .collect();
            if let Some(old) = replaces
                && sessions
                    .get(old)
                    .is_some_and(|row| row.origin == session.origin)
            {
                ended.push(old.to_owned());
            }
            let mut live: Vec<&WebSession> = sessions
                .values()
                .filter(|row| {
                    row.origin == session.origin
                        && row.method == session.method
                        && row.subject == session.subject
                        && row.superseded_until.is_none()
                        && !ended.contains(&row.id_hash)
                })
                .collect();
            live.sort_by(|a, b| (a.created_at, &a.id_hash).cmp(&(b.created_at, &b.id_hash)));
            let excess = live.len().saturating_sub(max.saturating_sub(1));
            let capped: Vec<String> = live[..excess]
                .iter()
                .map(|row| row.id_hash.clone())
                .collect();
            ended.extend(capped);
            if sessions.contains_key(&session.id_hash) && !ended.contains(&session.id_hash) {
                return Err(StoreError::Constraint("web_sessions.id_hash UNIQUE".into()));
            }
            for id_hash in &ended {
                sessions.remove(id_hash);
            }
            sessions.insert(session.id_hash.clone(), normalised(session));
            Ok(excess as u64)
        })
    }

    fn load_session<'a>(&'a self, id_hash: &'a str) -> SessionFuture<'a, Option<WebSession>> {
        Box::pin(async move {
            Ok(self
                .sessions()
                .get(id_hash)
                .filter(|session| session.superseded_until.is_none())
                .cloned())
        })
    }

    fn touch_session<'a>(
        &'a self,
        id_hash: &'a str,
        last_seen_at: DateTime<Utc>,
        checked_at: DateTime<Utc>,
    ) -> SessionFuture<'a, bool> {
        Box::pin(async move {
            Ok(match self.sessions().get_mut(id_hash) {
                Some(session) if session.superseded_until.is_none() => {
                    session.last_seen_at = session.last_seen_at.max(micros(last_seen_at));
                    session.checked_at = micros(checked_at);
                    true
                }
                _ => false,
            })
        })
    }

    fn delete_session<'a>(&'a self, id_hash: &'a str) -> SessionFuture<'a, bool> {
        Box::pin(async move { Ok(self.sessions().remove(id_hash).is_some()) })
    }

    fn subject_sessions<'a>(
        &'a self,
        origin: SessionOrigin,
        method: LoginMethod,
        subject: &'a str,
    ) -> SessionFuture<'a, Vec<WebSession>> {
        Box::pin(async move {
            let mut found: Vec<WebSession> = self
                .sessions()
                .values()
                .filter(|session| {
                    session.origin == origin
                        && session.method == method
                        && session.subject == subject
                        && session.superseded_until.is_none()
                })
                .cloned()
                .collect();
            found.sort_by(|a, b| (a.created_at, &a.id_hash).cmp(&(b.created_at, &b.id_hash)));
            Ok(found)
        })
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
        origin: SessionOrigin,
        now: DateTime<Utc>,
        idle_before: DateTime<Utc>,
    ) -> SessionFuture<'_, u64> {
        Box::pin(async move {
            let (now, idle_before) = (micros(now), micros(idle_before));
            let mut sessions = self.sessions();
            let before = sessions.len();
            sessions.retain(|_, session| {
                session.origin != origin
                    || (session.expires_at > now
                        && session.last_seen_at > idle_before
                        && session.superseded_until.is_none_or(|until| until > now))
            });
            Ok((before - sessions.len()) as u64)
        })
    }

    fn rotate_session<'a>(
        &'a self,
        session: &'a WebSession,
        old: &'a str,
        superseded_until: DateTime<Utc>,
    ) -> SessionFuture<'a, bool> {
        Box::pin(async move {
            check(session)?;
            let mut sessions = self.sessions();
            let rotatable = sessions.get(old).is_some_and(|current| {
                current.superseded_until.is_none()
                    && (current.origin, current.method, current.subject.as_str())
                        == (session.origin, session.method, session.subject.as_str())
            });
            if !rotatable {
                return Ok(false);
            }
            if sessions.contains_key(&session.id_hash) {
                return Err(StoreError::Constraint("web_sessions.id_hash UNIQUE".into()));
            }
            if let Some(current) = sessions.get_mut(old) {
                current.superseded_until = Some(micros(superseded_until));
            }
            sessions.insert(session.id_hash.clone(), normalised(session));
            Ok(true)
        })
    }

    fn load_superseded<'a>(
        &'a self,
        id_hash: &'a str,
        now: DateTime<Utc>,
    ) -> SessionFuture<'a, Option<WebSession>> {
        Box::pin(async move {
            let now = micros(now);
            Ok(self
                .sessions()
                .get(id_hash)
                .filter(|session| session.superseded_until.is_some_and(|until| until > now))
                .cloned())
        })
    }
}
