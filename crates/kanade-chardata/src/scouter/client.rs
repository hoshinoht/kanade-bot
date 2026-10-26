//! Plain-HTTP GETs to the scouter sidecar on the crate's hyper stack, as the
//! Nexon client does: one connection per call, a whole-exchange timeout, a
//! capped body, redirects never followed. A lookup's query is built here
//! and only here, from [`RosterIgn`]s: `ign=<IGN>` or `igns=<A>,<B>,…`.
//! Nothing else is ever sent (no preset, no `assume_crit`: the scouter's
//! default assumes 100% critical rate below it and labels the result; no
//! Nexon field, no cookie or credential).

use std::{pin::pin, time::Duration};

use http_body_util::{BodyExt, Empty, Limited};
use hyper::{
    Request, Uri,
    body::Bytes,
    client::conn::http1,
    header::{ACCEPT, HOST, RETRY_AFTER, USER_AGENT},
};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use tokio::net::TcpStream;

use super::{
    RosterIgn,
    wire::{Batch, ErrorBody, Member},
};

/// The sidecar may hold a request up to 60 s in its own queue, then works
/// up to 120 s on it (its write timeout is 190 s); kanade waits that long.
pub const TIMEOUT: Duration = Duration::from_secs(190);
/// The scouter's batch limit.
pub const MAX_BATCH: usize = 6;
pub const PATH: &str = "/v1/bosses";
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_BODY: usize = 512 * 1024;
/// A `busy` answer without `Retry-After` (the sidecar always sends one).
const BUSY_RETRY: u32 = 5;

/// A lookup that produced no estimate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupError {
    /// An error answer: the HTTP status, the scouter's error code (letters
    /// and `_` only, else `unknown`), the IGN it names, if any, and the
    /// `Retry-After` seconds of a 429. Its message is never kept.
    Refused {
        status: u16,
        code: String,
        ign: Option<String>,
        retry_after: Option<u32>,
    },
    /// No answer within the client timeout.
    Timeout,
    /// No connection could be made.
    Unreachable,
    /// The answer could not be read (the status, when there was one).
    Unreadable { status: Option<u16> },
}

impl LookupError {
    /// For health and logs: the scouter code, else `timeout`,
    /// `unreachable` or `unreadable`.
    pub fn label(&self) -> &str {
        match self {
            Self::Refused { code, .. } => code,
            Self::Timeout => "timeout",
            Self::Unreachable => "unreachable",
            Self::Unreadable { .. } => "unreadable",
        }
    }
}

/// The scouter sidecar at `KANADE_SCOUTER_URL`.
#[derive(Clone, Debug)]
pub struct ScouterClient {
    host: String,
    port: u16,
    /// `host[:port]`, as the `Host` header.
    authority: String,
    timeout: Duration,
}

/// The lookup query: `ign=` for one character, `igns=` (comma-separated)
/// for a batch. IGNs are ASCII letters and digits, so nothing needs
/// escaping.
pub fn query(igns: &[RosterIgn], batch: bool) -> String {
    let names: Vec<&str> = igns.iter().map(RosterIgn::as_str).collect();
    if batch {
        format!("igns={}", names.join(","))
    } else {
        format!("ign={}", names.join(","))
    }
}

impl ScouterClient {
    /// `url` as the configuration accepted it (`http://host[:port]`).
    pub fn new(url: &str) -> Option<Self> {
        let uri = url.parse::<Uri>().ok()?;
        if uri.scheme_str() != Some("http") {
            return None;
        }
        let authority = uri.authority()?;
        let raw = authority.host();
        let host = raw
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(raw)
            .to_owned();
        Some(Self {
            host,
            port: authority.port_u16().unwrap_or(80),
            authority: authority.as_str().to_owned(),
            timeout: TIMEOUT,
        })
    }

    /// A shorter client timeout (tests only).
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The longest one call waits.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// `GET /v1/bosses?ign=<IGN>`, waiting at most `within` (and never past
    /// the client timeout).
    pub async fn single(&self, ign: &RosterIgn, within: Duration) -> Result<Member, LookupError> {
        self.lookup(&query(std::slice::from_ref(ign), false), within)
            .await
    }

    /// `GET /v1/bosses?igns=<A>,<B>,…` (1 to [`MAX_BATCH`] unique IGNs),
    /// waiting at most `within`.
    pub async fn batch(&self, igns: &[RosterIgn], within: Duration) -> Result<Batch, LookupError> {
        // The caller dedupes and caps; the scouter would refuse it anyway.
        if !(1..=MAX_BATCH).contains(&igns.len()) {
            return Err(LookupError::Unreadable { status: None });
        }
        self.lookup(&query(igns, true), within).await
    }

    /// `GET /healthz` within a short timeout.
    pub async fn healthy(&self) -> Result<(), LookupError> {
        let answer = self
            .get("/healthz", PROBE_TIMEOUT.min(self.timeout))
            .await?;
        if answer.status == 200 {
            Ok(())
        } else {
            Err(LookupError::Unreadable {
                status: Some(answer.status),
            })
        }
    }

    async fn lookup<T: DeserializeOwned>(
        &self,
        query: &str,
        within: Duration,
    ) -> Result<T, LookupError> {
        let answer = self
            .get(&format!("{PATH}?{query}"), self.timeout.min(within))
            .await?;
        if answer.status == 200 {
            return serde_json::from_slice(&answer.body)
                .map_err(|_| LookupError::Unreadable { status: Some(200) });
        }
        let Ok(body) = serde_json::from_slice::<ErrorBody>(&answer.body) else {
            return Err(LookupError::Unreadable {
                status: Some(answer.status),
            });
        };
        let code = body.error.code;
        let code = if !code.is_empty()
            && code.len() <= 48
            && code
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        {
            code
        } else {
            "unknown".into()
        };
        let ign = body.error.ign.filter(|ign| {
            (1..=32).contains(&ign.len()) && ign.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });
        let retry_after = (answer.status == 429).then(|| {
            answer
                .retry_after
                .unwrap_or(if code == "busy" { BUSY_RETRY } else { 60 })
        });
        Err(LookupError::Refused {
            status: answer.status,
            code,
            ign,
            retry_after,
        })
    }

    async fn get(&self, path: &str, timeout: Duration) -> Result<Answer, LookupError> {
        let request = Request::get(path)
            .header(HOST, &self.authority)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, concat!("kanade/", env!("CARGO_PKG_VERSION")))
            .body(Empty::<Bytes>::new())
            .map_err(|_| LookupError::Unreadable { status: None })?;
        tokio::time::timeout(timeout, async {
            let tcp = TcpStream::connect((self.host.as_str(), self.port))
                .await
                .map_err(|_| LookupError::Unreachable)?;
            exchange(tcp, request).await
        })
        .await
        .map_err(|_| LookupError::Timeout)?
    }
}

struct Answer {
    status: u16,
    retry_after: Option<u32>,
    body: Bytes,
}

async fn exchange(tcp: TcpStream, request: Request<Empty<Bytes>>) -> Result<Answer, LookupError> {
    let (mut sender, connection) = http1::Builder::new()
        .handshake(TokioIo::new(tcp))
        .await
        .map_err(|_| LookupError::Unreadable { status: None })?;
    let mut connection = pin!(connection);
    let mut exchange = pin!(async move {
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| LookupError::Unreadable { status: None })?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u32>().ok());
        let body = Limited::new(response.into_body(), MAX_BODY)
            .collect()
            .await
            .map_err(|_| LookupError::Unreadable {
                status: Some(status),
            })?
            .to_bytes();
        Ok(Answer {
            status,
            retry_after,
            body,
        })
    });
    tokio::select! {
        biased;
        reply = &mut exchange => reply,
        _ = &mut connection => exchange.await,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use kanade_domain::{
        characters::{Character, CharacterRole},
        history::Actor,
    };

    fn roster(ign: &str) -> Character {
        let at = Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 0).unwrap();
        Character {
            id: "c1".into(),
            member_id: "1001".into(),
            ign: ign.into(),
            world: "msea".into(),
            class: Some("Bishop".into()),
            level: Some(283),
            role: CharacterRole::Main,
            retired_at: None,
            created_at: at,
            updated_at: at,
            updated_by: Actor::admin("seed"),
            version: 1,
        }
    }

    #[test]
    fn the_query_is_ign_or_igns_and_nothing_else() {
        let mika = RosterIgn::of(&roster("MikaBish")).unwrap();
        let ren = RosterIgn::of(&roster("RenShade")).unwrap();
        assert_eq!(query(std::slice::from_ref(&mika), false), "ign=MikaBish");
        assert_eq!(query(&[mika.clone(), ren], true), "igns=MikaBish,RenShade");
        assert_eq!(mika.key(), "mikabish");
        // Level, class and anything outside the scouter's rule never pass.
        for ign in ["", "Mika Bish", "Mika&preset=1", "Mikä", &"a".repeat(33)] {
            assert_eq!(RosterIgn::of(&roster(ign)), None, "{ign:?}");
        }
    }

    #[test]
    fn only_plain_http_urls_build_a_client() {
        let client = ScouterClient::new("http://scouter:8091").unwrap();
        assert_eq!((client.host.as_str(), client.port), ("scouter", 8091));
        assert_eq!(client.authority, "scouter:8091");
        assert_eq!(client.timeout, TIMEOUT);
        let v6 = ScouterClient::new("http://[::1]").unwrap();
        assert_eq!((v6.host.as_str(), v6.port), ("::1", 80));
        assert!(ScouterClient::new("https://scouter:8091").is_none());
    }
}
