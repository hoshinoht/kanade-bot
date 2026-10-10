use std::{
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use axum::Router;
use kanade::{
    api::listeners::{Site, router},
    runtime::config::HttpConfig,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

pub const ADMIN_HOST: &str = "kanade.test";
pub const PUBLIC_HOST: &str = "kanade-pub.test";
pub const SECRET: &str = "outside-the-web-root";

/// Synthetic web builds, boss art and a secret outside every served root.
pub struct Fixture {
    pub root: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!("kanade-api-{}", uuid::Uuid::new_v4()));
        let write = |relative: &str, contents: &[u8]| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        };
        write(
            "web/apps/admin/dist/index.html",
            b"<!doctype html>admin shell",
        );
        write(
            "web/apps/admin/dist/assets/app-abc123.js",
            b"console.log(1)",
        );
        write("web/apps/admin/dist/manifest.webmanifest", b"{}");
        write(
            "web/apps/public/dist/index.html",
            b"<!doctype html>public shell",
        );
        write(
            "web/apps/public/dist/offline.html",
            b"<!doctype html>napping",
        );
        write("web/apps/admin/secret.txt", SECRET.as_bytes());
        write("secret.txt", SECRET.as_bytes());
        write("boss/portraits/Carling.png", b"\x89PNG portrait");
        write("boss/artwork/entry/Carling.webp", b"RIFF entry");
        write("boss/secret.png", SECRET.as_bytes());
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.join("secret.txt"),
            root.join("web/apps/admin/dist/assets/escape.js"),
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.join("secret.txt"),
            root.join("boss/portraits/Escape.png"),
        )
        .unwrap();
        Self { root }
    }

    pub fn http(&self) -> HttpConfig {
        HttpConfig {
            admin_host: Some(ADMIN_HOST.into()),
            public_host: Some(PUBLIC_HOST.into()),
            trusted_proxy: None,
            cloudflared_peer: None,
            web_dir: Some(self.root.join("web")),
            boss_dir: Some(self.root.join("boss")),
            identity_dir: None,
            edge_secret_file: None,
        }
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub async fn spawn_router(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    address
}

pub async fn spawn(site: Site) -> SocketAddr {
    spawn_router(router(site)).await
}

pub async fn admin(http: &HttpConfig) -> SocketAddr {
    spawn(Site::admin(http)).await
}

pub async fn public(http: &HttpConfig) -> SocketAddr {
    spawn(Site::public(http).expect("public host configured")).await
}

#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub fn all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .collect()
    }

    /// The value a `Set-Cookie` gives `name` (empty when it clears it).
    pub fn cookie(&self, name: &str) -> Option<String> {
        self.all("set-cookie").into_iter().find_map(|line| {
            let (pair, _) = line.split_once(';').unwrap_or((line, ""));
            let (key, value) = pair.split_once('=')?;
            (key == name).then(|| value.to_owned())
        })
    }

    /// Where the browser goes next: a `303` Location, or the landing page's meta refresh target.
    pub fn destination(&self) -> Option<String> {
        if self.status == 303 {
            return self.header("location").map(str::to_owned);
        }
        let text = self.text();
        let start = text.find("url=")? + 4;
        let end = start + text[start..].find('"')?;
        Some(
            text[start..end]
                .replace("&#39;", "'")
                .replace("&quot;", "\"")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&"),
        )
    }

    /// Every header and the body, for leak checks.
    pub fn dump(&self) -> String {
        let headers: Vec<String> = self
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect();
        format!("{}\n{}", headers.join("\n"), self.text())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|_| panic!("not JSON: {}", self.text()))
    }

    /// An `ApiError` body with exactly `{error, message}`.
    pub fn api_error(&self) -> String {
        let value = self.json();
        let object = value.as_object().expect("error object");
        assert_eq!(object.len(), 2, "{value}");
        assert!(object["message"].is_string());
        object["error"].as_str().unwrap().to_owned()
    }
}

/// Sends raw bytes so tests can craft malformed or duplicated headers.
pub async fn raw(address: SocketAddr, request: &[u8]) -> Reply {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream.write_all(request).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    let split = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("complete response head");
    let head = String::from_utf8(bytes[..split].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .map(|line| {
            let (name, value) = line.split_once(':').unwrap();
            (name.to_ascii_lowercase(), value.trim().to_owned())
        })
        .collect::<Vec<_>>();
    assert!(
        !headers.iter().any(|(name, _)| name == "transfer-encoding"),
        "chunked replies are not expected"
    );
    Reply {
        status,
        headers,
        body: bytes[split + 4..].to_vec(),
    }
}

pub async fn request(
    address: SocketAddr,
    method: &str,
    host: &str,
    path: &str,
    extra: &[(&str, &str)],
) -> Reply {
    send(address, method, host, path, extra, None).await
}

/// A request with an optional JSON body (`Content-Type`/`Content-Length` added).
pub async fn send(
    address: SocketAddr,
    method: &str,
    host: &str,
    path: &str,
    extra: &[(&str, &str)],
    json: Option<&str>,
) -> Reply {
    let mut text = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (name, value) in extra {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = json {
        text.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ));
    } else {
        text.push_str("\r\n");
    }
    raw(address, text.as_bytes()).await
}

pub async fn get(address: SocketAddr, host: &str, path: &str) -> Reply {
    request(address, "GET", host, path, &[]).await
}

pub fn read(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap()
}
