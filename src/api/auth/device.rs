//! A session's device label ("Firefox · macOS") from the sign-in request's
//! `User-Agent`. Only the label is stored, never the header: it names a
//! browser and a system from fixed lists, so nothing the client sends is
//! echoed back. Unrecognised agents get no label.

use axum::http::{HeaderMap, header::USER_AGENT};

/// Agents longer than this are not read at all.
const MAX_AGENT: usize = 512;

/// First match wins, so engines other browsers also claim come last.
const BROWSERS: [(&str, &str); 8] = [
    ("Edg/", "Edge"),
    ("EdgiOS/", "Edge"),
    ("OPR/", "Opera"),
    ("Firefox/", "Firefox"),
    ("FxiOS/", "Firefox"),
    ("CriOS/", "Chrome"),
    ("Chrome/", "Chrome"),
    ("Safari/", "Safari"),
];

const SYSTEMS: [(&str, &str); 7] = [
    ("iPhone", "iPhone"),
    ("iPad", "iPad"),
    ("Android", "Android"),
    ("CrOS", "ChromeOS"),
    ("Mac OS X", "macOS"),
    ("Windows", "Windows"),
    ("Linux", "Linux"),
];

fn first(agent: &str, table: &[(&str, &'static str)]) -> Option<&'static str> {
    table
        .iter()
        .find(|(needle, _)| agent.contains(needle))
        .map(|(_, name)| *name)
}

/// "Browser · system", either alone when only one is known; `None` for none.
pub fn label(headers: &HeaderMap) -> Option<String> {
    let agent = headers.get(USER_AGENT)?.to_str().ok()?;
    if agent.len() > MAX_AGENT {
        return None;
    }
    match (first(agent, &BROWSERS), first(agent, &SYSTEMS)) {
        (Some(browser), Some(system)) => Some(format!("{browser} · {system}")),
        (Some(one), None) | (None, Some(one)) => Some(one.to_owned()),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn of(agent: &str) -> Option<String> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_str(agent).unwrap());
        label(&headers)
    }

    #[test]
    fn common_agents_get_a_browser_and_a_system() {
        for (agent, want) in [
            (
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:131.0) Gecko/20100101 Firefox/131.0",
                "Firefox · macOS",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1",
                "Safari · iPhone",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36",
                "Chrome · Windows",
            ),
            (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Safari/537.36 Edg/129.0.0.0",
                "Edge · Windows",
            ),
            (
                "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/129.0.0.0 Mobile Safari/537.36",
                "Chrome · Android",
            ),
            (
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/129.0 Mobile/15E148 Safari/604.1",
                "Chrome · iPhone",
            ),
        ] {
            assert_eq!(of(agent).as_deref(), Some(want), "{agent}");
        }
    }

    #[test]
    fn unknown_or_oversized_agents_get_no_label() {
        assert_eq!(of("curl/8.7.1"), None);
        assert_eq!(of("<script>alert(1)</script>"), None);
        assert_eq!(of(&format!("Firefox/1 {}", "x".repeat(MAX_AGENT))), None);
        assert_eq!(label(&HeaderMap::new()), None);
        assert_eq!(of("SomeBot (Linux)").as_deref(), Some("Linux"));
    }
}
