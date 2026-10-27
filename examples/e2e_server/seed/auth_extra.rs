//! Asahi's other admin Discord sessions (mock `account::DISCORD`): with the
//! template's own Discord sign-in ("Chrome · macOS", this one), Account ›
//! Sessions lists three, as the mock does.

use chrono::{DateTime, Duration, Utc};
use kanade::app::crypto;
use kanade_store::{
    SqliteStore,
    web_sessions::{LoginMethod, SessionOrigin, WebSession, WebSessionStore},
};

use super::ids::ASAHI;
use crate::Fail;

/// Device, signed in and last seen minutes before `now`.
const OTHERS: [(&str, i64, i64); 2] = [("Safari · iPhone", 480, 40), ("Firefox · Windows", 95, 12)];

pub async fn seed(store: &SqliteStore, now: DateTime<Utc>) -> Result<(), Fail> {
    for (n, (device, signed_in, seen)) in OTHERS.into_iter().enumerate() {
        let created_at = now - Duration::minutes(signed_in);
        let last_seen_at = now - Duration::minutes(seen);
        store
            .put_session(
                &WebSession {
                    // Never a cookie anyone holds: only its hash is stored.
                    id_hash: crypto::sha256_hex(format!("e2e-admin-discord-{n}").as_bytes()),
                    origin: SessionOrigin::Admin,
                    method: LoginMethod::Discord,
                    subject: ASAHI.into(),
                    display: "Asahi".into(),
                    created_at,
                    last_seen_at,
                    checked_at: last_seen_at,
                    expires_at: created_at + Duration::hours(24),
                    avatar_hash: None,
                    device: Some(device.into()),
                    client_tag: None,
                    superseded_until: None,
                },
                None,
            )
            .await?;
    }
    Ok(())
}
