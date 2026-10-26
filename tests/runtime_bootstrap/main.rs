mod backup;
mod live;
mod operator;
mod restore;

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Server tests run one at a time: a port picked by one test can be released
/// and handed to another test's server while it is still shutting down.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn binary() -> String {
    std::env::var("CARGO_BIN_EXE_kanade").expect("Cargo exposes the test binary")
}

fn unused_loopback_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Two distinct ports: both listeners are held at once, since a port released
/// by the first call can be handed straight back by the second.
fn unused_loopback_port_pair() -> (u16, u16) {
    let first = TcpListener::bind("127.0.0.1:0").unwrap();
    let second = TcpListener::bind("127.0.0.1:0").unwrap();
    (
        first.local_addr().unwrap().port(),
        second.local_addr().unwrap().port(),
    )
}

fn start_server(port: u16) -> Child {
    Command::new(binary())
        .args(["serve", "--offline"])
        .env("KANADE_TIMEZONE", "Asia/Kuala_Lumpur")
        .env("KANADE_ADMIN_BIND", format!("127.0.0.1:{port}"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn response(address: SocketAddr) -> String {
    request(address, "localhost", "/healthz")
}

fn request(address: SocketAddr, host: &str, path: &str) -> String {
    // Generous: parallel test binaries can slow startup well past a second.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match std::net::TcpStream::connect(address) {
            Ok(mut stream) => {
                stream
                    .write_all(
                        format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
                            .as_bytes(),
                    )
                    .unwrap();
                let mut value = String::new();
                stream.read_to_string(&mut value).unwrap();
                return value;
            }
            Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
            Err(error) => panic!("server never became ready: {error}"),
        }
    }
}

#[test]
fn offline_server_healthcheck_and_sigterm_are_operational() {
    let _serial = serial();
    let port = unused_loopback_port();
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut server = start_server(port);
    let body = response(address);
    assert!(body.starts_with("HTTP/1.1 200"));
    assert!(body.contains("\"mode\":\"offline\""));
    assert!(body.contains("\"scheduler\":\"unavailable\""));

    let healthcheck = Command::new(binary())
        .args(["healthcheck", "--url", &format!("http://{address}/healthz")])
        .status()
        .unwrap();
    assert!(healthcheck.success());

    #[cfg(unix)]
    Command::new("/bin/kill")
        .args(["-TERM", &server.id().to_string()])
        .status()
        .unwrap();
    #[cfg(not(unix))]
    server.kill().unwrap();
    assert!(
        server.wait_timeout(Duration::from_secs(3)).is_some(),
        "server did not drain after termination"
    );
    let mut logs = String::new();
    server
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut logs)
        .unwrap();
    let started = format!(
        "\"event\":\"server_started\",\"mode\":\"offline\",\"bind\":\"{address}\",\"version\":\"{}\"",
        env!("CARGO_PKG_VERSION")
    );
    assert!(logs.contains(&started), "{logs}");
}

#[test]
fn public_listener_starts_only_when_configured_and_is_closed() {
    let _serial = serial();
    let (admin_port, public_port) = unused_loopback_port_pair();
    let mut server = Command::new(binary())
        .args(["serve", "--offline"])
        .env("KANADE_TIMEZONE", "Asia/Kuala_Lumpur")
        .env("KANADE_ADMIN_BIND", format!("127.0.0.1:{admin_port}"))
        .env("KANADE_PUBLIC_BIND", format!("127.0.0.1:{public_port}"))
        .env("KANADE_PUBLIC_HOST", "kanade-pub.test")
        .env("KANADE_CLOUDFLARED_PEER", "127.0.0.1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let public = SocketAddr::from(([127, 0, 0, 1], public_port));
    let status = request(public, "kanade-pub.test", "/api/public/status");
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert!(status.contains("{\"portal\":\"closed\"}"));
    let health = request(public, "kanade-pub.test", "/healthz");
    assert!(health.starts_with("HTTP/1.1 404"), "{health}");
    let admin = response(SocketAddr::from(([127, 0, 0, 1], admin_port)));
    assert!(admin.starts_with("HTTP/1.1 200"));
    server.kill().unwrap();
    server.wait().unwrap();
}

#[test]
fn healthcheck_fails_when_no_loopback_server_is_available() {
    let _serial = serial();
    let port = unused_loopback_port();
    let status = Command::new(binary())
        .args([
            "healthcheck",
            "--url",
            &format!("http://127.0.0.1:{port}/healthz"),
        ])
        .status()
        .unwrap();
    assert!(!status.success());
}

/// The release number is the crate version, printed even when the settings
/// would refuse every other command.
#[test]
fn version_prints_the_crate_version_without_configuration() {
    for flag in ["--version", "-V"] {
        let output = Command::new(binary())
            .arg(flag)
            .env("KANADE_CONFIG", "/nonexistent/kanade.toml")
            .env("KANADE_TIMEZONE", "not-a-timezone")
            .output()
            .unwrap();
        assert!(output.status.success(), "{flag}: {output:?}");
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            concat!("kanade ", env!("CARGO_PKG_VERSION"), "\n")
        );
    }
}

/// `serve --offline` that must be refused with 78 (`EX_CONFIG`): its stderr.
/// `env` applies over a KL timezone and a loopback admin bind; `toml`, when
/// given, is the kanade.toml it reads.
fn refused_serve(env: &[(&str, &str)], toml: Option<&str>) -> String {
    let mut command = Command::new(binary());
    command
        .args(["serve", "--offline"])
        .env("KANADE_TIMEZONE", "Asia/Kuala_Lumpur")
        .env("KANADE_ADMIN_BIND", "127.0.0.1:0");
    let path = std::env::temp_dir().join(format!("kanade-refused-{}.toml", std::process::id()));
    if let Some(text) = toml {
        std::fs::write(&path, text).unwrap();
        command.env("KANADE_CONFIG", &path);
    }
    let output = command.envs(env.iter().copied()).output().unwrap();
    if toml.is_some() {
        std::fs::remove_file(&path).unwrap();
    }
    assert_eq!(output.status.code(), Some(78), "{env:?} {toml:?}");
    String::from_utf8(output.stderr).unwrap()
}

type Env = Vec<(&'static str, &'static str)>;

/// Bad settings are refused at startup, naming the kanade.toml key (the
/// variable in brackets) and never echoing a value: renamed, removed and
/// plain-secret variables, removed and secret-looking kanade.toml keys, bad
/// values, member sign-in bounds, the order fresh <= idle <= absolute and a
/// member sign-in or public host without what it needs.
#[test]
fn bad_settings_are_refused_at_startup_naming_the_key_without_the_value() {
    let _serial = serial();
    let toml_secret =
        |table: &str| format!("[runtime]\ntimezone = \"UTC\"\n{table} = \"sentinel\"\n");
    // (variables, kanade.toml, expected message, text that must not be echoed)
    let cases: Vec<(Env, Option<String>, &str, Option<&str>)> = vec![
        (
            vec![("KANADE_BIND", "127.0.0.1:0")],
            None,
            "KANADE_BIND was renamed to KANADE_ADMIN_BIND",
            None,
        ),
        (
            vec![("DISCORD_TOKEN", "plain-token-value")],
            None,
            "DISCORD_TOKEN is not read; put it in the secrets directory as `discord_token`",
            Some("plain-token-value"),
        ),
        (
            vec![("KANADE_DISCORD_TOKEN_FILE", "/run/secrets/sentinel")],
            None,
            "KANADE_DISCORD_TOKEN_FILE was removed: put the token in the secrets directory as `discord_token`",
            Some("sentinel"),
        ),
        (
            vec![("KANADE_HEALTHCHECK_URL", "http://127.0.0.1:1/sentinel")],
            None,
            "KANADE_HEALTHCHECK_URL was removed: the healthcheck targets KANADE_ADMIN_BIND",
            Some("sentinel"),
        ),
        (
            Vec::new(),
            Some(toml_secret("[admin]\ntoken_file")),
            "kanade.toml `admin.token_file` was removed: put the token in the secrets directory as `admin_token`",
            Some("sentinel"),
        ),
        (
            Vec::new(),
            Some(toml_secret("[settings]\npost_channel_id")),
            "kanade.toml `settings.post_channel_id` was removed: set it in Config › Weekly digest",
            Some("sentinel"),
        ),
        (
            vec![("KANADE_DISCORD_GATEWAY", "0")],
            None,
            "KANADE_DISCORD_GATEWAY was removed: replaced by kanade.toml `runtime.components` [KANADE_COMPONENTS]",
            None,
        ),
        (
            Vec::new(),
            Some("[runtime]\ntimezone = \"UTC\"\n[discord]\ngateway = false\n".into()),
            "kanade.toml `discord.gateway` was removed: replaced by `runtime.components`",
            None,
        ),
        (
            vec![("KANADE_COMPONENTS", "web,sentinel")],
            None,
            "kanade.toml `runtime.components` [KANADE_COMPONENTS]: this build has no such component",
            Some("sentinel"),
        ),
        (
            vec![("KANADE_TIMEZONE", "not-a-timezone")],
            None,
            "kanade.toml `runtime.timezone` [KANADE_TIMEZONE] must be a valid IANA timezone",
            Some("not-a-timezone"),
        ),
        (
            vec![("KANADE_TIMEZONE", "")],
            Some("[runtime]\ntimezone = \"Not/A-sentinel\"\n".into()),
            "kanade.toml `runtime.timezone` [KANADE_TIMEZONE] must be a valid IANA timezone",
            Some("sentinel"),
        ),
        (
            Vec::new(),
            Some(toml_secret("[discord]\ntoken")),
            "kanade.toml `discord.token` looks like a secret",
            Some("sentinel"),
        ),
        (
            Vec::new(),
            Some(toml_secret("[public]\ndiscord_client_secret")),
            "public.discord_client_secret",
            Some("sentinel"),
        ),
        (
            vec![("KANADE_PUBLIC_SESSION_IDLE_MINUTES", "61")],
            None,
            "kanade.toml `public.session_idle_minutes` [KANADE_PUBLIC_SESSION_IDLE_MINUTES] must be between 5 and 60",
            Some("61"),
        ),
        (
            vec![("KANADE_PUBLIC_SESSION_ABSOLUTE_HOURS", "25")],
            None,
            "[KANADE_PUBLIC_SESSION_ABSOLUTE_HOURS] must be between 1 and 24",
            Some("25"),
        ),
        (
            vec![("KANADE_PUBLIC_FRESH_WRITE_MINUTES", "4")],
            None,
            "[KANADE_PUBLIC_FRESH_WRITE_MINUTES] must be between 5 and 30",
            None,
        ),
        (
            vec![
                ("KANADE_PUBLIC_SESSION_IDLE_MINUTES", "10"),
                ("KANADE_PUBLIC_FRESH_WRITE_MINUTES", "15"),
            ],
            None,
            "[KANADE_PUBLIC_FRESH_WRITE_MINUTES] must not exceed",
            None,
        ),
        (
            vec![("KANADE_PUBLIC_DISCORD_CLIENT_SECRET", "plain-public-secret")],
            None,
            "KANADE_PUBLIC_DISCORD_CLIENT_SECRET is not read; put it in the secrets directory as `public_discord_client_secret`",
            Some("plain-public-secret"),
        ),
        (
            vec![("KANADE_PUBLIC_DISCORD_CLIENT_ID", "1234567890")],
            None,
            "kanade.toml `public.host` [KANADE_PUBLIC_HOST] is required for member sign-in",
            None,
        ),
        (
            vec![("KANADE_PUBLIC_HOST", "kanade-pub.test")],
            None,
            "KANADE_PUBLIC_BIND is required when kanade.toml `public.host` [KANADE_PUBLIC_HOST] is set",
            None,
        ),
    ];
    for (pairs, toml, expected, secret) in cases {
        let stderr = refused_serve(&pairs, toml.as_deref());
        assert!(stderr.contains(expected), "{pairs:?} {toml:?}: {stderr}");
        if let Some(secret) = secret {
            assert!(
                !stderr.contains(secret),
                "{pairs:?} {toml:?} echoed {secret}"
            );
        }
    }
}

/// `serve --offline` is the admin app's shell, so a bot-only list refuses
/// it as a usage error (64).
#[test]
fn offline_serve_needs_the_web_component() {
    let output = Command::new(binary())
        .args(["serve", "--offline"])
        .env("KANADE_TIMEZONE", "Asia/Kuala_Lumpur")
        .env("KANADE_ADMIN_BIND", "127.0.0.1:0")
        .env("KANADE_COMPONENTS", "bot")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(64), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("serve --offline needs the `web` component"),
        "{stderr}"
    );
}

trait ChildTimeout {
    fn wait_timeout(&mut self, timeout: Duration) -> Option<std::process::ExitStatus>;
}

impl ChildTimeout for Child {
    fn wait_timeout(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait().unwrap() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}
