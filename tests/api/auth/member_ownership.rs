//! Weekly-timing ownership on the public origin (user decision 2026-10-10):
//! the member's timings with their ownership, and hand-off, ask, accept,
//! decline and withdraw behind the member session, over the admin reads'
//! seeded store and writer. The members here have snowflake ids, since a
//! hand-off names its receiver by one.

use std::collections::BTreeSet;

use chrono::{NaiveTime, TimeDelta, TimeZone, Utc, Weekday};
use kanade::{
    api::auth::{audit::AuditEvent, rate::MEMBER_WRITE_ROUTE},
    domain::{
        attendance::AttendanceDefault,
        history::{Actor, ChangeMeta, Origin, Surface},
        members::{GatewayMember, MemberStore},
        schedule::{Change, ChangeSet, FixedRun},
        scheduler::{ScheduleStore, Scope},
    },
};
use serde_json::{Value, json};

use super::member_support::{Browser, MemberHarness, Options, PUB_ORIGIN, discord_user};
use crate::{
    reads::Reads,
    schemas::assert_valid,
    support::{PUBLIC_HOST, Reply, send},
};

const IP: &str = "198.51.100.10";
const TIMINGS: &str = "/api/public/timings";
const FIXED: &str = "f-own";
/// The party (Aki owns it as its first member) and an outsider.
const AKI: u64 = 300_000_000_000_000_001;
const BEN: u64 = 300_000_000_000_000_002;
const CHO: u64 = 300_000_000_000_000_003;
const DEE: u64 = 300_000_000_000_000_004;

const TIMINGS_KEYS: [&str; 2] = ["generated_at", "timings"];
const TIMING_KEYS: [&str; 9] = [
    "bosses",
    "id",
    "owner",
    "owner_pinned",
    "party",
    "requests",
    "time",
    "weekday",
    "you_own",
];
const REQUEST_KEYS: [&str; 6] = [
    "created_at",
    "expires_at",
    "id",
    "mine",
    "requester",
    "status",
];

fn id(member: u64) -> String {
    member.to_string()
}

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn owner_path(fixed: &str) -> String {
    format!("/api/public/timings/{fixed}/owner")
}

fn ask_path(fixed: &str) -> String {
    format!("/api/public/timings/{fixed}/owner-requests")
}

fn decide_path(request: &str, action: &str) -> String {
    format!("/api/public/owner-requests/{request}/{action}")
}

fn to(member: u64) -> String {
    json!({ "to": id(member) }).to_string()
}

/// The public origin over `reads`' state, with one weekly timing `f-own`
/// (Aki, Ben, Cho; Aki owns it as its first member) and Dee off its party.
struct Portal {
    reads: Reads,
    harness: MemberHarness,
}

impl Portal {
    async fn new() -> Self {
        let reads = Reads::new().await;
        let harness = MemberHarness::with(Options {
            state: reads.site.state.clone(),
            boss_dir: reads.site.boss_dir.clone(),
            ..Options::default()
        })
        .await;
        for (member, name) in [(AKI, "Aki"), (BEN, "Ben"), (CHO, "Cho"), (DEE, "Dee")] {
            harness.roster(member, true).await;
            reads
                .store
                .apply_gateway(GatewayMember {
                    user_id: id(member),
                    display_name: Some(name.into()),
                    nickname: None,
                    has_role: true,
                    is_bot: false,
                    roles: vec!["10".into()],
                    is_guild_admin: false,
                })
                .await
                .unwrap();
        }
        let fixed = FixedRun {
            id: FIXED.into(),
            owner_id: id(AKI),
            owner_pinned: false,
            channel_id: Some("kalos-four".into()),
            bosses: vec!["XKalos".into()],
            weekday: Weekday::Wed,
            time: NaiveTime::from_hms_opt(21, 0, 0).unwrap(),
            participants: vec![id(AKI), id(BEN), id(CHO)],
            note: Some("private note".into()),
            attendance_default: AttendanceDefault::default(),
            standing: Vec::new(),
        };
        let revision = reads.store.load(&Scope::All).await.unwrap().revision;
        reads
            .store
            .commit(
                revision,
                ChangeSet {
                    changes: vec![Change::PutFixedRun(fixed)],
                },
                ChangeMeta {
                    origin: Origin::new(Actor::admin("test"), Surface::AdminPortal),
                    at: Utc.with_ymd_and_hms(2026, 9, 29, 3, 0, 0).unwrap(),
                    notices: Vec::new(),
                    refs: Vec::new(),
                    request_digest: None,
                    expect: Default::default(),
                    outbox: Vec::new(),
                },
            )
            .await
            .unwrap();
        Self { reads, harness }
    }

    async fn sign_in(&self, member: u64) -> Browser {
        self.harness
            .sign_in_from(discord_user(member, "Member"), IP)
            .await
    }

    async fn get(&self, browser: Option<&Browser>, path: &str) -> Reply {
        let cookie = browser.map(Browser::cookie);
        let mut headers = vec![("CF-Connecting-IP", IP)];
        if let Some((name, value)) = &cookie {
            headers.push((name, value));
        }
        self.harness.get(path, &headers).await
    }

    /// `GET /api/public/timings`: 200, `no-store`, valid, exact keys.
    async fn timings(&self, browser: &Browser) -> Value {
        let reply = self.get(Some(browser), TIMINGS).await;
        assert_eq!(reply.status, 200, "{}", reply.text());
        assert_eq!(reply.header("cache-control"), Some("no-store"));
        let value = reply.json();
        assert_valid("public.json#/$defs/MemberTimings", TIMINGS, &value);
        assert_eq!(keys(&value), TIMINGS_KEYS.into());
        for timing in value["timings"].as_array().unwrap() {
            assert_eq!(keys(timing), TIMING_KEYS.into(), "{timing}");
            for request in timing["requests"].as_array().unwrap() {
                assert_eq!(keys(request), REQUEST_KEYS.into(), "{request}");
            }
        }
        value
    }

    /// `f-own` as `browser` sees it, if they are on it.
    async fn own(&self, browser: &Browser) -> Option<Value> {
        self.timings(browser).await["timings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|timing| timing["id"] == FIXED)
            .cloned()
    }

    /// A member write with every marker unless `headers` overrides them:
    /// session cookie, same origin, CSRF token and `key`.
    async fn send(
        &self,
        browser: Option<&Browser>,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> Reply {
        let cookie = browser.map(Browser::cookie);
        let mut all = vec![("CF-Connecting-IP", IP)];
        if let Some((name, value)) = &cookie {
            all.push((name, value));
        }
        all.extend_from_slice(headers);
        if body.is_none() {
            all.push(("Content-Length", "0"));
        }
        send(self.harness.public, "POST", PUBLIC_HOST, path, &all, body).await
    }

    async fn post(&self, browser: &Browser, path: &str, key: &str, body: Option<&str>) -> Reply {
        self.send(
            Some(browser),
            path,
            &[
                PUB_ORIGIN,
                ("X-Kanade-CSRF", &browser.csrf),
                ("Idempotency-Key", key),
            ],
            body,
        )
        .await
    }

    /// Ask for `f-own`: 201 with the new open request.
    async fn ask(&self, browser: &Browser, key: &str) -> Value {
        let reply = self.post(browser, &ask_path(FIXED), key, None).await;
        assert_eq!(reply.status, 201, "{}", reply.text());
        request_body(&reply)
    }
}

fn request_body(reply: &Reply) -> Value {
    let value = reply.json();
    assert_valid("public.json#/$defs/MemberOwnerRequest", "request", &value);
    assert_eq!(keys(&value), REQUEST_KEYS.into());
    value
}

fn refused(reply: &Reply) -> (u16, String) {
    (reply.status, reply.api_error())
}

fn ids(timing: &Value) -> Vec<&str> {
    timing["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["id"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn timings_show_the_party_their_ownership_and_only_their_own_requests() {
    let portal = Portal::new().await;
    let (aki, ben, cho, dee) = (
        portal.sign_in(AKI).await,
        portal.sign_in(BEN).await,
        portal.sign_in(CHO).await,
        portal.sign_in(DEE).await,
    );

    let mine = portal.own(&aki).await.expect("Aki is on it");
    assert_eq!(
        mine,
        json!({
            "id": FIXED,
            "bosses": mine["bosses"].clone(),
            "weekday": 2,
            "time": "21:00",
            "party": [
                {"id": id(AKI), "name": "Aki"},
                {"id": id(BEN), "name": "Ben"},
                {"id": id(CHO), "name": "Cho"},
            ],
            "owner": {"id": id(AKI), "name": "Aki"},
            "owner_pinned": false,
            "you_own": true,
            "requests": [],
        })
    );
    assert_eq!(mine["bosses"].as_array().unwrap().len(), 1);
    let listed = portal.timings(&aki).await;
    assert_eq!(
        listed["timings"].as_array().unwrap().len(),
        1,
        "the seeded f-kalos party is someone else's"
    );
    assert_eq!(portal.own(&ben).await.unwrap()["you_own"], false);
    assert_eq!(
        portal.timings(&dee).await["timings"],
        json!([]),
        "off the party: nothing"
    );

    // Ben and Cho ask: each sees their own, the owner sees both.
    let bens = portal.ask(&ben, "ben-1").await;
    let chos = portal.ask(&cho, "cho-1").await;
    assert_eq!(bens["requester"], json!({"id": id(BEN), "name": "Ben"}));
    assert_eq!(
        (bens["status"].clone(), bens["mine"].clone()),
        (json!("open"), json!(true))
    );
    let (ben_id, cho_id) = (bens["id"].as_str().unwrap(), chos["id"].as_str().unwrap());
    let theirs = portal.own(&ben).await.unwrap();
    assert_eq!(ids(&theirs), [ben_id]);
    assert_eq!(
        theirs["requests"][0], bens,
        "the read shows what the ask answered"
    );
    assert_eq!(ids(&portal.own(&cho).await.unwrap()), [cho_id]);
    let owners = portal.own(&aki).await.unwrap();
    let mut both = ids(&owners);
    both.sort_unstable();
    let mut expected = [ben_id, cho_id];
    expected.sort_unstable();
    assert_eq!(both, expected);
    assert!(
        owners["requests"]
            .as_array()
            .unwrap()
            .iter()
            .all(|request| request["mine"] == false)
    );

    // Admin routes are never on the public origin, nor these on the admin one.
    let admin = portal.harness.admin_get(TIMINGS, &[]).await;
    assert_eq!(refused(&admin), (404, "not_found".into()));
    // Unmounted methods on the new paths.
    let reply = portal.post(&aki, TIMINGS, "k", None).await;
    assert_eq!(refused(&reply), (404, "not_found".into()));
    let reply = portal.get(Some(&aki), &owner_path(FIXED)).await;
    assert_eq!(refused(&reply), (404, "not_found".into()));
}

#[tokio::test]
async fn the_owner_hands_off_at_once_and_only_within_the_party() {
    let portal = Portal::new().await;
    let (aki, ben) = (portal.sign_in(AKI).await, portal.sign_in(BEN).await);
    let pending = portal.ask(&portal.sign_in(CHO).await, "cho-1").await;
    let version = portal.reads.version().await;

    let not_owner = portal
        .post(&ben, &owner_path(FIXED), "h-1", Some(&to(CHO)))
        .await;
    assert_eq!(refused(&not_owner), (403, "not_owner".into()));
    let outsider = portal
        .post(&aki, &owner_path(FIXED), "h-2", Some(&to(DEE)))
        .await;
    assert_eq!(refused(&outsider), (409, "not_on_party".into()));
    let unknown = portal
        .post(&aki, &owner_path("f-none"), "h-3", Some(&to(BEN)))
        .await;
    assert_eq!(refused(&unknown), (404, "not_found".into()));
    for body in [
        r#"{"to":"1002"}"#.to_owned(),
        r#"{"to":"30000000000000000x"}"#.to_owned(),
        r#"{"to":3000000000000000002}"#.to_owned(),
        json!({"to": id(BEN), "staff": true}).to_string(),
        "{}".to_owned(),
        "not json".to_owned(),
    ] {
        let reply = portal
            .post(&aki, &owner_path(FIXED), "h-4", Some(&body))
            .await;
        assert_eq!(refused(&reply), (422, "invalid_body".into()), "{body}");
    }
    assert_eq!(portal.reads.version().await, version, "nothing written");

    let reply = portal
        .post(&aki, &owner_path(FIXED), "h-5", Some(&to(BEN)))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let handed = reply.json();
    assert_valid("public.json#/$defs/MemberTiming", "hand-off", &handed);
    assert_eq!(handed["owner"], json!({"id": id(BEN), "name": "Ben"}));
    assert_eq!(
        (handed["owner_pinned"].clone(), handed["you_own"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(handed["requests"], json!([]), "Cho's request went with it");
    assert_eq!(
        portal.reads.version().await,
        version + 1,
        "one recorded edit"
    );
    let bens = portal.own(&ben).await.unwrap();
    assert_eq!((bens["you_own"].clone(), ids(&bens)), (json!(true), vec![]));
    let withdrawn = portal
        .post(
            &portal.sign_in(CHO).await,
            &decide_path(pending["id"].as_str().unwrap(), "withdraw"),
            "w-1",
            None,
        )
        .await;
    assert_eq!(
        refused(&withdrawn),
        (409, "request_closed".into()),
        "superseded"
    );

    // A retry with the same key replays; Aki no longer owns it otherwise.
    let replay = portal
        .post(&aki, &owner_path(FIXED), "h-5", Some(&to(BEN)))
        .await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(portal.reads.version().await, version + 1);
    let again = portal
        .post(&aki, &owner_path(FIXED), "h-6", Some(&to(CHO)))
        .await;
    assert_eq!(refused(&again), (403, "not_owner".into()));

    // Past the fresh-write window the new owner must sign in again.
    portal.harness.advance(TimeDelta::minutes(16));
    let stale = portal
        .post(&ben, &owner_path(FIXED), "h-7", Some(&to(CHO)))
        .await;
    assert_eq!(refused(&stale), (401, "reauth_required".into()));
    let fresh = portal.sign_in(BEN).await;
    let reply = portal
        .post(&fresh, &owner_path(FIXED), "h-7", Some(&to(CHO)))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(reply.json()["owner"]["id"], id(CHO));
}

#[tokio::test]
async fn an_ask_replays_by_key_and_the_owner_accepts_it() {
    let portal = Portal::new().await;
    let (aki, ben, cho, dee) = (
        portal.sign_in(AKI).await,
        portal.sign_in(BEN).await,
        portal.sign_in(CHO).await,
        portal.sign_in(DEE).await,
    );
    let first = portal.ask(&ben, "ben-1").await;
    let replay = portal.post(&ben, &ask_path(FIXED), "ben-1", None).await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(request_body(&replay), first, "the same request");
    let second = portal.post(&ben, &ask_path(FIXED), "ben-2", None).await;
    assert_eq!(refused(&second), (409, "already_asked".into()));
    let elsewhere = portal.post(&ben, &ask_path("f-kalos"), "ben-1", None).await;
    assert_eq!(refused(&elsewhere), (422, "idempotency_mismatch".into()));
    for (browser, code) in [(&dee, "not_on_party"), (&aki, "already_owner")] {
        let reply = portal.post(browser, &ask_path(FIXED), "x-1", None).await;
        assert_eq!(refused(&reply), (409, code.into()));
    }
    let unknown = portal.post(&ben, &ask_path("f-none"), "ben-3", None).await;
    assert_eq!(refused(&unknown), (404, "not_found".into()));
    let chos = portal.ask(&cho, "cho-1").await;
    let (ben_id, cho_id) = (first["id"].as_str().unwrap(), chos["id"].as_str().unwrap());
    assert!(
        ben_id.len() <= 64
            && ben_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "{ben_id}"
    );
    assert_ne!(ben_id, cho_id, "keys are per member");

    // Only the owner decides: not the requester, nor another party member.
    for browser in [&ben, &cho] {
        let reply = portal
            .post(browser, &decide_path(ben_id, "accept"), "a-1", None)
            .await;
        assert_eq!(refused(&reply), (403, "not_owner".into()));
    }
    let missing = portal
        .post(&aki, &decide_path("public-none", "accept"), "a-2", None)
        .await;
    assert_eq!(refused(&missing), (404, "not_found".into()));
    let version = portal.reads.version().await;
    let accepted = portal
        .post(&aki, &decide_path(ben_id, "accept"), "a-3", None)
        .await;
    assert_eq!(accepted.status, 200, "{}", accepted.text());
    let accepted = request_body(&accepted);
    assert_eq!(
        (accepted["status"].clone(), accepted["mine"].clone()),
        (json!("accepted"), json!(false))
    );
    assert_eq!(
        portal.reads.version().await,
        version + 1,
        "the owner is pinned"
    );
    let bens = portal.own(&ben).await.unwrap();
    assert_eq!(bens["owner"]["id"], id(BEN));
    assert_eq!(
        (bens["owner_pinned"].clone(), bens["you_own"].clone()),
        (json!(true), json!(true))
    );
    assert_eq!(
        ids(&bens),
        Vec::<&str>::new(),
        "Cho's request was superseded"
    );
    assert_eq!(ids(&portal.own(&cho).await.unwrap()), Vec::<&str>::new());

    // A closed request: its owner hears so, anyone else only that they may not.
    let closed = portal
        .post(&ben, &decide_path(cho_id, "accept"), "a-4", None)
        .await;
    assert_eq!(refused(&closed), (409, "request_closed".into()));
    let former = portal
        .post(&aki, &decide_path(cho_id, "decline"), "d-1", None)
        .await;
    assert_eq!(refused(&former), (403, "not_owner".into()));
}

#[tokio::test]
async fn decline_and_withdraw_belong_to_the_owner_and_the_requester() {
    let portal = Portal::new().await;
    let (aki, ben, cho) = (
        portal.sign_in(AKI).await,
        portal.sign_in(BEN).await,
        portal.sign_in(CHO).await,
    );
    // Asking, declining and withdrawing need no fresh sign-in; accepting does.
    portal.harness.advance(TimeDelta::minutes(16));
    let asked = portal.ask(&ben, "ben-1").await;
    let request = asked["id"].as_str().unwrap();
    let stale = portal
        .post(&aki, &decide_path(request, "accept"), "a-1", None)
        .await;
    assert_eq!(refused(&stale), (401, "reauth_required".into()));

    let declined_by_cho = portal
        .post(&cho, &decide_path(request, "decline"), "d-1", None)
        .await;
    assert_eq!(refused(&declined_by_cho), (403, "not_owner".into()));
    for browser in [&aki, &cho] {
        let reply = portal
            .post(browser, &decide_path(request, "withdraw"), "w-1", None)
            .await;
        assert_eq!(refused(&reply), (403, "not_requester".into()));
    }
    let withdrawn = portal
        .post(&ben, &decide_path(request, "withdraw"), "w-2", None)
        .await;
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.text());
    assert_eq!(request_body(&withdrawn)["status"], "withdrawn");
    let twice = portal
        .post(&ben, &decide_path(request, "withdraw"), "w-3", None)
        .await;
    assert_eq!(refused(&twice), (409, "request_closed".into()));

    let again = portal.ask(&ben, "ben-2").await;
    let request = again["id"].as_str().unwrap();
    let version = portal.reads.version().await;
    let declined = portal
        .post(&aki, &decide_path(request, "decline"), "d-2", None)
        .await;
    assert_eq!(declined.status, 200, "{}", declined.text());
    assert_eq!(request_body(&declined)["status"], "declined");
    assert_eq!(
        portal.reads.version().await,
        version,
        "declining writes no edit"
    );
    let owners = portal.own(&aki).await.unwrap();
    assert_eq!(
        (owners["you_own"].clone(), ids(&owners)),
        (json!(true), vec![])
    );
}

#[tokio::test]
async fn member_writes_need_a_key_csrf_a_session_and_an_open_portal() {
    let portal = Portal::new().await;
    let ben = portal.sign_in(BEN).await;
    let ask = ask_path(FIXED);
    let csrf = ("X-Kanade-CSRF", ben.csrf.as_str());

    for (headers, what) in [
        (vec![PUB_ORIGIN, csrf], "no key"),
        (vec![PUB_ORIGIN, csrf, ("Idempotency-Key", "")], "empty key"),
        (
            vec![PUB_ORIGIN, csrf, ("Idempotency-Key", "a b")],
            "bad key",
        ),
        (
            vec![
                PUB_ORIGIN,
                csrf,
                ("Idempotency-Key", "k-1"),
                ("Idempotency-Key", "k-2"),
            ],
            "two keys",
        ),
    ] {
        let reply = portal.send(Some(&ben), &ask, &headers, None).await;
        assert_eq!(
            refused(&reply),
            (400, "invalid_idempotency_key".into()),
            "{what}"
        );
    }
    let long = "k".repeat(129);
    let reply = portal
        .send(
            Some(&ben),
            &ask,
            &[PUB_ORIGIN, csrf, ("Idempotency-Key", &long)],
            None,
        )
        .await;
    assert_eq!(refused(&reply), (400, "invalid_idempotency_key".into()));

    for (headers, what) in [
        (vec![PUB_ORIGIN, ("Idempotency-Key", "k-1")], "no token"),
        (vec![csrf, ("Idempotency-Key", "k-1")], "no origin"),
        (
            vec![
                ("Origin", "https://evil.example"),
                csrf,
                ("Idempotency-Key", "k-1"),
            ],
            "foreign origin",
        ),
    ] {
        let reply = portal.send(Some(&ben), &ask, &headers, None).await;
        assert_eq!(refused(&reply), (403, "csrf".into()), "{what}");
    }
    assert_eq!(
        portal.own(&ben).await.unwrap()["requests"],
        json!([]),
        "nothing was asked"
    );

    let paths = [
        owner_path(FIXED),
        ask.clone(),
        decide_path("r", "accept"),
        decide_path("r", "decline"),
        decide_path("r", "withdraw"),
    ];
    for path in &paths {
        let reply = portal
            .send(None, path, &[PUB_ORIGIN, ("Idempotency-Key", "k-1")], None)
            .await;
        assert_eq!(refused(&reply), (401, "unauthenticated".into()), "{path}");
    }
    let reply = portal.get(None, TIMINGS).await;
    assert_eq!(refused(&reply), (401, "unauthenticated".into()));

    portal.harness.set_open(false);
    for path in &paths {
        let reply = portal.post(&ben, path, "k-1", Some(&to(CHO))).await;
        assert_eq!(refused(&reply), (503, "closed".into()), "{path}");
    }
    let reply = portal.get(Some(&ben), TIMINGS).await;
    assert_eq!(refused(&reply), (503, "closed".into()));
}

#[tokio::test]
async fn member_writes_are_rate_limited_per_member() {
    let portal = Portal::new().await;
    let (ben, cho) = (portal.sign_in(BEN).await, portal.sign_in(CHO).await);
    let path = decide_path("public-none", "withdraw");
    for n in 0..20 {
        let reply = portal.post(&ben, &path, &format!("w-{n}"), None).await;
        assert_eq!(refused(&reply), (404, "not_found".into()), "write {n}");
    }
    let limited = portal.post(&ben, &path, "w-20", None).await;
    assert_eq!(refused(&limited), (429, "rate_limited".into()));
    // Every write counts, even a well-formed ask.
    let ask = portal.post(&ben, &ask_path(FIXED), "ben-1", None).await;
    assert_eq!(refused(&ask), (429, "rate_limited".into()));
    assert!(
        portal
            .harness
            .audit
            .events()
            .contains(&AuditEvent::RateLimited {
                route: MEMBER_WRITE_ROUTE
            })
    );
    // Reads and other members are not limited by it.
    assert!(portal.own(&ben).await.is_some());
    portal.ask(&cho, "cho-1").await;
    // Two writes a minute come back.
    portal.harness.advance(TimeDelta::seconds(30));
    let reply = portal.post(&ben, &path, "w-21", None).await;
    assert_eq!(refused(&reply), (404, "not_found".into()));
}
