//! Cookie, query-string and form encoding helpers for the auth routes (axum's
//! `query`/`form` features are not enabled), plus the browser-flow responses
//! both realms' sign-in routes answer with.

use axum::{
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE},
    },
    response::{IntoResponse, Response},
};

/// The admin realm's session and pre-auth cookies.
pub const SESSION_COOKIE: &str = "__Host-kanade_admin";
pub const LOGIN_COOKIE: &str = "__Host-kanade_admin_login";
/// The member realm's (public origin) session and pre-auth cookies.
pub const MEMBER_SESSION_COOKIE: &str = "__Host-kanade_pub";
pub const MEMBER_LOGIN_COOKIE: &str = "__Host-kanade_pub_login";

/// The one value of `name`; absent or ambiguous (repeated with different values) is `None`.
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut found: Option<&str> = None;
    for header in headers.get_all(COOKIE) {
        let Ok(header) = header.to_str() else {
            continue;
        };
        for pair in header.split(';') {
            let Some((key, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if key != name {
                continue;
            }
            match found {
                Some(previous) if previous != value => return None,
                _ => found = Some(value),
            }
        }
    }
    found.filter(|value| is_token(value)).map(str::to_owned)
}

/// Our ids are 43 base64url characters; anything else never reaches the store.
pub fn is_token(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// `__Host-` cookies: Secure, Path=/, no Domain.
pub fn set_cookie(name: &str, value: &str, same_site: &str, max_age_seconds: i64) -> HeaderValue {
    let text = format!(
        "{name}={value}; Path=/; Secure; HttpOnly; SameSite={same_site}; Max-Age={}",
        max_age_seconds.max(0)
    );
    HeaderValue::from_str(&text).unwrap_or_else(|_| HeaderValue::from_static(""))
}

pub fn clear_cookie(name: &str, same_site: &str) -> HeaderValue {
    set_cookie(name, "", same_site, 0)
}

/// Decoded query pairs; malformed percent escapes drop the pair.
pub fn query_pairs(query: Option<&str>) -> Vec<(String, String)> {
    query
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some((decode(key)?, decode(value)?))
        })
        .collect()
}

/// The single value of `key`; repeated keys are ambiguous and refused.
pub fn query_value(pairs: &[(String, String)], key: &str) -> Option<String> {
    let mut values = pairs.iter().filter(|(name, _)| name == key);
    let first = values.next()?;
    values.next().is_none().then(|| first.1.clone())
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hex = text.get(index + 1..index + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// `application/x-www-form-urlencoded` / query component encoding.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

pub fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// A post-login destination: a same-origin absolute path only (no scheme,
/// authority, `//`, backslashes, control characters or API paths); else `/`.
pub fn safe_next(next: Option<&str>) -> String {
    let Some(next) = next else {
        return "/".into();
    };
    let ok = next.starts_with('/')
        && !next.starts_with("//")
        && !next.starts_with("/api/")
        && next.len() <= 512
        && next
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'\\');
    if ok { next.to_owned() } else { "/".into() }
}

pub fn see_other(location: &str, cookies: impl IntoIterator<Item = HeaderValue>) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    if let Ok(location) = HeaderValue::from_str(location) {
        headers.insert(LOCATION, location);
    }
    for cookie in cookies {
        headers.append(SET_COOKIE, cookie);
    }
    response
}

pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// A same-origin page that navigates to `next`. A `SameSite=Strict` cookie
/// set during Discord's cross-site redirect is not sent on a redirect chain
/// that started cross-site; a navigation started by our own page is
/// same-site, so the landing request carries the session. No script: the
/// CSP (`default-src 'none'`) does not govern meta refresh.
pub fn landing(next: &str, cookies: impl IntoIterator<Item = HeaderValue>) -> Response {
    let next = escape_html(next);
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"refresh\" content=\"0; url={next}\"><title>Signed in</title></head>\
         <body><p><a href=\"{next}\">Continue to Kanade</a></p></body></html>"
    );
    let mut response = (
        StatusCode::OK,
        [(CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response();
    for cookie in cookies {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_stays_on_this_origin() {
        for good in ["/", "/week?week=next", "/runs/r1#sheet"] {
            assert_eq!(safe_next(Some(good)), good);
        }
        for bad in [
            "https://evil.example/",
            "//evil.example/",
            "/\\evil.example",
            "evil.example",
            "/api/admin/auth/logout",
            "/week\r\nSet-Cookie: x=1",
            "/ space",
            "",
        ] {
            assert_eq!(safe_next(Some(bad)), "/", "{bad:?}");
        }
        assert_eq!(safe_next(None), "/");
    }

    #[test]
    fn cookies_must_be_single_well_formed_ids() {
        let id = "a".repeat(43);
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&format!("x=1; {SESSION_COOKIE}={id}")).unwrap(),
        );
        assert_eq!(cookie(&headers, SESSION_COOKIE), Some(id.clone()));
        headers.append(
            COOKIE,
            HeaderValue::from_str(&format!("{SESSION_COOKIE}={}", "b".repeat(43))).unwrap(),
        );
        assert_eq!(cookie(&headers, SESSION_COOKIE), None, "conflicting values");
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("__Host-kanade_admin=short"),
        );
        assert_eq!(cookie(&headers, SESSION_COOKIE), None);
    }

    #[test]
    fn query_and_form_encoding_round_trip() {
        let pairs = query_pairs(Some("code=a%2Bb&state=x+y&bad=%zz&dup=1&dup=2"));
        assert_eq!(query_value(&pairs, "code").as_deref(), Some("a+b"));
        assert_eq!(query_value(&pairs, "state").as_deref(), Some("x y"));
        assert_eq!(query_value(&pairs, "bad"), None);
        assert_eq!(query_value(&pairs, "dup"), None);
        assert_eq!(
            form(&[("redirect_uri", "https://k.example/cb?x=1"), ("a b", "é")]),
            "redirect_uri=https%3A%2F%2Fk.example%2Fcb%3Fx%3D1&a%20b=%C3%A9"
        );
    }
}
