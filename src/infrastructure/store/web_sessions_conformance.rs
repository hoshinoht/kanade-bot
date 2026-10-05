//! Web session storage every store must keep: round trips, rotation in one
//! write, touch/delete, per-identity revocation, pruning and CHECKs.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::web_sessions::{
    LoginMethod, SessionOrigin, WebSession, WebSessionStore,
};

pub async fn run_suite<S: WebSessionStore>(make: impl AsyncFn() -> S) {
    sessions_round_trip_and_rotate(make().await).await;
    touch_delete_and_revoke_by_identity(make().await).await;
    prune_removes_absolute_and_idle_expiries(make().await).await;
    malformed_rows_are_refused(make().await).await;
}

fn at(minute: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 0).unwrap() + TimeDelta::minutes(minute)
}

fn hash(fill: char) -> String {
    fill.to_string().repeat(64)
}

pub fn session(fill: char, method: LoginMethod, subject: &str) -> WebSession {
    WebSession {
        id_hash: hash(fill),
        origin: SessionOrigin::Admin,
        method,
        subject: subject.into(),
        display: format!("{} {subject}", method.as_str()),
        created_at: at(0),
        last_seen_at: at(0),
        checked_at: at(0),
        expires_at: at(12 * 60),
        avatar_hash: None,
        device: None,
    }
}

async fn sessions_round_trip_and_rotate<S: WebSessionStore>(store: S) {
    let mut first = session('a', LoginMethod::Discord, "100");
    first.avatar_hash = Some("a_0123456789abcdef0123456789abcdef".into());
    first.device = Some("Firefox · macOS".into());
    store.put_session(&first, None).await.expect("put");
    assert_eq!(
        store.load_session(&first.id_hash).await.expect("load"),
        Some(first.clone()),
        "round trip"
    );
    assert!(
        matches!(
            store.put_session(&first, None).await,
            Err(StoreError::Constraint(_))
        ),
        "duplicate id hash"
    );

    let rotated = session('b', LoginMethod::Discord, "100");
    store
        .put_session(&rotated, Some(&first.id_hash))
        .await
        .expect("rotate");
    assert_eq!(
        store.load_session(&first.id_hash).await.expect("load"),
        None
    );
    assert!(
        store
            .load_session(&rotated.id_hash)
            .await
            .expect("load")
            .is_some()
    );
    assert_eq!(store.load_session(&hash('c')).await.expect("load"), None);
}

async fn touch_delete_and_revoke_by_identity<S: WebSessionStore>(store: S) {
    for (fill, method, subject) in [
        ('a', LoginMethod::Discord, "100"),
        ('b', LoginMethod::Discord, "100"),
        ('c', LoginMethod::Discord, "200"),
        ('d', LoginMethod::Tailscale, "100"),
    ] {
        store
            .put_session(&session(fill, method, subject), None)
            .await
            .expect("put");
    }
    assert!(
        store
            .touch_session(&hash('a'), at(5), at(4))
            .await
            .expect("touch")
    );
    let touched = store.load_session(&hash('a')).await.expect("load").unwrap();
    assert_eq!((touched.last_seen_at, touched.checked_at), (at(5), at(4)));
    assert!(
        store
            .touch_session(&hash('a'), at(3), at(6))
            .await
            .expect("older touch")
    );
    let touched = store.load_session(&hash('a')).await.expect("load").unwrap();
    assert_eq!((touched.last_seen_at, touched.checked_at), (at(5), at(6)));
    assert!(
        !store
            .touch_session(&hash('e'), at(5), at(5))
            .await
            .expect("touch")
    );

    let listed: Vec<String> = store
        .subject_sessions(SessionOrigin::Admin, LoginMethod::Discord, "100")
        .await
        .expect("list")
        .into_iter()
        .map(|session| session.id_hash)
        .collect();
    assert_eq!(
        listed,
        [hash('a'), hash('b')],
        "one identity's sessions, oldest first"
    );
    assert!(
        store
            .subject_sessions(SessionOrigin::Public, LoginMethod::Discord, "100")
            .await
            .expect("list")
            .is_empty(),
        "never another origin's"
    );

    assert_eq!(
        store
            .delete_subject_sessions(SessionOrigin::Admin, LoginMethod::Discord, "100")
            .await
            .expect("revoke"),
        2,
        "only that identity's sessions"
    );
    assert_eq!(store.load_session(&hash('b')).await.expect("load"), None);
    assert!(
        store
            .load_session(&hash('c'))
            .await
            .expect("load")
            .is_some()
    );
    assert!(
        store
            .load_session(&hash('d'))
            .await
            .expect("load")
            .is_some()
    );

    assert!(store.delete_session(&hash('c')).await.expect("delete"));
    assert!(!store.delete_session(&hash('c')).await.expect("delete"));
}

async fn prune_removes_absolute_and_idle_expiries<S: WebSessionStore>(store: S) {
    let mut expired = session('a', LoginMethod::Token, "token");
    expired.expires_at = at(10);
    let mut idle = session('b', LoginMethod::Token, "token");
    idle.last_seen_at = at(3);
    let mut live = session('c', LoginMethod::Token, "token");
    live.last_seen_at = at(9);
    for session in [&expired, &idle, &live] {
        store.put_session(session, None).await.expect("put");
    }
    assert_eq!(
        store.prune_sessions(at(10), at(5)).await.expect("prune"),
        2,
        "expiry at the instant and idle at or before the cutoff are pruned"
    );
    assert!(
        store
            .load_session(&hash('c'))
            .await
            .expect("load")
            .is_some()
    );
}

async fn malformed_rows_are_refused<S: WebSessionStore>(store: S) {
    let mut bad_hash = session('a', LoginMethod::Token, "token");
    bad_hash.id_hash = "A".repeat(64);
    let mut short_hash = session('a', LoginMethod::Token, "token");
    short_hash.id_hash = "a".repeat(63);
    let mut empty_subject = session('a', LoginMethod::Token, "token");
    empty_subject.subject.clear();
    let mut long_display = session('a', LoginMethod::Token, "token");
    long_display.display = "x".repeat(201);
    let mut bad_avatar = session('a', LoginMethod::Discord, "100");
    bad_avatar.avatar_hash = Some("<svg onload=x>".into());
    let mut short_avatar = session('a', LoginMethod::Discord, "100");
    short_avatar.avatar_hash = Some("0123".into());
    let device = |text: String| {
        let mut row = session('a', LoginMethod::Discord, "100");
        row.device = Some(text);
        row
    };
    let underscored = |hash: &str| {
        let mut row = session('a', LoginMethod::Discord, "100");
        row.avatar_hash = Some(hash.into());
        row
    };
    for (case, session) in [
        (
            "underscore inside",
            underscored("0123456789abcdef0123456789abcd_f"),
        ),
        (
            "prefix not a_",
            underscored("_a0123456789abcdef0123456789abcdef"),
        ),
        (
            "doubled prefix",
            underscored("a_a_23456789abcdef0123456789abcdef"),
        ),
        (
            "uppercase avatar hash",
            underscored("0123456789ABCDEF0123456789ABCDEF"),
        ),
        ("empty device", device(String::new())),
        ("long device", device("x".repeat(65))),
        ("markup avatar hash", bad_avatar),
        ("short avatar hash", short_avatar),
        ("uppercase hash", bad_hash),
        ("short hash", short_hash),
        ("empty subject", empty_subject),
        ("long display", long_display),
    ] {
        assert!(
            matches!(
                store.put_session(&session, None).await,
                Err(StoreError::Constraint(_))
            ),
            "{case}"
        );
    }
}
