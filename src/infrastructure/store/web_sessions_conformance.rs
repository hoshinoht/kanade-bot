//! Web session storage every store must keep: round trips, rotation in one
//! write, touch/delete, per-identity revocation, origin-scoped pruning and
//! replacement, D9 rotation grace, the per-identity cap in one write and
//! CHECKs.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::web_sessions::{
    LoginMethod, SessionOrigin, WebSession, WebSessionStore,
};

pub async fn run_suite<S: WebSessionStore>(make: impl AsyncFn() -> S) {
    sessions_round_trip_and_rotate(make().await).await;
    touch_delete_and_revoke_by_identity(make().await).await;
    prune_removes_absolute_and_idle_expiries(make().await).await;
    prune_keeps_the_other_origin(make().await).await;
    replace_never_crosses_origins(make().await).await;
    rotation_keeps_the_old_id_for_its_grace(make().await).await;
    malformed_rows_are_refused(make().await).await;
    capped_put_holds_the_cap_per_identity(make().await).await;
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
        client_tag: None,
        superseded_until: None,
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
        store
            .prune_sessions(SessionOrigin::Admin, at(10), at(5))
            .await
            .expect("prune"),
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

async fn prune_keeps_the_other_origin<S: WebSessionStore>(store: S) {
    let mut admin = session('a', LoginMethod::Discord, "100");
    admin.last_seen_at = at(3);
    let mut public = session('b', LoginMethod::Discord, "100");
    public.origin = SessionOrigin::Public;
    public.last_seen_at = at(3);
    for session in [&admin, &public] {
        store.put_session(session, None).await.expect("put");
    }
    assert_eq!(
        store
            .prune_sessions(SessionOrigin::Admin, at(10), at(5))
            .await
            .expect("admin prune"),
        1,
        "an admin prune never touches public rows"
    );
    assert_eq!(
        store.load_session(&admin.id_hash).await.expect("load"),
        None
    );
    assert!(
        store
            .load_session(&public.id_hash)
            .await
            .expect("load")
            .is_some()
    );

    let mut admin = session('c', LoginMethod::Discord, "100");
    admin.last_seen_at = at(3);
    store.put_session(&admin, None).await.expect("put");
    assert_eq!(
        store
            .prune_sessions(SessionOrigin::Public, at(10), at(5))
            .await
            .expect("public prune"),
        1,
        "a public prune never touches admin rows"
    );
    assert_eq!(
        store.load_session(&public.id_hash).await.expect("load"),
        None
    );
    assert!(
        store
            .load_session(&admin.id_hash)
            .await
            .expect("load")
            .is_some()
    );
}

async fn replace_never_crosses_origins<S: WebSessionStore>(store: S) {
    let admin = session('a', LoginMethod::Discord, "100");
    let mut public = session('b', LoginMethod::Discord, "100");
    public.origin = SessionOrigin::Public;
    for session in [&admin, &public] {
        store.put_session(session, None).await.expect("put");
    }

    let mut new_public = session('c', LoginMethod::Discord, "100");
    new_public.origin = SessionOrigin::Public;
    store
        .put_session(&new_public, Some(&admin.id_hash))
        .await
        .expect("put");
    let new_admin = session('d', LoginMethod::Discord, "100");
    store
        .put_session(&new_admin, Some(&public.id_hash))
        .await
        .expect("put");
    for (case, kept) in [
        ("a public sign-in never replaces an admin session", &admin),
        ("an admin sign-in never replaces a public session", &public),
        ("the public insert still lands", &new_public),
        ("the admin insert still lands", &new_admin),
    ] {
        assert_eq!(
            store.load_session(&kept.id_hash).await.expect("load"),
            Some(kept.clone()),
            "{case}"
        );
    }

    let mut colliding = session('a', LoginMethod::Discord, "100");
    colliding.origin = SessionOrigin::Public;
    assert!(
        matches!(
            store.put_session(&colliding, Some(&admin.id_hash)).await,
            Err(StoreError::Constraint(_))
        ),
        "replacing another origin's id with itself is still a duplicate"
    );
    assert_eq!(
        store.load_session(&admin.id_hash).await.expect("load"),
        Some(admin)
    );
}

async fn rotation_keeps_the_old_id_for_its_grace<S: WebSessionStore>(store: S) {
    let public = |fill: char, subject: &str| {
        let mut row = session(fill, LoginMethod::Discord, subject);
        row.origin = SessionOrigin::Public;
        row.client_tag = Some(hash('e'));
        row
    };
    let old = public('a', "100");
    store.put_session(&old, None).await.expect("put");
    let mut new = public('b', "100");
    new.client_tag = Some(hash('f'));
    new.created_at = at(1);

    assert!(
        matches!(
            store.rotate_session(&old, &old.id_hash, at(10)).await,
            Err(StoreError::Constraint(_))
        ),
        "the new id must be fresh"
    );
    assert_eq!(
        store.load_session(&old.id_hash).await.expect("load"),
        Some(old.clone()),
        "a refused rotation writes nothing"
    );
    for (case, other) in [
        ("another subject", public('c', "200")),
        ("another origin", session('c', LoginMethod::Discord, "100")),
        ("another method", {
            let mut row = public('c', "100");
            row.method = LoginMethod::Tailscale;
            row
        }),
    ] {
        assert!(
            !store
                .rotate_session(&other, &old.id_hash, at(10))
                .await
                .expect("rotate"),
            "{case}"
        );
        assert_eq!(
            store.load_session(&other.id_hash).await.expect("load"),
            None,
            "{case} writes nothing"
        );
    }
    assert!(
        !store
            .rotate_session(&new, &hash('d'), at(10))
            .await
            .expect("rotate"),
        "an unknown id"
    );

    assert!(
        store
            .rotate_session(&new, &old.id_hash, at(10))
            .await
            .expect("rotate")
    );
    assert_eq!(
        store.load_session(&new.id_hash).await.expect("load"),
        Some(new.clone())
    );
    assert_eq!(
        store.load_session(&old.id_hash).await.expect("load"),
        None,
        "a superseded id is never a live session"
    );
    let superseded = WebSession {
        superseded_until: Some(at(10)),
        ..old.clone()
    };
    assert_eq!(
        store
            .load_superseded(&old.id_hash, at(9))
            .await
            .expect("load"),
        Some(superseded),
        "readable within its grace"
    );
    assert_eq!(
        store
            .load_superseded(&old.id_hash, at(10))
            .await
            .expect("load"),
        None,
        "the grace ends at superseded_until"
    );
    assert_eq!(
        store
            .load_superseded(&new.id_hash, at(9))
            .await
            .expect("load"),
        None,
        "a live id is not superseded"
    );
    assert!(
        !store
            .touch_session(&old.id_hash, at(9), at(9))
            .await
            .expect("touch"),
        "a superseded id is never touched"
    );
    assert!(
        !store
            .rotate_session(&public('c', "100"), &old.id_hash, at(20))
            .await
            .expect("rotate"),
        "a superseded id never rotates again"
    );
    let live: Vec<String> = store
        .subject_sessions(SessionOrigin::Public, LoginMethod::Discord, "100")
        .await
        .expect("list")
        .into_iter()
        .map(|session| session.id_hash)
        .collect();
    assert_eq!(
        live,
        [new.id_hash.clone()],
        "the session cap counts live ids"
    );

    assert_eq!(
        store
            .prune_sessions(SessionOrigin::Public, at(9), at(-1))
            .await
            .expect("prune"),
        0,
        "kept while its grace lasts"
    );
    assert_eq!(
        store
            .prune_sessions(SessionOrigin::Admin, at(10), at(-1))
            .await
            .expect("prune"),
        0,
        "never by the other origin's prune"
    );
    assert_eq!(
        store
            .prune_sessions(SessionOrigin::Public, at(10), at(-1))
            .await
            .expect("prune"),
        1,
        "pruned once its grace is over"
    );
    assert!(
        store
            .load_session(&new.id_hash)
            .await
            .expect("load")
            .is_some()
    );
    assert!(!store.delete_session(&old.id_hash).await.expect("delete"));
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
    let tagged = |tag: String| {
        let mut row = session('a', LoginMethod::Discord, "100");
        row.origin = SessionOrigin::Public;
        row.client_tag = Some(tag);
        row
    };
    let mut superseded = session('a', LoginMethod::Discord, "100");
    superseded.superseded_until = Some(at(10));
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
        ("uppercase client tag", tagged("E".repeat(64))),
        ("short client tag", tagged("e".repeat(63))),
        ("non-hex client tag", tagged("g".repeat(64))),
        ("starts superseded", superseded),
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

async fn capped_put_holds_the_cap_per_identity<S: WebSessionStore>(store: S) {
    let public = |fill: char, subject: &str, minute: i64| {
        let mut row = session(fill, LoginMethod::Discord, subject);
        row.origin = SessionOrigin::Public;
        row.created_at = at(minute);
        row
    };
    let live = async |store: &S| -> Vec<String> {
        store
            .subject_sessions(SessionOrigin::Public, LoginMethod::Discord, "100")
            .await
            .expect("list")
            .into_iter()
            .map(|row| row.id_hash)
            .collect()
    };
    let admin = session('4', LoginMethod::Discord, "100");
    let other_subject = public('5', "200", 0);
    let mut other_method = public('6', "100", 0);
    other_method.method = LoginMethod::Tailscale;
    let mut expired = public('7', "300", 0);
    expired.expires_at = at(4);
    for row in [
        &public('1', "100", 1),
        &public('2', "100", 2),
        &public('3', "100", 3),
        &admin,
        &other_subject,
        &other_method,
        &expired,
    ] {
        store.put_session(row, None).await.expect("put");
    }
    // A rotated-out id is not a live session and does not count.
    assert!(
        store
            .rotate_session(&public('8', "100", 3), &hash('3'), at(10))
            .await
            .expect("rotate")
    );

    let capped = |fill: char, minute: i64| public(fill, "100", minute);
    assert_eq!(
        store
            .put_capped_session(&capped('9', 5), None, 3, at(5), at(-1))
            .await
            .expect("capped put"),
        1,
        "the oldest live session beyond max - 1 ends"
    );
    assert_eq!(live(&store).await, [hash('2'), hash('8'), hash('9')]);
    assert!(
        store
            .load_superseded(&hash('3'), at(5))
            .await
            .expect("load")
            .is_some(),
        "a superseded id in its grace is kept"
    );
    assert_eq!(
        store.load_session(&hash('7')).await.expect("load"),
        None,
        "the origin is pruned in the same write"
    );

    assert_eq!(
        store
            .put_capped_session(&capped('a', 6), Some(&hash('8')), 3, at(6), at(-1))
            .await
            .expect("capped put"),
        0,
        "the replaced session makes room"
    );
    assert_eq!(live(&store).await, [hash('2'), hash('9'), hash('a')]);

    assert_eq!(
        store
            .put_capped_session(&capped('b', 7), Some(&admin.id_hash), 3, at(7), at(-1))
            .await
            .expect("capped put"),
        1,
        "another origin's id is never replaced, so the cap ends one"
    );
    assert_eq!(live(&store).await, [hash('9'), hash('a'), hash('b')]);

    assert!(
        matches!(
            store
                .put_capped_session(&capped('b', 8), None, 3, at(8), at(-1))
                .await,
            Err(StoreError::Constraint(_))
        ),
        "duplicate id hash"
    );
    let mut superseded = capped('c', 8);
    superseded.superseded_until = Some(at(10));
    assert!(
        matches!(
            store
                .put_capped_session(&superseded, None, 3, at(8), at(-1))
                .await,
            Err(StoreError::Constraint(_))
        ),
        "starts superseded"
    );
    assert_eq!(
        live(&store).await,
        [hash('9'), hash('a'), hash('b')],
        "a refused capped put writes nothing"
    );

    for (case, kept) in [
        ("the admin session", &admin),
        ("another subject", &other_subject),
        ("another method", &other_method),
    ] {
        assert_eq!(
            store.load_session(&kept.id_hash).await.expect("load"),
            Some(kept.clone()),
            "{case}"
        );
    }
}
