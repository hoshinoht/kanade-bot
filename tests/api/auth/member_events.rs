//! `GET /api/public/events`: member-scoped change hints over SSE, behind the
//! open portal and the member session, over the admin reads' seeded store
//! (r-kalos this week for Alice 1001, Bob 1002 and Dan 1004; Cara 1003 is
//! on the weekly timing and next week's run only). Frames carry `{topic}`
//! and `{boot}` only; `mine` reaches only the members a change touched.

use std::time::Duration;

use chrono::TimeDelta;
use kanade::infrastructure::store::web_sessions::WebSession;
use serde_json::{Value, json};
use tokio::time::Instant;

use super::{
    member_support::{Browser, PUB_ORIGIN},
    member_writes::{ALICE, BOB, CARA, DAN, Portal},
};
use crate::{
    events::Stream,
    reads::EVENTS,
    schemas::assert_valid,
    support::{ADMIN_HOST, PUBLIC_HOST, request},
};

const PATH: &str = "/api/public/events";
const IP: &str = "198.51.100.10";
/// Long enough for a hint to arrive, well under any poll.
const HINT: Duration = Duration::from_secs(5);
/// A window in which nothing more may arrive.
const QUIET: Duration = Duration::from_millis(600);

async fn open(portal: &Portal, browser: &Browser) -> Stream {
    let (name, value) = browser.cookie();
    Stream::connect(
        portal.harness.public,
        PUBLIC_HOST,
        PATH,
        &[(name, value.as_str()), ("CF-Connecting-IP", IP)],
    )
    .await
}

/// A stream that opened: 200, and its `ready` holds `boot` only.
async fn live(portal: &Portal, browser: &Browser) -> Member {
    let mut stream = open(portal, browser).await;
    assert_eq!(stream.status, 200, "{}", stream.head);
    let block = stream.next(HINT).await.expect("ready");
    let mut lines = block.lines();
    assert_eq!(lines.next(), Some("retry: 3000"));
    assert_eq!(lines.next(), Some("event: ready"));
    let ready: Value =
        serde_json::from_str(lines.next().unwrap().strip_prefix("data: ").unwrap()).unwrap();
    assert_eq!(keys(&ready), ["boot"], "{ready}");
    assert_valid("public.json#/$defs/MemberEventReady", "ready", &ready);
    assert_eq!(lines.next(), None);
    Member {
        stream,
        boot: ready["boot"].as_str().unwrap().to_owned(),
        seen: Vec::new(),
    }
}

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

/// One member's open stream and every frame it sent.
struct Member {
    stream: Stream,
    boot: String,
    seen: Vec<String>,
}

impl Member {
    /// The topics of the hints that arrive within `window` (a burst is read
    /// whole); each hint is `{topic}` only.
    async fn topics(&mut self, window: Duration) -> Vec<String> {
        let mut topics = Vec::new();
        let mut until = Instant::now() + window;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let Some(block) = self.next_within(left).await else {
                return topics;
            };
            self.seen.push(block.clone());
            if block.starts_with(':') {
                continue;
            }
            let data = block
                .strip_prefix("data: ")
                .unwrap_or_else(|| panic!("a message event: {block:?}"));
            let hint: Value = serde_json::from_str(data).unwrap();
            assert_eq!(keys(&hint), ["topic"], "no data in a hint: {hint}");
            assert_valid("public.json#/$defs/MemberEventHint", "hint", &hint);
            topics.push(hint["topic"].as_str().unwrap().to_owned());
            // The rest of a burst follows at once.
            until = until.min(Instant::now() + QUIET);
        }
    }

    /// The next block, or `None` when `wait` passes (or the stream ended).
    async fn next_within(&mut self, wait: Duration) -> Option<String> {
        tokio::time::timeout(wait, self.stream.next(wait + Duration::from_secs(1)))
            .await
            .ok()
            .flatten()
    }

    /// No frame so far named any seeded member (the boot id aside).
    fn named_nobody(&self) {
        for block in &self.seen {
            let block = block.replace(&self.boot, "");
            for id in [ALICE, BOB, CARA, DAN] {
                assert!(!block.contains(&id.to_string()), "{id} in {block:?}");
            }
        }
    }

    async fn ends(&mut self) {
        self.stream.ends_within(EVENTS.heartbeat * 8).await;
    }
}

#[tokio::test]
async fn the_stream_needs_an_open_portal_and_a_session() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let reply = portal.get(None, PATH).await;
    assert_eq!(
        (reply.status, reply.api_error()),
        (401, "unauthenticated".into())
    );

    portal.harness.set_open(false);
    for browser in [None, Some(&alice)] {
        let reply = portal.get(browser, PATH).await;
        assert_eq!((reply.status, reply.api_error()), (503, "closed".into()));
    }
    portal.harness.set_open(true);

    // An admin session means nothing here, and the admin origin has no such route.
    let admin = portal
        .harness
        .get(
            PATH,
            &[("Cookie", &portal.reads.cookie), ("CF-Connecting-IP", IP)],
        )
        .await;
    assert_eq!(admin.status, 401);
    let reply = request(
        portal.reads.admin,
        "GET",
        ADMIN_HOST,
        PATH,
        &[("Cookie", &portal.reads.cookie)],
    )
    .await;
    assert_eq!(reply.status, 404);

    let member = live(&portal, &alice).await;
    assert_eq!(
        member.stream.header("content-type"),
        Some("text/event-stream")
    );
    assert_eq!(member.stream.header("x-accel-buffering"), Some("no"));
    assert_eq!(
        member.stream.header("cache-control"),
        Some("no-store, no-transform"),
        "{}",
        member.stream.head
    );
}

#[tokio::test]
async fn a_stream_heartbeats_and_hints_nothing_while_nothing_changes() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let mut member = live(&portal, &alice).await;
    for _ in 0..3 {
        let beat = member.stream.next(EVENTS.heartbeat * 4).await.unwrap();
        assert_eq!(beat, ": keep-alive");
    }
    // A write no portal page reads hints nothing.
    portal
        .reads
        .ok(
            "PATCH",
            "/api/admin/members/1006",
            json!({"ping_level": "all"}),
            "members.json#/$defs/MemberRow",
        )
        .await;
    assert!(member.topics(QUIET).await.is_empty());
}

#[tokio::test]
async fn a_member_write_hints_the_writer_mine_and_others_schedule() {
    let portal = Portal::new().await;
    let dan = portal.sign_in(DAN).await;
    let cara = portal.sign_in(CARA).await;
    let mut writer = live(&portal, &dan).await;
    let mut other = live(&portal, &cara).await;

    let version = portal.version().await;
    let reply = portal
        .write(
            &dan,
            "PUT",
            "/api/public/runs/r-kalos/answer",
            "a-1",
            Some(&json!({"answer": "yes", "version": version})),
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(writer.topics(HINT).await, ["schedule", "mine"]);
    assert_eq!(other.topics(HINT).await, ["schedule"], "not on r-kalos");
    writer.named_nobody();
    other.named_nobody();
}

#[tokio::test]
async fn an_admin_edit_hints_mine_only_to_the_members_it_touched() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let cara = portal.sign_in(CARA).await;
    let mut on_it = live(&portal, &alice).await;
    let mut off_it = live(&portal, &cara).await;

    let version = portal.version().await;
    portal
        .reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 6, "time": "21:00", "version": version}),
            "week.json#/$defs/MoveResult",
        )
        .await;
    assert_eq!(on_it.topics(HINT).await, ["schedule", "mine"]);
    assert_eq!(off_it.topics(HINT).await, ["schedule"]);

    // Taking Bob off the run changes Alice's run too, never Cara's.
    let version = portal.version().await;
    portal
        .reads
        .ok(
            "PATCH",
            "/api/admin/runs/r-kalos/participants",
            json!({"remove": BOB.to_string(), "version": version}),
            "week.json#/$defs/RunResult",
        )
        .await;
    assert_eq!(on_it.topics(HINT).await, ["schedule", "mine"]);
    assert_eq!(off_it.topics(HINT).await, ["schedule"]);
    on_it.named_nobody();
    off_it.named_nobody();
}

#[tokio::test]
async fn signing_out_ends_the_stream() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let mut member = live(&portal, &alice).await;
    let reply = portal
        .send(
            Some(&alice),
            "POST",
            "/api/public/auth/logout",
            &[PUB_ORIGIN, ("X-Kanade-CSRF", &alice.csrf)],
            None,
        )
        .await;
    assert_eq!(reply.status, 204, "{}", reply.text());
    member.ends().await;
    let until = Instant::now() + Duration::from_secs(5);
    while portal.reads.events.member_streams() > 0 {
        assert!(Instant::now() < until, "an ended stream releases its place");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn signing_out_everywhere_ends_every_stream_of_the_member() {
    let portal = Portal::new().await;
    let phone = portal.sign_in(ALICE).await;
    let laptop = portal.sign_in(ALICE).await;
    let bob = portal.sign_in(BOB).await;
    let mut first = live(&portal, &phone).await;
    let mut second = live(&portal, &laptop).await;
    let mut others = live(&portal, &bob).await;
    let reply = portal
        .send(
            Some(&laptop),
            "POST",
            "/api/public/sessions/end-all",
            &[PUB_ORIGIN, ("X-Kanade-CSRF", &laptop.csrf)],
            None,
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    first.ends().await;
    second.ends().await;
    assert_eq!(
        others.stream.next(EVENTS.heartbeat * 4).await.as_deref(),
        Some(": keep-alive"),
        "another member's stream stays"
    );
}

#[tokio::test]
async fn idle_expiry_ends_the_stream_which_never_counts_as_activity() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let mut member = live(&portal, &alice).await;
    // 29 minutes on with only the stream open: still live.
    portal.harness.advance(TimeDelta::minutes(29));
    for _ in 0..3 {
        assert_eq!(
            member.stream.next(EVENTS.heartbeat * 4).await.as_deref(),
            Some(": keep-alive")
        );
    }
    // Past the 30-minute idle limit counted from the last request.
    portal.harness.advance(TimeDelta::minutes(2));
    member.ends().await;
    assert_eq!(
        portal.get(Some(&alice), "/api/public/session").await.status,
        401
    );
}

#[tokio::test]
async fn a_reconnect_never_extends_the_idle_window_nor_rotates_the_id() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let seen = |rows: Vec<WebSession>| {
        rows.into_iter()
            .map(|row| (row.id_hash, row.last_seen_at))
            .collect::<Vec<_>>()
    };
    let before = seen(portal.harness.rows(ALICE).await);
    // 29 minutes on, no request but the stream's reconnects: one from a new
    // address (which an ordinary request would rotate on), one as before.
    portal.harness.advance(TimeDelta::minutes(29));
    let (name, value) = alice.cookie();
    let moved = Stream::connect(
        portal.harness.public,
        PUBLIC_HOST,
        PATH,
        &[(name, value.as_str()), ("CF-Connecting-IP", "203.0.113.7")],
    )
    .await;
    assert_eq!(moved.status, 200, "{}", moved.head);
    assert_eq!(moved.header("set-cookie"), None, "{}", moved.head);
    drop(moved);
    drop(live(&portal, &alice).await);
    assert_eq!(seen(portal.harness.rows(ALICE).await), before);
    // Touched at 29 minutes, the session would live until 59; it idles out
    // at 30, counted from the last ordinary request.
    portal.harness.advance(TimeDelta::minutes(2));
    let refused = open(&portal, &alice).await;
    assert_eq!(refused.status, 401, "{}", refused.head);
    assert!(portal.harness.rows(ALICE).await.is_empty());
}

#[tokio::test]
async fn absolute_expiry_ends_the_stream_of_a_busy_session() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let mut member = live(&portal, &alice).await;
    // Reads every 29 minutes keep the session from idling out for 7 h 44 min.
    for _ in 0..16 {
        portal.harness.advance(TimeDelta::minutes(29));
        let reply = portal.get(Some(&alice), "/api/public/session").await;
        assert_eq!(reply.status, 200, "{}", reply.text());
    }
    assert_eq!(
        member.topics(EVENTS.heartbeat * 4).await,
        Vec::<String>::new(),
        "live, hinting nothing"
    );
    // Past the 8-hour absolute limit, 20 minutes after the last read.
    portal.harness.advance(TimeDelta::minutes(20));
    member.ends().await;
}

#[tokio::test]
async fn losing_the_role_ends_the_stream_and_the_sessions() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let mut member = live(&portal, &alice).await;
    portal.harness.roster(ALICE, false).await;
    member.ends().await;
    assert!(portal.harness.rows(ALICE).await.is_empty());
    assert_eq!(portal.get(Some(&alice), PATH).await.status, 401);
}

#[tokio::test]
async fn streams_are_capped_per_member_and_overall_apart_from_admins() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let bob = portal.sign_in(BOB).await;
    let dan = portal.sign_in(DAN).await;
    let mut alice_streams = Vec::new();
    for _ in 0..3 {
        alice_streams.push(live(&portal, &alice).await);
    }
    let refused = portal.get(Some(&alice), PATH).await;
    assert_eq!(
        (refused.status, refused.api_error()),
        (429, "too_many_streams".into()),
        "three per member"
    );
    let _bob = live(&portal, &bob).await;
    let refused = portal.get(Some(&dan), PATH).await;
    assert_eq!(
        (refused.status, refused.api_error()),
        (429, "too_many_streams".into()),
        "the overall cap"
    );
    assert_eq!(
        portal.reads.events.member_streams(),
        EVENTS.max_member_clients
    );

    // Admin streams have their own places, and still send `{topic, seq}`.
    let mut admin = Stream::open(portal.reads.admin, &portal.reads.cookie).await;
    assert_eq!(admin.status, 200);
    let ready = admin.ready().await;
    let second = Stream::open(portal.reads.admin, &portal.reads.cookie).await;
    assert_eq!(second.status, 200);
    assert_eq!(portal.reads.events.streams(), EVENTS.max_clients);
    let version = portal.version().await;
    portal
        .reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 6, "time": "21:00", "version": version}),
            "week.json#/$defs/MoveResult",
        )
        .await;
    assert_eq!(
        admin.hint().await,
        json!({"topic": "schedule", "seq": ready + 1})
    );

    // A closed stream frees its member's place.
    drop(alice_streams.pop());
    let until = Instant::now() + Duration::from_secs(5);
    while portal.reads.events.member_streams() == EVENTS.max_member_clients {
        assert!(Instant::now() < until, "a closed stream releases its place");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    live(&portal, &alice).await;
}

#[tokio::test]
async fn a_stream_takes_one_read_token_when_it_opens() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let mut member = live(&portal, &alice).await;
    for _ in 0..4 {
        member.stream.next(EVENTS.heartbeat * 4).await.unwrap();
    }
    // 120 a minute: the stream took one, its heartbeats none.
    for n in 0..119 {
        let reply = portal.get(Some(&alice), "/api/public/me/allowance").await;
        assert_eq!(reply.status, 200, "read {n}: {}", reply.text());
    }
    let refused = portal.get(Some(&alice), "/api/public/me/allowance").await;
    assert_eq!(
        (refused.status, refused.api_error()),
        (429, "rate_limited".into())
    );
}
