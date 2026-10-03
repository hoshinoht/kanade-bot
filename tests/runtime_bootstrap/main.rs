mod backup;
mod live;

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
fn renamed_bind_variable_is_refused() {
    let _serial = serial();
    let output = Command::new(binary())
        .args(["serve", "--offline"])
        .env("KANADE_TIMEZONE", "Asia/Kuala_Lumpur")
        .env("KANADE_BIND", "127.0.0.1:0")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("KANADE_BIND was renamed to KANADE_ADMIN_BIND"));
}

#[test]
fn plain_discord_token_is_refused_without_echoing_it() {
    let _serial = serial();
    let output = Command::new(binary())
        .args(["serve", "--offline"])
        .env("KANADE_TIMEZONE", "Asia/Kuala_Lumpur")
        .env("KANADE_ADMIN_BIND", "127.0.0.1:0")
        .env("DISCORD_TOKEN", "plain-token-value")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("DISCORD_TOKEN is not read; use KANADE_DISCORD_TOKEN_FILE"));
    assert!(!stderr.contains("plain-token-value"));
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

#[test]
fn invalid_configuration_fails_without_echoing_values() {
    let _serial = serial();
    let output = Command::new(binary())
        .args(["serve", "--offline"])
        .env("KANADE_TIMEZONE", "not-a-timezone")
        .env("KANADE_ADMIN_BIND", "127.0.0.1:0")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("KANADE_TIMEZONE must be a valid IANA timezone"));
    assert!(!stderr.contains("not-a-timezone"));
}

#[test]
fn config_file_errors_name_the_key_without_the_value() {
    let _serial = serial();
    let path = std::env::temp_dir().join(format!("kanade-toml-{}", std::process::id()));
    std::fs::write(&path, "[runtime]\ntimezone = \"Not/A-sentinel\"\n").unwrap();
    let run = |timezone: &str| {
        Command::new(binary())
            .args(["serve", "--offline"])
            .env("KANADE_CONFIG", &path)
            .env("KANADE_TIMEZONE", timezone)
            .env("KANADE_ADMIN_BIND", "127.0.0.1:0")
            .output()
            .unwrap()
    };
    let output = run("");
    assert_eq!(output.status.code(), Some(78));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains(
        "KANADE_TIMEZONE must be a valid IANA timezone (kanade.toml `runtime.timezone`)"
    ));
    assert!(!stderr.contains("sentinel"));

    std::fs::write(
        &path,
        "[runtime]\ntimezone = \"UTC\"\n[discord]\ntoken = \"sentinel\"\n",
    )
    .unwrap();
    let output = run("Asia/Kuala_Lumpur");
    std::fs::remove_file(&path).unwrap();
    assert_eq!(output.status.code(), Some(78));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("kanade.toml `discord.token` looks like a secret"));
    assert!(!stderr.contains("sentinel"));
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
