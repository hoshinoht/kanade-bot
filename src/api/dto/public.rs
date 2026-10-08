//! Wire shapes of the member session routes (`public.json`). The structs
//! carry no doc comments so the generated TypeScript stays exactly the
//! frozen shapes; the schema documents each field.

use serde::Serialize;

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PublicMember {
    pub id: String,
    pub display: String,
    pub avatar: String,
}

// The CSRF token travels in `X-Kanade-CSRF`, never in the body.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PublicSession {
    pub member: PublicMember,
    pub fresh_until: String,
}

// No address or location: only what the device list shows (D5-A).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PublicSessionRow {
    pub handle: String,
    pub device: Option<String>,
    pub signed_in_at: String,
    pub last_seen_at: String,
    pub current: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PublicSessions {
    pub sessions: Vec<PublicSessionRow>,
    pub generated_at: String,
}
