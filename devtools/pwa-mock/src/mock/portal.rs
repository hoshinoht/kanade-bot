//! Public-origin member sessions as `docs/notes/member-auth-contract.md`
//! describes them, without Discord: one invented member (Asahi under a
//! snowflake-shaped id), a session per browser cookie, the 10-session cap,
//! the device list and the `login_error` a sign-in ends with. Lifetimes are
//! not simulated (the mock clock is pinned); `/__mock/public/end` stands in
//! for a session that expired or lost eligibility.

use super::MoveError;
use super::clock::{iso_secs, now_secs};
use serde_json::{Value, json};

/// The member every mock sign-in becomes (the seed's Asahi, `1001`).
pub const MEMBER_ID: &str = "100000000000001001";
/// The same member as the store's seed rows name them.
pub const MEMBER_SEED_ID: &str = "1001";
pub const MEMBER_NAME: &str = "Asahi";

/// A seed member id (`1001`) as the public origin names it: snowflake-shaped,
/// so `snowflake("1001") == MEMBER_ID`.
pub fn snowflake(seed_id: &str) -> String {
    format!("1{seed_id:0>17}")
}

/// D5-A: a new sign-in ends the oldest beyond this many.
const MAX_SESSIONS: usize = 10;
/// The fresh-write window after a Discord round trip (D1 default).
const FRESH_SECS: i64 = 15 * 60;

/// The `login_error` codes a sign-in can end with (contract §1).
pub const LOGIN_ERRORS: [&str; 7] = [
    "state",
    "denied",
    "not_eligible",
    "discord",
    "unavailable",
    "rate_limited",
    "closed",
];

/// Other devices already signed in when the member first signs in here, so
/// the device list has something to sign out: (device, signed in and last
/// seen, in minutes ago).
const ELSEWHERE: [(Option<&str>, i64, i64); 2] = [
    (Some("Safari · iPhone"), 300, 40),
    (Some("Firefox · Windows"), 95, 12),
];

struct Session {
    /// The cookie value (the server keeps only its hash).
    id: String,
    handle: String,
    csrf: String,
    device: Option<String>,
    signed_in: i64,
    seen: i64,
    /// `/__mock/public/rotate`: the next request rotates the id (D9).
    rotate: bool,
}

/// What a request's session resolved to.
pub struct Current {
    pub id: String,
    pub csrf: String,
    /// Set when this request rotated the session: the new cookie and token.
    pub rotated: bool,
}

#[derive(Default)]
pub struct Portal {
    sessions: Vec<Session>,
    next: u32,
    /// The next Discord sign-in ends with this `login_error`.
    error: Option<&'static str>,
}

impl Portal {
    fn mint(&mut self, kind: &str) -> String {
        self.next += 1;
        format!("mock{kind}{:032x}", u64::from(self.next) * 0x9e37_79b9)
    }

    fn handle(&mut self) -> String {
        self.next += 1;
        format!("{:024x}", 0x00c0_ffee_0000_u64 + u64::from(self.next))
    }

    /// A Discord sign-in that succeeded: a new session (and the cookie value).
    pub fn sign_in(&mut self, device: Option<String>) -> Current {
        let now = now_secs();
        if self.sessions.is_empty() {
            for (device, signed, seen) in ELSEWHERE {
                let (id, csrf, handle) = (self.mint("id"), self.mint("csrf"), self.handle());
                self.sessions.push(Session {
                    id,
                    handle,
                    csrf,
                    device: device.map(str::to_owned),
                    signed_in: now - signed * 60,
                    seen: now - seen * 60,
                    rotate: false,
                });
            }
        }
        // Oldest first beyond the cap, leaving room for this one.
        self.sessions.sort_by_key(|s| s.signed_in);
        let over = (self.sessions.len() + 1).saturating_sub(MAX_SESSIONS);
        self.sessions.drain(..over);
        let (id, csrf, handle) = (self.mint("id"), self.mint("csrf"), self.handle());
        self.sessions.push(Session {
            id: id.clone(),
            handle,
            csrf: csrf.clone(),
            device,
            signed_in: now,
            seen: now,
            rotate: false,
        });
        Current {
            id,
            csrf,
            rotated: false,
        }
    }

    /// A live session's CSRF token, for the requests that do not need one to exist.
    pub fn token(&self, id: &str) -> Option<&str> {
        self.sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.csrf.as_str())
    }

    /// Inside the fresh-write window after its Discord round trip
    /// (`fresh_until`): owner changes need it (`401 reauth_required`).
    pub fn fresh(&self, id: &str) -> bool {
        self.sessions
            .iter()
            .any(|s| s.id == id && now_secs() < s.signed_in + FRESH_SECS)
    }

    /// The session behind a cookie, rotated first when it was asked to; the
    /// CSRF token a write must carry is the one it had before rotating.
    pub fn resolve(&mut self, id: &str) -> Option<(Current, String)> {
        let at = self.sessions.iter().position(|s| s.id == id)?;
        let rotate = self.sessions[at].rotate;
        let (new_id, new_csrf) = if rotate {
            (Some(self.mint("id")), Some(self.mint("csrf")))
        } else {
            (None, None)
        };
        let session = &mut self.sessions[at];
        let expected = session.csrf.clone();
        session.seen = now_secs();
        if let (Some(id), Some(csrf)) = (new_id, new_csrf) {
            // Same lifetime and device; only the id and its token change.
            session.id = id;
            session.csrf = csrf;
            session.rotate = false;
        }
        Some((
            Current {
                id: session.id.clone(),
                csrf: session.csrf.clone(),
                rotated: rotate,
            },
            expected,
        ))
    }

    pub fn session(&self, id: &str) -> Value {
        let signed_in = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map_or_else(now_secs, |s| s.signed_in);
        json!({
            "member": {
                "id": MEMBER_ID,
                "display": MEMBER_NAME,
                "avatar": "/api/public/session/avatar?v=1",
            },
            "fresh_until": iso_secs(signed_in + FRESH_SECS),
        })
    }

    /// `PublicSessions`, oldest first.
    pub fn list(&self, id: &str) -> Value {
        let mut rows: Vec<&Session> = self.sessions.iter().collect();
        rows.sort_by_key(|s| s.signed_in);
        let sessions: Vec<Value> = rows
            .into_iter()
            .map(|s| {
                json!({
                    "handle": s.handle, "device": s.device,
                    "signed_in_at": iso_secs(s.signed_in), "last_seen_at": iso_secs(s.seen),
                    "current": s.id == id,
                })
            })
            .collect();
        json!({ "sessions": sessions, "generated_at": iso_secs(now_secs()) })
    }

    pub fn end_one(&mut self, id: &str, handle: &str) -> Result<(), MoveError> {
        let at = self
            .sessions
            .iter()
            .position(|s| s.handle == handle)
            .ok_or(MoveError::Coded(
                404,
                "not_found",
                "That session has already ended.".into(),
            ))?;
        if self.sessions[at].id == id {
            return Err(MoveError::Coded(
                409,
                "current_session",
                "This is the session you are using; sign out instead.".into(),
            ));
        }
        self.sessions.remove(at);
        Ok(())
    }

    /// Sign out: this session only.
    pub fn end(&mut self, id: &str) {
        self.sessions.retain(|s| s.id != id);
    }

    /// Sign out everywhere, or the member lost eligibility: every session.
    pub fn end_all(&mut self) -> usize {
        let ended = self.sessions.len();
        self.sessions.clear();
        ended
    }

    pub fn rotate_all(&mut self) {
        for session in &mut self.sessions {
            session.rotate = true;
        }
    }

    /// `/__mock/public/unfresh`: every sign-in is older than the fresh-write
    /// window (the pinned clock never ages one), so writes that need a fresh
    /// sign-in answer `401 reauth_required` until the next sign-in.
    pub fn age_sign_ins(&mut self) {
        let old = now_secs() - FRESH_SECS - 5 * 60;
        for session in &mut self.sessions {
            session.signed_in = session.signed_in.min(old);
        }
    }

    pub fn fail_next(&mut self, code: &'static str) {
        self.error = Some(code);
    }

    pub fn take_error(&mut self) -> Option<&'static str> {
        self.error.take()
    }
}

#[cfg(test)]
mod tests {
    use super::Portal;

    #[test]
    fn a_first_sign_in_finds_two_other_devices_and_the_cap_ends_the_oldest() {
        let mut portal = Portal::default();
        let first = portal.sign_in(Some("Chrome · macOS".into()));
        let list = portal.list(&first.id);
        let rows = list["sessions"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.iter().filter(|r| r["current"] == true).count(), 1);
        for _ in 0..12 {
            portal.sign_in(None);
        }
        assert_eq!(portal.sessions.len(), 10);
        assert!(portal.token(&first.id).is_none(), "the oldest ends first");
    }

    #[test]
    fn rotation_keeps_the_row_and_wants_the_old_token_once() {
        let mut portal = Portal::default();
        let me = portal.sign_in(None);
        portal.rotate_all();
        let (now, expected) = portal.resolve(&me.id).unwrap();
        assert!(now.rotated);
        assert_eq!(expected, me.csrf);
        assert_ne!(now.id, me.id);
        assert!(portal.token(&me.id).is_none());
        let (again, expected) = portal.resolve(&now.id).unwrap();
        assert!(!again.rotated);
        assert_eq!(expected, now.csrf);
    }

    #[test]
    fn the_current_session_ends_only_through_sign_out() {
        let mut portal = Portal::default();
        let me = portal.sign_in(None);
        let list = portal.list(&me.id);
        let mine = list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["current"] == true)
            .unwrap()["handle"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(portal.end_one(&me.id, &mine).is_err());
        assert!(portal.end_one(&me.id, "000000000000000000000000").is_err());
        assert_eq!(portal.end_all(), 3);
        assert!(portal.token(&me.id).is_none());
    }

    #[test]
    fn aged_sign_ins_are_no_longer_fresh_until_the_next_one() {
        let mut portal = Portal::default();
        let me = portal.sign_in(None);
        assert!(portal.fresh(&me.id));
        portal.age_sign_ins();
        assert!(!portal.fresh(&me.id));
        let again = portal.sign_in(None);
        assert!(portal.fresh(&again.id));
    }
}
