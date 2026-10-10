use std::net::IpAddr;

use hyper::Uri;
use rustls::pki_types::ServerName;

use super::config::HttpConfigError;

#[derive(Clone, Debug)]
pub(crate) struct Endpoint {
    pub host: String,
    pub port: u16,
    /// `Host` header value: the authority without userinfo.
    pub authority: String,
    /// `Some` for `https`.
    pub server_name: Option<ServerName<'static>>,
    /// Plain `localhost` must resolve to loopback addresses only.
    pub loopback_only: bool,
    base_path: String,
}

impl Endpoint {
    pub fn parse(base_url: &str) -> Result<Self, HttpConfigError> {
        let invalid = HttpConfigError::InvalidBaseUrl;
        let uri: Uri = base_url.trim().parse().map_err(|_| invalid)?;
        let authority = uri.authority().ok_or(invalid)?;
        if authority.as_str().contains('@') || uri.query().is_some() {
            return Err(invalid);
        }
        let raw_host = authority.host();
        let host = raw_host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(raw_host)
            .to_ascii_lowercase();
        if host.is_empty() {
            return Err(invalid);
        }
        let ip = host.parse::<IpAddr>().ok();
        let (port, server_name) = match uri.scheme_str() {
            Some("https") => {
                let name = ServerName::try_from(host.clone()).map_err(|_| invalid)?;
                (authority.port_u16().unwrap_or(443), Some(name))
            }
            Some("http") => {
                let local = ip.is_some_and(|ip| ip.is_loopback())
                    || host == "localhost"
                    || host == "host.docker.internal";
                if !local {
                    return Err(HttpConfigError::InsecureHttp);
                }
                (authority.port_u16().unwrap_or(80), None)
            }
            _ => return Err(invalid),
        };
        let path = uri.path().trim_end_matches('/');
        let base_path = format!("{}/v1", path.strip_suffix("/v1").unwrap_or(path));
        if base_path.contains("//") {
            return Err(invalid);
        }
        Ok(Self {
            loopback_only: server_name.is_none() && host == "localhost",
            authority: authority.as_str().to_owned(),
            host,
            port,
            server_name,
            base_path,
        })
    }

    pub fn path(&self, suffix: &str) -> String {
        format!("{}/{suffix}", self.base_path)
    }
}

#[cfg(test)]
mod tests {
    use super::{Endpoint, HttpConfigError};

    fn chat(url: &str) -> String {
        Endpoint::parse(url).unwrap().path("chat/completions")
    }

    #[test]
    fn base_path_trims_one_v1_and_never_doubles_slashes() {
        assert_eq!(chat("http://127.0.0.1:11434"), "/v1/chat/completions");
        assert_eq!(chat("http://127.0.0.1:11434/"), "/v1/chat/completions");
        assert_eq!(chat("http://localhost:11434/v1"), "/v1/chat/completions");
        assert_eq!(chat("http://localhost:11434/v1/"), "/v1/chat/completions");
        assert_eq!(
            chat("https://gw.example/api/v1"),
            "/api/v1/chat/completions"
        );
        assert_eq!(chat("https://gw.example/v1/v1"), "/v1/v1/chat/completions");
        assert_eq!(chat("http://[::1]:8080/v1"), "/v1/chat/completions");
        assert_eq!(
            Endpoint::parse("https://gw.example//v1").unwrap_err(),
            HttpConfigError::InvalidBaseUrl
        );
    }

    #[test]
    fn plain_http_is_limited_to_local_hosts() {
        for url in [
            "http://127.0.0.1",
            "http://127.8.9.10:1",
            "http://[::1]",
            "http://LOCALHOST:11434",
            "http://host.docker.internal:11434/v1",
        ] {
            assert!(Endpoint::parse(url).is_ok(), "{url}");
        }
        for url in [
            "http://10.0.0.5:11434",
            "http://192.168.1.2",
            "http://gw.example",
            "http://localhost.example",
            "http://0.0.0.0",
        ] {
            assert_eq!(
                Endpoint::parse(url).unwrap_err(),
                HttpConfigError::InsecureHttp,
                "{url}"
            );
        }
        for url in [
            "ftp://127.0.0.1",
            "127.0.0.1:11434",
            "https://user:secret@gw.example",
            "https://gw.example/v1?x=1",
            "",
        ] {
            assert_eq!(
                Endpoint::parse(url).unwrap_err(),
                HttpConfigError::InvalidBaseUrl,
                "{url}"
            );
        }
    }
}
