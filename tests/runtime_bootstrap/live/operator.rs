//! `kanade portal` and `kanade sessions` against the real admin API of a
//! live serve: the switch round-trips, other token sessions are listed,
//! revoked and signed out, and every run ends its own session.

use super::*;

fn cli(live: &Live, admin: SocketAddr, args: &[&str]) -> Output {
    let output = Command::new(binary())
        .args(args)
        .args(["--url", &format!("http://{admin}")])
        .env_clear()
        .env("KANADE_SECRETS_DIR", &live.root)
        .output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!all.contains(ADMIN_TOKEN), "{all}");
    assert_eq!(output.status.code(), Some(0), "{args:?}: {all}");
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// A browser-like token session; returns its cookie pair.
fn browser(admin: SocketAddr) -> String {
    let login = call(
        admin,
        "POST",
        "/api/admin/auth/token",
        &[("Sec-Fetch-Site", "same-origin")],
        Some(&format!(r#"{{"token":"{ADMIN_TOKEN}"}}"#)),
    );
    assert_eq!(login.status, 200, "{}", login.body);
    login
        .header("set-cookie")
        .and_then(|value| value.split(';').next())
        .unwrap()
        .to_owned()
}

fn signed_in(admin: SocketAddr, cookie: &str) -> bool {
    call(
        admin,
        "GET",
        "/api/admin/session",
        &[("Cookie", cookie)],
        None,
    )
    .status
        == 200
}

/// A served admin listener; token sign-ins allow 5 per minute per client,
/// so each test stays within that.
fn serve(live: &Live) -> (SocketAddr, Child) {
    let port = unused_loopback_port();
    let admin = SocketAddr::from(([127, 0, 0, 1], port));
    let server = live.spawn(port, None);
    assert!(ready(admin).starts_with("HTTP/1.1 200"));
    (admin, server)
}

fn stop(server: Child) {
    let output = terminate(server);
    assert!(output.status.success(), "{output:?}");
    assert!(!String::from_utf8_lossy(&output.stderr).contains(ADMIN_TOKEN));
}

#[test]
fn portal_commands_round_trip_the_live_switch_and_end_their_sessions() {
    let _serial = serial();
    let live = Live::new();
    let (admin, server) = serve(&live);
    let switch = |args: &[&str]| stdout(&cli(&live, admin, args));
    assert_eq!(
        switch(&["portal", "status"]),
        "public portal switch: closed\n"
    );
    assert_eq!(switch(&["portal", "open"]), "public portal switch: open\n");
    assert_eq!(
        switch(&["portal", "close"]),
        "public portal switch: closed\n"
    );
    assert!(
        switch(&["sessions", "list"]).starts_with("1 live session(s) of the break-glass token:\n"),
        "every earlier run signed itself out"
    );
    stop(server);
}

#[test]
fn sessions_commands_revoke_and_sign_out_live_token_sessions() {
    let _serial = serial();
    let live = Live::new();
    let (admin, server) = serve(&live);
    let other = browser(admin);
    let listed = stdout(&cli(&live, admin, &["sessions", "list"]));
    assert!(
        listed.starts_with("2 live session(s) of the break-glass token:\n"),
        "{listed}"
    );
    let handle = listed
        .lines()
        .skip(1)
        .find(|line| !line.ends_with("(this command)"))
        .and_then(|line| line.split_whitespace().next())
        .unwrap()
        .to_owned();
    assert!(signed_in(admin, &other));
    assert_eq!(
        stdout(&cli(&live, admin, &["sessions", "revoke", &handle])),
        format!("session {handle} signed out\n")
    );
    assert!(!signed_in(admin, &other));

    let another = browser(admin);
    assert_eq!(
        stdout(&cli(&live, admin, &["sessions", "sign-out-all"])),
        "signed out 1 other session(s) and this command's own\n"
    );
    assert!(!signed_in(admin, &another));
    stop(server);
}
