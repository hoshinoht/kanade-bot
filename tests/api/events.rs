//! `GET /api/admin/events`: change hints over SSE. Admin listener only,
//! session-bound, bounded, heartbeating, ended by sign-out, expiry and
//! shutdown, and fed by the store's real commit seams with `{topic, seq}` only.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use chrono::{NaiveTime, TimeDelta, TimeZone, Utc, Weekday};
use kanade::{
    api::{
        server::{LiveAdmin, serve},
        write::ApiClock,
    },
    domain::{
        drafts::ProposalSource,
        ids::RandomIds,
        members::{MemberStore, Roster},
        notify::{
            Claim, DeliveryJournal, DeliveryTarget, EffectKind, IntentContent, NotificationIntent,
            Receipt,
        },
        proposals::{ChangeKind, ProposedChange},
        schedule::{ReminderPolicy, SchedulePolicy},
        scheduler::{ProposalRequest, SchedulerService, Supersede},
    },
    runtime::{
        application::{HealthFuture, HealthProbe, OfflineApplication},
        config::{HttpConfig, RuntimeConfig},
    },
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{Instant, timeout},
};

use crate::{
    reads::{EVENTS, Reads},
    support::{ADMIN_HOST, Fixture, get, public, request},
};

const EVENTS_PATH: &str = "/api/admin/events";

/// One open event stream read frame by frame (HTTP/1.1 chunked).
pub(crate) struct Stream {
    socket: TcpStream,
    raw: Vec<u8>,
    text: String,
    ended: bool,
    pub status: u16,
    pub head: String,
}

impl Stream {
    pub(crate) async fn open(address: SocketAddr, cookie: &str) -> Self {
        let mut socket = TcpStream::connect(address).await.unwrap();
        let request = format!(
            "GET {EVENTS_PATH} HTTP/1.1\r\nHost: {ADMIN_HOST}\r\nAccept: text/event-stream\r\nCookie: {cookie}\r\nConnection: close\r\n\r\n"
        );
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut stream = Self {
            socket,
            raw: Vec::new(),
            text: String::new(),
            ended: false,
            status: 0,
            head: String::new(),
        };
        while !stream.raw.windows(4).any(|w| w == b"\r\n\r\n") {
            assert!(stream.fill().await, "the response head arrives");
        }
        let split = stream
            .raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap();
        stream.head = String::from_utf8(stream.raw[..split].to_vec())
            .unwrap()
            .to_ascii_lowercase();
        stream.status = stream.head[9..12].parse().unwrap();
        stream.raw.drain(..split + 4);
        stream
    }

    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.head
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{name}: ")))
    }

    async fn fill(&mut self) -> bool {
        let mut buf = [0u8; 4096];
        let n = self.socket.read(&mut buf).await.unwrap_or(0);
        self.raw.extend_from_slice(&buf[..n]);
        n > 0
    }

    /// Moves every complete chunk into `text`; the terminating chunk ends it.
    fn dechunk(&mut self) {
        loop {
            let Some(line) = self.raw.windows(2).position(|w| w == b"\r\n") else {
                return;
            };
            let size =
                usize::from_str_radix(std::str::from_utf8(&self.raw[..line]).unwrap(), 16).unwrap();
            if size == 0 {
                self.ended = true;
                return;
            }
            if self.raw.len() < line + 2 + size + 2 {
                return;
            }
            let chunk = self.raw[line + 2..line + 2 + size].to_vec();
            self.text.push_str(std::str::from_utf8(&chunk).unwrap());
            self.raw.drain(..line + 2 + size + 2);
        }
    }

    /// The next SSE block (lines up to a blank line), or `None` once the
    /// stream has ended. Panics if nothing arrives within `wait`.
    pub(crate) async fn next(&mut self, wait: Duration) -> Option<String> {
        let until = Instant::now() + wait;
        loop {
            self.dechunk();
            if let Some(end) = self.text.find("\n\n") {
                let block = self.text[..end].to_owned();
                self.text.drain(..end + 2);
                return Some(block);
            }
            if self.ended {
                return None;
            }
            let left = until.saturating_duration_since(Instant::now());
            match timeout(left, self.fill()).await {
                Ok(true) => {}
                Ok(false) => {
                    self.ended = true;
                    return None;
                }
                Err(_) => panic!(
                    "nothing on the stream within {wait:?}; held: {:?}",
                    self.text
                ),
            }
        }
    }

    /// The next hint's JSON, skipping heartbeats; asserts it carries only
    /// `topic` and `seq`.
    pub(crate) async fn hint(&mut self) -> Value {
        loop {
            let block = self.next(Duration::from_secs(5)).await.expect("a hint");
            if block.starts_with(':') {
                continue;
            }
            let data = block
                .strip_prefix("data: ")
                .unwrap_or_else(|| panic!("a message event: {block:?}"));
            let value: Value = serde_json::from_str(data).unwrap();
            let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
            assert_eq!(keys, ["seq", "topic"], "no data in a hint: {value}");
            return value;
        }
    }

    /// The topics of every hint within `window`.
    pub(crate) async fn topics(&mut self, window: Duration) -> Vec<String> {
        let until = Instant::now() + window;
        let mut topics = Vec::new();
        loop {
            self.dechunk();
            if let Some(end) = self.text.find("\n\n") {
                let block = self.text[..end].to_owned();
                self.text.drain(..end + 2);
                if let Some(data) = block.strip_prefix("data: ") {
                    let value: Value = serde_json::from_str(data).unwrap();
                    topics.push(value["topic"].as_str().unwrap().to_owned());
                }
                continue;
            }
            if self.ended {
                return topics;
            }
            match timeout(until.saturating_duration_since(Instant::now()), self.fill()).await {
                Ok(true) => {}
                Ok(false) => self.ended = true,
                Err(_) => return topics,
            }
        }
    }

    /// Reads to the end; panics if the stream is still open after `wait`.
    pub(crate) async fn ends_within(&mut self, wait: Duration) {
        let until = Instant::now() + wait;
        while self
            .next(until.saturating_duration_since(Instant::now()))
            .await
            .is_some()
        {}
    }

    /// The opening `ready` event's seq (after the `retry` line).
    pub(crate) async fn ready(&mut self) -> u64 {
        let block = self.next(Duration::from_secs(5)).await.expect("ready");
        let mut lines = block.lines();
        assert_eq!(lines.next(), Some("retry: 3000"));
        assert_eq!(lines.next(), Some("event: ready"));
        let data: Value =
            serde_json::from_str(lines.next().unwrap().strip_prefix("data: ").unwrap()).unwrap();
        data["seq"].as_u64().unwrap()
    }
}

#[tokio::test]
async fn events_need_a_session_and_are_never_served_on_the_public_origin() {
    let reads = Reads::new().await;
    let reply = request(reads.admin, "GET", ADMIN_HOST, EVENTS_PATH, &[]).await;
    assert_eq!(
        (reply.status, reply.api_error()),
        (401, "unauthenticated".into())
    );

    let fixture = Fixture::new();
    let http = fixture.http();
    let public = public(&http).await;
    let reply = get(public, "kanade-pub.test", EVENTS_PATH).await;
    assert_eq!((reply.status, reply.api_error()), (404, "not_found".into()));
}

#[tokio::test]
async fn a_stream_opens_ready_uncached_and_heartbeats() {
    let reads = Reads::new().await;
    let mut stream = Stream::open(reads.admin, &reads.cookie).await;
    assert_eq!(stream.status, 200);
    assert_eq!(stream.header("content-type"), Some("text/event-stream"));
    assert_eq!(stream.header("cache-control"), Some("no-store"));
    assert_eq!(stream.header("x-accel-buffering"), Some("no"));
    assert_eq!(stream.ready().await, 0, "no hint yet");
    for _ in 0..2 {
        let beat = stream.next(EVENTS.heartbeat * 4).await.unwrap();
        assert_eq!(beat, ": keep-alive");
    }
}

#[tokio::test]
async fn hints_come_from_the_commit_seams_with_topic_and_seq_only() {
    let reads = Reads::new().await;
    let mut stream = Stream::open(reads.admin, &reads.cookie).await;
    let ready = stream.ready().await;

    // Schedule: a portal move commits one history record.
    let version = reads.version().await;
    reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 6, "time": "21:00", "version": version}),
            "week.json#/$defs/MoveResult",
        )
        .await;
    assert_eq!(
        stream.hint().await,
        json!({"topic": "schedule", "seq": ready + 1})
    );

    // Inbox: the extractor's proposal path.
    let at = Utc.with_ymd_and_hms(2026, 9, 29, 3, 0, 0).unwrap();
    let mut roster = Roster::new();
    for profile in reads.store.list_members().await.unwrap() {
        roster.upsert(profile.member);
    }
    let policy = SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::Asia::Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    );
    SchedulerService::new(
        reads.store.clone(),
        RandomIds,
        ApiClock(Arc::new(move || at)),
    )
    .propose(
        ProposalRequest {
            change: ProposedChange {
                run_id: Some("r-star".into()),
                channel_id: Some("kalos-four".into()),
                ..ProposedChange::new(ChangeKind::Cancel)
            },
            source: ProposalSource::Extraction,
            source_id: "log-1".into(),
            supersede: Supersede::Keep,
        },
        &policy,
        &roster,
    )
    .await
    .unwrap();
    assert_eq!(stream.hint().await["topic"], "inbox");

    // Delivery: the tick binds a posted card.
    let posted = Utc.with_ymd_and_hms(2026, 9, 29, 3, 30, 0).unwrap();
    let week = Utc.with_ymd_and_hms(2026, 9, 23, 16, 0, 0).unwrap();
    let store = &reads.store;
    let lease = store
        .begin_lease("events-test", "delivery", posted)
        .await
        .unwrap();
    let intent = NotificationIntent {
        effect: EffectKind::Digest,
        effect_context: Vec::new(),
        channel_id: "star".into(),
        targets: vec![DeliveryTarget::Digest(week)],
        mentions: Vec::new(),
        content: IntentContent::Digest {
            week_start: week,
            inclusion: Default::default(),
        },
        warnings: Vec::new(),
    };
    let Ok(Claim::Fresh(attempt)) = store.claim(&lease, &intent, None, posted).await else {
        panic!("digest claim");
    };
    let receipt = Receipt {
        channel_id: "star".into(),
        message_id: "5150".into(),
    };
    store
        .bind(&lease, &attempt, &receipt, None, posted)
        .await
        .unwrap();
    store.end_lease(&lease, posted).await.unwrap();
    assert_eq!(
        stream.topics(Duration::from_millis(300)).await,
        ["delivery"]
    );

    // A refused write commits nothing and hints nothing.
    let stale = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 2, "time": "21:00", "version": version}),
            &[],
        )
        .await;
    assert_eq!(stale.status, 409);
    assert!(stream.topics(Duration::from_millis(300)).await.is_empty());
}

#[tokio::test]
async fn streams_are_bounded_and_a_closed_one_frees_its_place() {
    let reads = Reads::new().await;
    let mut first = Stream::open(reads.admin, &reads.cookie).await;
    let _second = Stream::open(reads.admin, &reads.cookie).await;
    assert_eq!(first.ready().await, 0);
    let refused = request(
        reads.admin,
        "GET",
        ADMIN_HOST,
        EVENTS_PATH,
        &[("Cookie", reads.cookie.as_str())],
    )
    .await;
    assert_eq!(
        (refused.status, refused.api_error()),
        (429, "too_many_streams".into())
    );
    assert_eq!(reads.events.streams(), EVENTS.max_clients);

    drop(first);
    let until = Instant::now() + Duration::from_secs(5);
    while reads.events.streams() == EVENTS.max_clients {
        assert!(Instant::now() < until, "a closed stream releases its place");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let third = Stream::open(reads.admin, &reads.cookie).await;
    assert_eq!(third.status, 200);
}

#[tokio::test]
async fn signing_out_ends_the_stream() {
    let reads = Reads::new().await;
    let mut stream = Stream::open(reads.admin, &reads.cookie).await;
    stream.ready().await;
    let out = reads
        .call("POST", "/api/admin/auth/logout", json!({}), &[])
        .await;
    assert!((200..300).contains(&out.status), "{}", out.text());
    stream.ends_within(EVENTS.heartbeat * 4).await;
}

#[tokio::test]
async fn session_expiry_ends_the_stream() {
    let reads = Reads::new().await;
    let mut stream = Stream::open(reads.admin, &reads.cookie).await;
    stream.ready().await;
    assert_eq!(
        stream.next(EVENTS.heartbeat * 4).await.as_deref(),
        Some(": keep-alive"),
        "live while the session stands"
    );
    // Past the idle limit (60 min) with no request in between.
    *reads.session_skew.lock().unwrap() = TimeDelta::minutes(61);
    stream.ends_within(EVENTS.heartbeat * 4).await;
    assert_eq!(reads.events.streams(), 0);
}

#[derive(Debug)]
struct Up;

impl HealthProbe for Up {
    fn health(&self) -> HealthFuture<'_> {
        Box::pin(async { OfflineApplication.health() })
    }
}

#[tokio::test]
async fn shutdown_ends_open_streams_promptly() {
    let reads = Reads::new().await;
    let address = {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap()
    };
    let config = RuntimeConfig {
        admin_bind: address,
        public_bind: None,
        http: HttpConfig {
            admin_host: Some(ADMIN_HOST.into()),
            ..HttpConfig::default()
        },
        admin_auth: Default::default(),
        timezone: chrono_tz::Asia::Kuala_Lumpur,
        shutdown_timeout: Duration::from_secs(10),
    };
    let live = LiveAdmin {
        auth: reads.site.auth.clone().unwrap(),
        state: reads.site.state.clone().unwrap(),
        health: Arc::new(Up),
    };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        serve(&config, Some(live), async {
            let _ = stopped.await;
        })
        .await
    });
    let mut stream = loop {
        if TcpStream::connect(address).await.is_ok() {
            break Stream::open(address, &reads.cookie).await;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(stream.status, 200);
    stream.ready().await;

    let started = Instant::now();
    stop.send(()).unwrap();
    stream.ends_within(Duration::from_secs(2)).await;
    timeout(Duration::from_secs(2), server)
        .await
        .expect("the drain does not wait for the deadline")
        .unwrap()
        .expect("a clean shutdown");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn holding_streams_open_never_extends_the_idle_window() {
    let reads = Reads::new().await;
    // Signed in at the pinned instant; 59 minutes later only streams were opened.
    *reads.session_skew.lock().unwrap() = TimeDelta::minutes(59);
    let mut stream = Stream::open(reads.admin, &reads.cookie).await;
    assert_eq!(stream.status, 200);
    stream.ready().await;
    let again = Stream::open(reads.admin, &reads.cookie).await;
    assert_eq!(
        again.status, 200,
        "a reconnect is allowed while the session stands"
    );
    drop(again);
    // Past the idle limit counted from the last real request: the session is over.
    *reads.session_skew.lock().unwrap() = TimeDelta::minutes(61);
    stream.ends_within(EVENTS.heartbeat * 4).await;
    let reply = request(
        reads.admin,
        "GET",
        ADMIN_HOST,
        "/api/admin/session",
        &[("Cookie", reads.cookie.as_str())],
    )
    .await;
    assert_eq!(reply.status, 401, "the streams did not count as activity");
}

#[tokio::test]
async fn after_shutdown_begins_new_streams_are_unavailable() {
    let reads = Reads::new().await;
    reads.events.close();
    let reply = request(
        reads.admin,
        "GET",
        ADMIN_HOST,
        EVENTS_PATH,
        &[("Cookie", reads.cookie.as_str())],
    )
    .await;
    assert_eq!(
        (reply.status, reply.api_error()),
        (503, "unavailable".into())
    );
}

#[tokio::test]
async fn member_edits_hint_members_and_identical_roster_syncs_do_not() {
    let reads = Reads::new().await;
    let mut stream = Stream::open(reads.admin, &reads.cookie).await;
    stream.ready().await;
    reads
        .ok(
            "PATCH",
            "/api/admin/members/1001",
            json!({"ping_level": "all"}),
            "members.json#/$defs/MemberRow",
        )
        .await;
    assert_eq!(stream.hint().await["topic"], "members");
    // The gateway resends Bob exactly as stored: nothing changed, nothing hinted.
    let bob = reads.store.load_member("1002").await.unwrap().unwrap();
    reads
        .store
        .apply_gateway(kanade::domain::members::GatewayMember {
            user_id: bob.member.user_id.clone(),
            display_name: bob.member.display_name.clone(),
            nickname: bob.member.nickname.clone(),
            has_role: bob.member.has_role,
            is_bot: bob.member.is_bot,
            roles: bob.roles.clone(),
            is_guild_admin: bob.is_guild_admin,
        })
        .await
        .unwrap();
    assert!(stream.topics(Duration::from_millis(300)).await.is_empty());
}
