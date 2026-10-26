//! Sign-ins (mock `sign_ins::rows`) and the other live sessions of the
//! template's admin (mock `account::TOKEN`) and of Asahi on the portal
//! (mock `portal`), written as both sign-in realms record them.

use chrono::{DateTime, Duration, Utc};
use kanade::{
    app::crypto,
    infrastructure::store::{
        auth_audit::{AuditKind, AuditRealm, AuditRow, AuthAuditStore},
        web_sessions::{LoginMethod, SessionOrigin, WebSession, WebSessionStore},
    },
};

use super::{Seed, ids::*};
use crate::Fail;

/// A member realm row's keyed client tag (invented).
const TAG: &str = "5f2c9a1d7e3b40c6a8d1f0b2e4c6a8d0f1b3c5e7a9d2f4b6c8e0a1d3f5b7c9e2";
/// An admin who signed in with Discord, not on the roster.
const HOSHINO: &str = "100000000000002001";
/// A Discord user the portal refused.
const REFUSED: &str = "100000000000003999";

struct Row {
    hour: i64,
    realm: AuditRealm,
    event: AuditKind,
    actor: Option<String>,
    method: Option<&'static str>,
    reason: Option<&'static str>,
    request: Option<&'static str>,
    client: Option<&'static str>,
    device: Option<&'static str>,
}

fn row(hour: i64, realm: AuditRealm, event: AuditKind) -> Row {
    Row {
        hour,
        realm,
        event,
        actor: None,
        method: None,
        reason: None,
        request: None,
        client: None,
        device: None,
    }
}

/// Oldest first.
fn rows() -> Vec<Row> {
    use AuditKind::*;
    use AuditRealm::{Admin, Member};
    let discord = |id: &str| Some(format!("discord:{id}"));
    vec![
        Row {
            actor: Some("token".into()),
            method: Some("login"),
            request: Some("POST /api/admin/auth/token"),
            client: Some("100.64.0.9"),
            ..row(2, Admin, BreakGlassUsed)
        },
        Row {
            method: Some("token"),
            reason: Some("bad_token"),
            client: Some("100.64.0.9"),
            ..row(3, Admin, LoginRefused)
        },
        Row {
            actor: discord(HOSHINO),
            method: Some("discord"),
            client: Some("100.64.0.7"),
            device: Some("Firefox on macOS"),
            ..row(5, Admin, LoginSucceeded)
        },
        Row {
            actor: discord(MIKAN),
            method: Some("discord"),
            client: Some(TAG),
            device: Some("Safari on iOS"),
            ..row(7, Member, LoginSucceeded)
        },
        Row {
            actor: Some(REFUSED.into()),
            method: Some("discord"),
            reason: Some("not_eligible"),
            client: Some(TAG),
            ..row(8, Member, LoginRefused)
        },
        Row {
            reason: Some("discord_start"),
            client: Some(TAG),
            ..row(9, Member, RateLimited)
        },
        Row {
            actor: discord(MIKAN),
            client: Some(TAG),
            ..row(10, Member, SessionRotated)
        },
        Row {
            actor: discord(HOSHINO),
            reason: Some("logout"),
            client: Some("100.64.0.7"),
            ..row(11, Admin, SessionEnded)
        },
    ]
}

/// Another live session of `subject`: signed in and last seen these many
/// minutes before the seed's now, alive `lifetime` from sign-in.
struct Other {
    origin: SessionOrigin,
    method: LoginMethod,
    subject: &'static str,
    display: &'static str,
    device: Option<&'static str>,
    signed_in: i64,
    seen: i64,
    lifetime: Duration,
}

const OTHERS: [Other; 3] = [
    // The template admin's other break-glass session.
    Other {
        origin: SessionOrigin::Admin,
        method: LoginMethod::Token,
        subject: "token",
        display: "Admin (token)",
        device: None,
        signed_in: 300,
        seen: 25,
        lifetime: Duration::hours(24),
    },
    Other {
        origin: SessionOrigin::Public,
        method: LoginMethod::Discord,
        subject: ASAHI,
        display: "Asahi",
        device: Some("Safari · iPhone"),
        signed_in: 300,
        seen: 40,
        lifetime: Duration::hours(8),
    },
    Other {
        origin: SessionOrigin::Public,
        method: LoginMethod::Discord,
        subject: ASAHI,
        display: "Asahi",
        device: Some("Firefox · Windows"),
        signed_in: 95,
        seen: 12,
        lifetime: Duration::hours(8),
    },
];

pub async fn seed(seed: &Seed<'_>) -> Result<(), Fail> {
    for (n, row) in (1_u32..).zip(rows()) {
        seed.store
            .append_audit(AuditRow {
                seq: 0,
                at: seed.hour(row.hour),
                realm: row.realm,
                event: row.event,
                actor: row.actor,
                method: row.method.map(str::to_owned),
                reason: row.reason.map(str::to_owned),
                request: row.request.map(str::to_owned),
                client: row.client.map(str::to_owned),
                device: row.device.map(str::to_owned),
                request_id: format!("seed-{n}"),
            })
            .await?;
    }
    for (n, other) in OTHERS.iter().enumerate() {
        let minutes = |m: i64| -> DateTime<Utc> { seed.now - Duration::minutes(m) };
        let created_at = minutes(other.signed_in);
        let last_seen_at = minutes(other.seen);
        seed.store
            .put_session(
                &WebSession {
                    // Never a cookie anyone holds: only its hash is stored.
                    id_hash: crypto::sha256_hex(format!("e2e-seed-session-{n}").as_bytes()),
                    origin: other.origin,
                    method: other.method,
                    subject: other.subject.into(),
                    display: other.display.into(),
                    created_at,
                    last_seen_at,
                    checked_at: last_seen_at,
                    expires_at: created_at + other.lifetime,
                    avatar_hash: None,
                    device: other.device.map(str::to_owned),
                    client_tag: (other.origin == SessionOrigin::Public).then(|| TAG.into()),
                    superseded_until: None,
                },
                None,
            )
            .await?;
    }
    Ok(())
}
