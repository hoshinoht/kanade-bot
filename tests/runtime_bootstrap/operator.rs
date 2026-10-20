//! `kanade portal` and `kanade sessions` against a loopback stub of the
//! admin API: sign-in, the action and sign-out in that order with the
//! listener's Host/Origin, the cookie and CSRF token; sign-out also after a
//! failed action; no secret in any output; exit 69 with nothing attempted
//! when the API is unreachable or unavailable.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Command, Output},
    sync::{Arc, Mutex},
};

use serde_json::{Value, json};

use super::{binary, unused_loopback_port};

const TOKEN: &str = "stub-break-glass-token-0123456789abcdef";
const COOKIE: &str = "stub-session-cookie-value-9876543210";
const CSRF: &str = "stub-csrf-token-value-5555";

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

type Answer = (u16, Vec<(&'static str, String)>, String);

/// A one-request-per-connection HTTP stub on a loopback port.
struct Stub {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Stub {
    fn start(answer: impl Fn(&Seen) -> Answer + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        // Detached: it ends with the test process.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                if let Some(request) = read(&stream) {
                    let (status, headers, body) = answer(&request);
                    log.lock().unwrap().push(request);
                    write(stream, status, &headers, &body);
                }
            }
        });
        Self { port, seen }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn calls(&self) -> Vec<String> {
        self.seen()
            .iter()
            .map(|seen| format!("{} {}", seen.method, seen.path))
            .collect()
    }
}

fn read(stream: &TcpStream) -> Option<Seen> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next()?.to_owned(), parts.next()?.to_owned());
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let (key, value) = line.split_once(':')?;
        headers.push((key.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    let length = headers
        .iter()
        .find(|(key, _)| key == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Seen {
        method,
        path,
        headers,
        body: String::from_utf8(body).ok()?,
    })
}

fn write(mut stream: TcpStream, status: u16, headers: &[(&str, String)], body: &str) {
    let mut text = format!(
        "HTTP/1.1 {status} Stub\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if !body.is_empty() {
        text.push_str("Content-Type: application/json\r\n");
    }
    for (name, value) in headers {
        text.push_str(&format!("{name}: {value}\r\n"));
    }
    text.push_str("\r\n");
    text.push_str(body);
    let _ = stream.write_all(text.as_bytes());
}

fn portal(open: bool) -> String {
    json!({"self_service": {"mode": "cards_and_link", "effective_mode": "cards_and_link", "public_portal": open}})
        .to_string()
}

/// The admin API as the commands use it.
fn api(seen: &Seen) -> Answer {
    match (seen.method.as_str(), seen.path.as_str()) {
        ("POST", "/api/admin/auth/token") => (
            200,
            vec![
                (
                    "Set-Cookie",
                    format!("__Host-kanade_admin={COOKIE}; Path=/; Secure; HttpOnly; SameSite=Strict"),
                ),
                ("X-Kanade-CSRF", CSRF.into()),
            ],
            json!({"display": "Break-glass token", "method": "token"}).to_string(),
        ),
        ("POST", "/api/admin/auth/logout") => (204, Vec::new(), String::new()),
        ("GET", "/api/admin/config") => (200, Vec::new(), portal(false)),
        ("PATCH", "/api/admin/config") => {
            let open = seen.json()["self_service"]["public_portal"] == true;
            (200, Vec::new(), portal(open))
        }
        ("GET", "/api/admin/me/sessions") => (
            200,
            Vec::new(),
            json!({"sessions": [
                {"handle": "aaaaaaaaaaaaaaaaaaaaaaaa", "method": "token", "device": null, "signed_in_at": "2026-10-11T00:00:00Z", "last_seen_at": "2026-10-11T00:00:00Z", "current": true},
                {"handle": "bbbbbbbbbbbbbbbbbbbbbbbb", "method": "token", "device": null, "signed_in_at": "2026-10-10T00:00:00Z", "last_seen_at": "2026-10-10T01:00:00Z", "current": false}
            ], "generated_at": "2026-10-11T00:00:00Z"})
            .to_string(),
        ),
        ("POST", "/api/admin/me/sessions/sign-out-others") => {
            (200, Vec::new(), json!({"ended": 2}).to_string())
        }
        ("DELETE", "/api/admin/me/sessions/bbbbbbbbbbbbbbbbbbbbbbbb") => {
            (204, Vec::new(), String::new())
        }
        ("DELETE", _) => (
            404,
            Vec::new(),
            json!({"error": "not_found", "message": "That session has already ended."})
                .to_string(),
        ),
        _ => (404, Vec::new(), String::new()),
    }
}

struct Run {
    _dir: TokenDir,
    output: Output,
}

impl Run {
    fn code(&self) -> Option<i32> {
        self.output.status.code()
    }

    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.output.stdout).into_owned()
    }

    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.output.stderr).into_owned()
    }

    /// Neither stream carries the token, the cookie or the CSRF token.
    fn assert_no_secrets(&self) {
        let all = self.stdout() + &self.stderr();
        for secret in [TOKEN, COOKIE, CSRF] {
            assert!(!all.contains(secret), "{secret} leaked: {all}");
        }
    }
}

struct TokenDir(PathBuf);

impl Drop for TokenDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn kanade(args: &[&str], env: &[(&str, &str)]) -> Run {
    let root = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("kanade-operator-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("admin_token"), format!("{TOKEN}\n")).unwrap();
    let output = Command::new(binary())
        .args(args)
        .env_clear()
        .env("KANADE_SECRETS_DIR", &root)
        .envs(env.iter().copied())
        .output()
        .unwrap();
    Run {
        _dir: TokenDir(root),
        output,
    }
}

#[test]
fn portal_open_signs_in_patches_with_the_session_and_signs_out() {
    let stub = Stub::start(api);
    let run = kanade(&["portal", "open", "--url", &stub.url()], &[]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stdout(), "public portal switch: open\n");
    run.assert_no_secrets();
    assert_eq!(
        stub.calls(),
        [
            "POST /api/admin/auth/token",
            "PATCH /api/admin/config",
            "POST /api/admin/auth/logout"
        ]
    );
    let seen = stub.seen();
    let host = format!("localhost:{}", stub.port);
    let origin = format!("http://{host}");
    for request in &seen {
        assert_eq!(request.header("host"), Some(host.as_str()));
        assert_eq!(request.header("origin"), Some(origin.as_str()));
    }
    assert_eq!(seen[0].json(), json!({"token": TOKEN}));
    assert_eq!(seen[0].header("cookie"), None);
    let session = format!("__Host-kanade_admin={COOKIE}");
    for request in &seen[1..] {
        assert_eq!(request.header("cookie"), Some(session.as_str()));
        assert_eq!(request.header("x-kanade-csrf"), Some(CSRF));
    }
    assert_eq!(
        seen[1].json(),
        json!({"self_service": {"public_portal": true}})
    );
    assert!(
        seen[1]
            .header("idempotency-key")
            .is_some_and(|key| key.starts_with("kanade-cli-")),
        "{:?}",
        seen[1].headers
    );
}

#[test]
fn the_admin_host_names_host_and_https_origin() {
    let stub = Stub::start(api);
    let run = kanade(
        &["portal", "status", "--url", &stub.url()],
        &[("KANADE_ADMIN_HOST", "kanade.test")],
    );
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(run.stdout(), "public portal switch: closed\n");
    for request in stub.seen() {
        assert_eq!(request.header("host"), Some("kanade.test"));
        assert_eq!(request.header("origin"), Some("https://kanade.test"));
    }
    assert_eq!(
        stub.calls(),
        [
            "POST /api/admin/auth/token",
            "GET /api/admin/config",
            "POST /api/admin/auth/logout"
        ]
    );
}

#[test]
fn a_failed_action_still_signs_out_and_redacts_echoed_secrets() {
    let stub = Stub::start(|seen| match (seen.method.as_str(), seen.path.as_str()) {
        ("PATCH", _) => (
            500,
            Vec::new(),
            json!({"error": "unavailable", "message": format!("echo {TOKEN} {COOKIE} {CSRF}")})
                .to_string(),
        ),
        _ => api(seen),
    });
    let run = kanade(&["portal", "close", "--url", &stub.url()], &[]);
    assert_eq!(run.code(), Some(69), "{}", run.stderr());
    run.assert_no_secrets();
    assert!(run.stderr().contains("[redacted]"), "{}", run.stderr());
    assert!(run.stdout().is_empty());
    assert_eq!(
        stub.calls(),
        [
            "POST /api/admin/auth/token",
            "PATCH /api/admin/config",
            "POST /api/admin/auth/logout"
        ]
    );

    let run = kanade(
        &[
            "sessions",
            "revoke",
            "cccccccccccccccccccccccc",
            "--url",
            &stub.url(),
        ],
        &[],
    );
    assert_eq!(run.code(), Some(64), "{}", run.stderr());
    assert!(run.stderr().contains("That session has already ended."));
    assert_eq!(
        stub.calls()[3..],
        [
            "POST /api/admin/auth/token",
            "DELETE /api/admin/me/sessions/cccccccccccccccccccccccc",
            "POST /api/admin/auth/logout"
        ]
    );
}

/// A panic inside the signed-in work still signs out exactly once, and is
/// reported as a failure rather than unwinding past the sign-out.
#[cfg(feature = "test-support")]
#[test]
fn a_panicking_action_still_signs_out_once() {
    let stub = Stub::start(api);
    let root = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("kanade-operator-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let _dir = TokenDir(root.clone());
    std::fs::write(root.join("admin_token"), format!("{TOKEN}\n")).unwrap();
    let config = kanade::runtime::config::AdminClientConfig {
        target: format!("127.0.0.1:{}", stub.port).parse().unwrap(),
        authority: format!("localhost:{}", stub.port),
        origin: format!("http://localhost:{}", stub.port),
        token: kanade::runtime::secrets::SecretFile::new(
            &root,
            kanade::runtime::secrets::ADMIN_TOKEN,
        ),
    };
    let outcome = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(kanade::cli::admin::signed_in(&config, async |_session| {
            panic!("the action panicked")
        }));
    let error = outcome.unwrap_err();
    assert_eq!(error.exit_code(), 69, "{error}");
    assert_eq!(
        stub.calls(),
        ["POST /api/admin/auth/token", "POST /api/admin/auth/logout"]
    );
    let seen = stub.seen();
    assert_eq!(
        seen[1].header("cookie"),
        Some(format!("__Host-kanade_admin={COOKIE}").as_str())
    );
    assert_eq!(seen[1].header("x-kanade-csrf"), Some(CSRF));
}

#[test]
fn an_unavailable_or_refusing_api_is_never_asked_to_change_anything() {
    // Nothing listens there.
    let port = unused_loopback_port();
    let run = kanade(
        &[
            "portal",
            "open",
            "--url",
            &format!("http://127.0.0.1:{port}"),
        ],
        &[],
    );
    assert_eq!(run.code(), Some(69), "{}", run.stderr());
    assert!(run.stderr().contains("unreachable"), "{}", run.stderr());
    run.assert_no_secrets();

    for (status, code) in [(503, 69), (401, 78)] {
        let stub = Stub::start(move |_| {
            (
                status,
                Vec::new(),
                json!({"error": "nope", "message": "No."}).to_string(),
            )
        });
        let run = kanade(&["sessions", "sign-out-all", "--url", &stub.url()], &[]);
        assert_eq!(run.code(), Some(code), "{status}: {}", run.stderr());
        assert_eq!(stub.calls(), ["POST /api/admin/auth/token"], "{status}");
        run.assert_no_secrets();
    }

    // A plain token variable is refused before anything is sent.
    let stub = Stub::start(api);
    let run = kanade(
        &["portal", "status", "--url", &stub.url()],
        &[("KANADE_ADMIN_TOKEN", TOKEN)],
    );
    assert_eq!(run.code(), Some(78));
    assert!(stub.calls().is_empty());
    run.assert_no_secrets();
}

#[test]
fn sessions_commands_list_revoke_and_sign_out_everything() {
    let stub = Stub::start(api);
    let run = kanade(&["sessions", "list", "--url", &stub.url()], &[]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    let stdout = run.stdout();
    assert!(
        stdout.starts_with("2 live session(s) of the break-glass token:\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("aaaaaaaaaaaaaaaaaaaaaaaa  token  -  signed in"),
        "{stdout}"
    );
    assert!(stdout.contains("(this command)"), "{stdout}");

    let run = kanade(
        &[
            "sessions",
            "revoke",
            "bbbbbbbbbbbbbbbbbbbbbbbb",
            "--url",
            &stub.url(),
        ],
        &[],
    );
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stdout(),
        "session bbbbbbbbbbbbbbbbbbbbbbbb signed out\n"
    );

    let run = kanade(&["sessions", "sign-out-all", "--url", &stub.url()], &[]);
    assert_eq!(run.code(), Some(0), "{}", run.stderr());
    assert_eq!(
        run.stdout(),
        "signed out 2 other session(s) and this command's own\n"
    );
    run.assert_no_secrets();
    assert_eq!(
        stub.calls()[6..],
        [
            "POST /api/admin/auth/token",
            "POST /api/admin/me/sessions/sign-out-others",
            "POST /api/admin/auth/logout"
        ]
    );
}
