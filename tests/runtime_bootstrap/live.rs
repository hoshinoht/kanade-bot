//! Live `serve` (Discord not wired): a temp store, break-glass sign-in and
//! the admin API over real state, run as the shipped binary.

use std::{
    fs,
    io::{Read, Write},
    net::SocketAddr,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

use super::{ChildTimeout, binary, request, unused_loopback_port};

const ADMIN_TOKEN: &str = "live-serve-break-glass-token-0123456789abcdef";

struct Live {
    root: PathBuf,
}

impl Live {
    fn new() -> Self {
        let base = fs::canonicalize(std::env::temp_dir()).unwrap();
        let root = base.join(format!("kanade-live-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        // Mixed case: CI's filesystem is case-sensitive.
        let bundles = root.join("Personas/bundles");
        fs::create_dir_all(&bundles).unwrap();
        fs::copy(
            repo.join("config/personas/bundles/kanade.yaml"),
            bundles.join("kanade.yaml"),
        )
        .unwrap();
        fs::write(root.join("discord_token"), "bot-token-value\n").unwrap();
        fs::write(root.join("admin_token"), format!("{ADMIN_TOKEN}\n")).unwrap();
        fs::write(root.join("client_secret"), "discord-client-secret\n").unwrap();
        Self { root }
    }

    fn command(&self, admin: u16, public: Option<u16>) -> Command {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = |relative: &str| self.root.join(relative).display().to_string();
        let mut command = Command::new(binary());
        command
            .arg("serve")
            .env_clear()
            .env("KANADE_TIMEZONE", "Asia/Kuala_Lumpur")
            .env("KANADE_ADMIN_BIND", format!("127.0.0.1:{admin}"))
            .env("KANADE_DISCORD_TOKEN_FILE", path("discord_token"))
            .env("KANADE_ADMIN_TOKEN_FILE", path("admin_token"))
            .env("KANADE_GUILD_ID", "900")
            .env("KANADE_BOSSING_ROLE_ID", "10")
            .env("KANADE_ADMIN_ROLE_ID", "20")
            // Neither directory exists yet: serve creates both 0700.
            .env("KANADE_DB_PATH", path("db/kanade.sqlite3"))
            .env("KANADE_OWNER_LOCK_DIR", path("locks"))
            .env(
                "KANADE_CATALOG_FILE",
                repo.join("boss/bosses.yaml").display().to_string(),
            )
            .env(
                "KANADE_KNOWLEDGE_DIR",
                repo.join("boss/knowledge").display().to_string(),
            )
            .env("KANADE_PERSONA_DIR", path("Personas"))
            .env("KANADE_WATCH_CATEGORY_IDS", "41,42")
            .env("KANADE_CHAT_CATEGORY_IDS", "43")
            .env("KANADE_BOSS_WEEK_RESET_WEEKDAY", "wed")
            .env("KANADE_BOSS_WEEK_RESET_TIME", "08:00")
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(public) = public {
            command
                .env("KANADE_PUBLIC_BIND", format!("127.0.0.1:{public}"))
                .env("KANADE_PUBLIC_HOST", "kanade-pub.test");
        }
        command
    }

    fn spawn(&self, admin: u16, public: Option<u16>) -> Child {
        self.command(admin, public).spawn().unwrap()
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Reply {
    status: u16,
    head: String,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case(name).then(|| value.trim())
        })
    }

    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| panic!("not JSON: {}", self.body))
    }
}

fn call(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Reply {
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    let mut text = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        text.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    text.push_str("\r\n");
    text.push_str(body.unwrap_or_default());
    stream.write_all(text.as_bytes()).unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").expect("response head");
    Reply {
        status: head.split(' ').nth(1).unwrap().parse().unwrap(),
        head: head.to_owned(),
        body: body.to_owned(),
    }
}

fn terminate(mut child: Child) -> Output {
    Command::new("/bin/kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(
        child.wait_timeout(Duration::from_secs(10)).is_some(),
        "server did not drain after termination"
    );
    child.wait_with_output().unwrap()
}

/// The store opens and the files load before the listener binds.
fn ready(address: SocketAddr) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    while std::net::TcpStream::connect(address).is_err() {
        assert!(Instant::now() < deadline, "server never became ready");
        std::thread::sleep(Duration::from_millis(50));
    }
    request(address, "localhost", "/healthz")
}

#[test]
fn live_serve_signs_in_with_the_token_and_reads_the_real_store() {
    let live = Live::new();
    let (admin_port, public_port) = (unused_loopback_port(), unused_loopback_port());
    let admin = SocketAddr::from(([127, 0, 0, 1], admin_port));
    let public = SocketAddr::from(([127, 0, 0, 1], public_port));
    let server = live.spawn(admin_port, Some(public_port));

    let health = ready(admin);
    assert!(health.starts_with("HTTP/1.1 200"), "{health}");
    for field in [
        r#""mode":"live""#,
        r#""storage":"ok""#,
        r#""discord":"disabled""#,
    ] {
        assert!(health.contains(field), "{health}");
    }
    let healthcheck = Command::new(binary())
        .args(["healthcheck", "--url", &format!("http://{admin}/healthz")])
        .env_clear()
        .status()
        .unwrap();
    assert!(healthcheck.success());

    let login = call(
        admin,
        "POST",
        "/api/admin/auth/token",
        &[("Sec-Fetch-Site", "same-origin")],
        Some(&format!(r#"{{"token":"{ADMIN_TOKEN}"}}"#)),
    );
    assert_eq!(login.status, 200, "{}", login.body);
    assert_eq!(login.json()["method"], "token");
    let cookie = login
        .header("set-cookie")
        .and_then(|value| value.split(';').next())
        .unwrap()
        .to_owned();
    let session = call(
        admin,
        "GET",
        "/api/admin/session",
        &[("Cookie", &cookie)],
        None,
    );
    assert_eq!(session.status, 200, "{}", session.body);
    assert_eq!(
        session.json(),
        serde_json::json!({"display": "Break-glass token", "method": "token"})
    );
    let week = call(
        admin,
        "GET",
        "/api/admin/week",
        &[("Cookie", &cookie)],
        None,
    );
    assert_eq!(week.status, 200, "{}", week.body);
    assert!(week.json().is_object());
    let channels = call(
        admin,
        "GET",
        "/api/admin/channels",
        &[("Cookie", &cookie)],
        None,
    );
    assert_eq!(channels.status, 200, "{}", channels.body);

    // Admin routes are never mounted on the public listener.
    for path in ["/api/admin/week", "/api/admin/session", "/healthz"] {
        let reply = request(public, "kanade-pub.test", path);
        assert!(reply.starts_with("HTTP/1.1 404"), "{path}: {reply}");
    }

    // One owner per store: a second process is refused and exits.
    let second = live.command(unused_loopback_port(), None).output().unwrap();
    assert!(!second.status.success());
    let stderr = String::from_utf8(second.stderr).unwrap();
    assert!(
        stderr.contains("KANADE_DB_PATH is already owned by another kanade process"),
        "{stderr}"
    );
    assert!(!stderr.contains(live.root.to_str().unwrap()), "{stderr}");

    let output = terminate(server);
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(r#""event":"store_closed""#), "{stderr}");
    assert!(!stderr.contains("store_dropped_unclosed"), "{stderr}");
    assert!(!stderr.contains(ADMIN_TOKEN) && !stderr.contains("bot-token-value"));

    // The lock went with the process; sessions survived in the store.
    let port = unused_loopback_port();
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let restarted = live.spawn(port, None);
    assert!(ready(address).starts_with("HTTP/1.1 200"));
    let session = call(
        address,
        "GET",
        "/api/admin/session",
        &[("Cookie", &cookie)],
        None,
    );
    assert_eq!(session.status, 200, "{}", session.body);
    assert!(terminate(restarted).status.success());
}

#[test]
fn live_serve_builds_discord_sign_in_without_network() {
    let live = Live::new();
    let port = unused_loopback_port();
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let host = format!("localhost:{port}");
    let mut command = live.command(port, None);
    command
        .env("KANADE_ADMIN_HOST", &host)
        .env("KANADE_ADMIN_DISCORD_CLIENT_ID", "4242")
        .env(
            "KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE",
            live.root.join("client_secret"),
        )
        .env(
            "KANADE_ADMIN_DISCORD_REDIRECT_URI",
            format!("http://{host}/api/admin/auth/discord/callback"),
        );
    let server = command.spawn().unwrap();
    assert!(ready(address).starts_with("HTTP/1.1 200"));
    let methods = request(address, &host, "/api/admin/auth/methods");
    assert!(methods.starts_with("HTTP/1.1 200"), "{methods}");
    assert!(
        methods.contains(r#"{"discord":true,"tailscale":false,"token":true}"#),
        "{methods}"
    );
    let output = terminate(server);
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("discord-client-secret"), "{stderr}");
}

#[test]
fn live_serve_refuses_a_missing_bot_token_file_before_opening_the_store() {
    let live = Live::new();
    fs::remove_file(live.root.join("discord_token")).unwrap();
    let output = live.command(unused_loopback_port(), None).output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("KANADE_DISCORD_TOKEN_FILE"), "{stderr}");
    assert!(!live.root.join("db").exists(), "store opened anyway");
}
