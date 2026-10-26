//! Web session storage every store must keep: round trips, rotation in one
//! write, touch/delete, per-identity revocation, origin-scoped pruning and
//! replacement, D9 rotation grace, the per-identity cap in one write and
//! CHECKs.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use super::fixtures::refused_for;
use crate::web_sessions::{LoginMethod, SessionOrigin, WebSession, WebSessionStore};

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

async fn load<S: WebSessionStore>(store: &S, id_hash: &str) -> Option<WebSession> {
    store.load_session(id_hash).await.expect("load")
}

async fn prune<S: WebSessionStore>(
    store: &S,
    origin: SessionOrigin,
    now: DateTime<Utc>,
    idle_cutoff: DateTime<Utc>,
) -> u64 {
    let pruned = store.prune_sessions(origin, now, idle_cutoff).await;
    pruned.expect("prune")
}

async fn rotate<S: WebSessionStore>(store: &S, new: &WebSession, old: &str, at: i64) -> bool {
    let rotated = store.rotate_session(new, old, self::at(at)).await;
    rotated.expect("rotate")
}

async fn listed<S: WebSessionStore>(
    store: &S,
    origin: SessionOrigin,
    subject: &str,
) -> Vec<String> {
    let rows = store.subject_sessions(origin, LoginMethod::Discord, subject);
    let rows = rows.await.expect("list");
    rows.into_iter().map(|session| session.id_hash).collect()
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
        load(&store, &first.id_hash).await,
        Some(first.clone()),
        "round trip"
    );
    refused_for("duplicate id hash", store.put_session(&first, None).await);

    let rotated = session('b', LoginMethod::Discord, "100");
    store
        .put_session(&rotated, Some(&first.id_hash))
        .await
        .expect("rotate");
    assert_eq!(load(&store, &first.id_hash).await, None);
    assert!(load(&store, &rotated.id_hash).await.is_some());
    assert_eq!(load(&store, &hash('c')).await, None);
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
    let touch = async |fill, seen, checked| {
        let id_hash = hash(fill);
        let touched = store.touch_session(&id_hash, at(seen), at(checked));
        touched.await.expect("touch")
    };
    assert!(touch('a', 5, 4).await);
    let touched = load(&store, &hash('a')).await.unwrap();
    assert_eq!((touched.last_seen_at, touched.checked_at), (at(5), at(4)));
    assert!(touch('a', 3, 6).await, "an older touch");
    let touched = load(&store, &hash('a')).await.unwrap();
    assert_eq!((touched.last_seen_at, touched.checked_at), (at(5), at(6)));
    assert!(!touch('e', 5, 5).await);

    assert_eq!(
        listed(&store, SessionOrigin::Admin, "100").await,
        [hash('a'), hash('b')],
        "one identity's sessions, oldest first"
    );
    assert!(
        listed(&store, SessionOrigin::Public, "100")
            .await
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
    assert_eq!(load(&store, &hash('b')).await, None);
    assert!(load(&store, &hash('c')).await.is_some());
    assert!(load(&store, &hash('d')).await.is_some());

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
        prune(&store, SessionOrigin::Admin, at(10), at(5)).await,
        2,
        "expiry at the instant and idle at or before the cutoff are pruned"
    );
    assert!(load(&store, &hash('c')).await.is_some());
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
        prune(&store, SessionOrigin::Admin, at(10), at(5)).await,
        1,
        "an admin prune never touches public rows"
    );
    assert_eq!(load(&store, &admin.id_hash).await, None);
    assert!(load(&store, &public.id_hash).await.is_some());

    let mut admin = session('c', LoginMethod::Discord, "100");
    admin.last_seen_at = at(3);
    store.put_session(&admin, None).await.expect("put");
    assert_eq!(
        prune(&store, SessionOrigin::Public, at(10), at(5)).await,
        1,
        "a public prune never touches admin rows"
    );
    assert_eq!(load(&store, &public.id_hash).await, None);
    assert!(load(&store, &admin.id_hash).await.is_some());
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
            load(&store, &kept.id_hash).await,
            Some(kept.clone()),
            "{case}"
        );
    }

    let mut colliding = session('a', LoginMethod::Discord, "100");
    colliding.origin = SessionOrigin::Public;
    refused_for(
        "replacing another origin's id with itself is still a duplicate",
        store.put_session(&colliding, Some(&admin.id_hash)).await,
    );
    assert_eq!(load(&store, &admin.id_hash).await, Some(admin));
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

    refused_for(
        "the new id must be fresh",
        store.rotate_session(&old, &old.id_hash, at(10)).await,
    );
    assert_eq!(
        load(&store, &old.id_hash).await,
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
        assert!(!rotate(&store, &other, &old.id_hash, 10).await, "{case}");
        let written = load(&store, &other.id_hash).await;
        assert_eq!(written, None, "{case} writes nothing");
    }
    assert!(!rotate(&store, &new, &hash('d'), 10).await, "an unknown id");

    assert!(rotate(&store, &new, &old.id_hash, 10).await);
    assert_eq!(load(&store, &new.id_hash).await, Some(new.clone()));
    assert_eq!(
        load(&store, &old.id_hash).await,
        None,
        "a superseded id is never a live session"
    );
    let superseded = WebSession {
        superseded_until: Some(at(10)),
        ..old.clone()
    };
    let grace = async |id_hash, minute| {
        let row = store.load_superseded(id_hash, at(minute)).await;
        row.expect("load")
    };
    assert_eq!(
        grace(&old.id_hash, 9).await,
        Some(superseded),
        "readable within its grace"
    );
    assert_eq!(
        grace(&old.id_hash, 10).await,
        None,
        "the grace ends at superseded_until"
    );
    assert_eq!(
        grace(&new.id_hash, 9).await,
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
        !rotate(&store, &public('c', "100"), &old.id_hash, 20).await,
        "a superseded id never rotates again"
    );
    assert_eq!(
        listed(&store, SessionOrigin::Public, "100").await,
        [new.id_hash.clone()],
        "the session cap counts live ids"
    );

    assert_eq!(
        prune(&store, SessionOrigin::Public, at(9), at(-1)).await,
        0,
        "kept while its grace lasts"
    );
    assert_eq!(
        prune(&store, SessionOrigin::Admin, at(10), at(-1)).await,
        0,
        "never by the other origin's prune"
    );
    assert_eq!(
        prune(&store, SessionOrigin::Public, at(10), at(-1)).await,
        1,
        "pruned once its grace is over"
    );
    assert!(load(&store, &new.id_hash).await.is_some());
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
        refused_for(case, store.put_session(&session, None).await);
    }
}

async fn capped_put_holds_the_cap_per_identity<S: WebSessionStore>(store: S) {
    let public = |fill: char, subject: &str, minute: i64| {
        let mut row = session(fill, LoginMethod::Discord, subject);
        row.origin = SessionOrigin::Public;
        row.created_at = at(minute);
        row
    };
    let live = async |store: &S| listed(store, SessionOrigin::Public, "100").await;
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
    assert!(rotate(&store, &public('8', "100", 3), &hash('3'), 10).await);

    let capped = |fill: char, minute: i64| public(fill, "100", minute);
    let put = async |row: &WebSession, replaces: Option<&str>, minute| {
        let put = store.put_capped_session(row, replaces, 3, at(minute), at(-1));
        put.await
    };
    assert_eq!(
        put(&capped('9', 5), None, 5).await.expect("capped put"),
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
        load(&store, &hash('7')).await,
        None,
        "the origin is pruned in the same write"
    );

    assert_eq!(
        put(&capped('a', 6), Some(&hash('8')), 6)
            .await
            .expect("capped put"),
        0,
        "the replaced session makes room"
    );
    assert_eq!(live(&store).await, [hash('2'), hash('9'), hash('a')]);

    assert_eq!(
        put(&capped('b', 7), Some(&admin.id_hash), 7)
            .await
            .expect("capped put"),
        1,
        "another origin's id is never replaced, so the cap ends one"
    );
    assert_eq!(live(&store).await, [hash('9'), hash('a'), hash('b')]);

    refused_for("duplicate id hash", put(&capped('b', 8), None, 8).await);
    let mut superseded = capped('c', 8);
    superseded.superseded_until = Some(at(10));
    refused_for("starts superseded", put(&superseded, None, 8).await);
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
            load(&store, &kept.id_hash).await,
            Some(kept.clone()),
            "{case}"
        );
    }
}
