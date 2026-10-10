//! CSRF for both origins: every unsafe cookie-authenticated request must
//! come from this origin (`Sec-Fetch-Site`/`Origin`) and echo the session's
//! token in [`CSRF_HEADER`]. The token is an HMAC of the session id under a
//! per-realm context, so it needs no storage, dies with the session and a
//! member token never validates as an admin one (or the reverse).

use axum::http::{
    HeaderMap, Method,
    header::{HOST, ORIGIN},
};

use super::crypto;

/// Response header on the session reads and required request header on mutations.
pub const CSRF_HEADER: &str = "x-kanade-csrf";
/// The admin realm's token context.
pub const ADMIN: &[u8] = b"kanade-admin-csrf-v1";
/// The member (public origin) realm's token context.
pub const MEMBER: &[u8] = b"kanade-public-csrf-v1";

pub fn is_unsafe(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

pub fn token(context: &[u8], session_id: &str) -> String {
    crypto::keyed_tag(session_id.as_bytes(), context)
}

pub fn token_matches(context: &[u8], headers: &HeaderMap, session_id: &str) -> bool {
    let mut values = headers.get_all(CSRF_HEADER).iter();
    match (values.next(), values.next()) {
        (Some(value), None) => {
            crypto::constant_eq(value.as_bytes(), token(context, session_id).as_bytes())
        }
        _ => false,
    }
}

/// Browsers send `Sec-Fetch-Site` and/or `Origin` on unsafe requests; each one
/// present must name this origin, and at least one must be present. `http:`
/// origins are accepted only for loopback development hosts.
pub fn same_origin(headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(HOST).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let site = headers.get("sec-fetch-site").map(|value| value.as_bytes());
    let origin = headers.get(ORIGIN).and_then(|value| value.to_str().ok());
    if site.is_none() && origin.is_none() {
        return false;
    }
    if site.is_some_and(|site| site != b"same-origin") {
        return false;
    }
    origin.is_none_or(|origin| {
        let host = host.to_ascii_lowercase();
        let origin = origin.to_ascii_lowercase();
        origin == format!("https://{host}")
            || (loopback(&host) && origin == format!("http://{host}"))
    })
}

fn loopback(host: &str) -> bool {
    ["localhost", "127.0.0.1", "[::1]"].iter().any(|name| {
        host.strip_prefix(name)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
    })
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn only_this_origin_passes() {
        let host = ("host", "kanade.test");
        assert!(same_origin(&headers(&[
            host,
            ("sec-fetch-site", "same-origin")
        ])));
        assert!(same_origin(&headers(&[
            host,
            ("origin", "https://kanade.test")
        ])));
        for refused in [
            vec![host],
            vec![host, ("sec-fetch-site", "same-site")],
            vec![host, ("sec-fetch-site", "cross-site")],
            vec![host, ("sec-fetch-site", "none")],
            vec![host, ("origin", "https://evil.example")],
            vec![host, ("origin", "http://kanade.test")],
            vec![host, ("origin", "null")],
            vec![
                host,
                ("sec-fetch-site", "same-origin"),
                ("origin", "https://evil.example"),
            ],
        ] {
            assert!(!same_origin(&headers(&refused)), "{refused:?}");
        }
        assert!(same_origin(&headers(&[
            ("host", "localhost:4393"),
            ("origin", "http://localhost:4393")
        ])));
    }

    #[test]
    fn tokens_bind_to_the_session_and_the_realm() {
        let id = "a".repeat(43);
        let mut map = HeaderMap::new();
        map.insert(
            CSRF_HEADER,
            HeaderValue::from_str(&token(ADMIN, &id)).unwrap(),
        );
        assert!(token_matches(ADMIN, &map, &id));
        assert!(!token_matches(ADMIN, &map, &"b".repeat(43)));
        assert!(
            !token_matches(MEMBER, &map, &id),
            "an admin token is not a member token"
        );
        assert_eq!(
            token(ADMIN, &id),
            crypto::keyed_tag(id.as_bytes(), b"kanade-admin-csrf-v1"),
            "admin tokens are unchanged"
        );
        map.append(
            CSRF_HEADER,
            HeaderValue::from_str(&token(ADMIN, &id)).unwrap(),
        );
        assert!(!token_matches(ADMIN, &map, &id), "repeated header");
        assert!(!token_matches(ADMIN, &HeaderMap::new(), &id));
    }
}
