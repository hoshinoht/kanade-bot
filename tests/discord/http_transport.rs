//! The Twilight transport against an in-process loopback HTTP stub, through
//! twilight-http's own proxy setting (plain HTTP, never discord.com).

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use twilight_http::Client;
use twilight_model::id::Id;

use kanade::bot::mentions;
use kanade::bot::transport::{
    AmbiguousKind, DiscordTransport, InteractionRef, InteractionReply, MAX_SENDS, Outcome,
    OutgoingMessage, Presence, RejectionKind, TransportConfig, TwilightTransport,
};

use super::support::CHANNEL;

const TOKEN: &str = "synthetic-token-never-real";

/// What the stub does with the next request.
#[derive(Clone)]
enum Reply {
    Respond {
        status: u16,
        headers: Vec<(&'static str, &'static str)>,
        body: String,
    },
    /// Read the request, then drop the connection without answering.
    Drop,
    /// Read the request, then never answer.
    Stall,
}

fn json_reply(status: u16, body: Value) -> Reply {
    Reply::Respond {
        status,
        headers: Vec::new(),
        body: body.to_string(),
    }
}

fn raw_reply(status: u16, body: &str) -> Reply {
    Reply::Respond {
        status,
        headers: Vec::new(),
        body: body.to_owned(),
    }
}

#[derive(Clone, Debug)]
struct Seen {
    request_line: String,
    headers: String,
    body: Vec<u8>,
}

#[derive(Clone, Default)]
struct Stub {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Stub {
    async fn start(replies: Vec<Reply>) -> (Self, SocketAddr) {
        let stub = Self {
            replies: Arc::new(Mutex::new(replies.into())),
            seen: Arc::default(),
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = stub.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(server.clone().serve(stream));
            }
        });
        (stub, addr)
    }

    async fn serve(self, mut stream: TcpStream) {
        let mut buffer = Vec::new();
        loop {
            let Some(seen) = read_request(&mut stream, &mut buffer).await else {
                return;
            };
            self.seen.lock().unwrap().push(seen);
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Reply::Drop);
            match reply {
                Reply::Respond {
                    status,
                    headers,
                    body,
                } => {
                    let extra: String = headers
                        .iter()
                        .map(|(name, value)| format!("{name}: {value}\r\n"))
                        .collect();
                    let head = format!(
                        "HTTP/1.1 {status} Stub\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{extra}\r\n",
                        body.len()
                    );
                    if stream.write_all(head.as_bytes()).await.is_err()
                        || stream.write_all(body.as_bytes()).await.is_err()
                    {
                        return;
                    }
                }
                Reply::Drop => return,
                Reply::Stall => {
                    std::future::pending::<()>().await;
                }
            }
        }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

async fn read_request(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> Option<Seen> {
    let mut chunk = [0_u8; 4096];
    let head_end = loop {
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buffer.len() < head_end + length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = buffer[head_end..head_end + length].to_vec();
    buffer.drain(..head_end + length);
    let (request_line, headers) = head.split_once("\r\n").unwrap_or((&head, ""));
    Some(Seen {
        request_line: request_line.to_owned(),
        headers: headers.to_owned(),
        body,
    })
}

/// A client shaped like production (rate limiter on) but aimed at the stub.
fn transport_with(addr: SocketAddr, config: TransportConfig) -> TwilightTransport {
    kanade::runtime::tls::install_ring_provider().ok();
    let client = Client::builder()
        .token(TOKEN.to_owned())
        .proxy(addr.to_string(), true)
        .timeout(config.attempt_timeout)
        .build();
    TwilightTransport::from_client(client, Id::new(9), config)
}

fn transport(addr: SocketAddr, attempt: Duration, deadline: Duration) -> TwilightTransport {
    transport_with(
        addr,
        TransportConfig {
            attempt_timeout: attempt,
            deadline,
            respond_deadline: deadline,
        },
    )
}

fn quick(addr: SocketAddr) -> TwilightTransport {
    transport(addr, Duration::from_secs(5), Duration::from_secs(10))
}

fn post() -> OutgoingMessage {
    OutgoingMessage {
        content: Some("@everyone <@&500> <@1002> run at 8".into()),
        embeds: Vec::new(),
        allowed_mentions: mentions::allow_users(&["1001"]),
    }
}

async fn create(
    replies: Vec<Reply>,
) -> (
    Outcome<twilight_model::id::Id<twilight_model::id::marker::MessageMarker>>,
    Stub,
) {
    let (stub, addr) = Stub::start(replies).await;
    let outcome = quick(addr).create_message(Id::new(CHANNEL), &post()).await;
    (outcome, stub)
}

#[tokio::test]
async fn create_delivers_with_explicit_allow_list_on_the_wire() {
    let (outcome, stub) = create(vec![json_reply(
        200,
        json!({ "id": "123456789", "extra": true }),
    )])
    .await;
    assert_eq!(outcome, Outcome::Delivered(Id::new(123_456_789)));
    let seen = stub.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].request_line,
        format!("POST /api/v10/channels/{CHANNEL}/messages HTTP/1.1")
    );
    assert!(
        seen[0]
            .headers
            .to_ascii_lowercase()
            .contains("content-type: application/json")
    );
    let body: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(
        body["allowed_mentions"],
        json!({ "parse": [], "users": ["1001"] }),
        "content mentions cannot widen the list"
    );
}

#[tokio::test]
async fn missing_permissions_is_definite() {
    let (outcome, stub) = create(vec![json_reply(
        403,
        json!({ "code": 50013, "message": "Missing Permissions" }),
    )])
    .await;
    assert_eq!(
        outcome,
        Outcome::DefinitelyRejected(RejectionKind::MissingPermissions)
    );
    assert_eq!(stub.seen().len(), 1, "never retried");
}

#[tokio::test]
async fn unknown_channel_is_definite() {
    let (outcome, _) = create(vec![json_reply(
        404,
        json!({ "code": 10003, "message": "Unknown Channel" }),
    )])
    .await;
    assert_eq!(
        outcome,
        Outcome::DefinitelyRejected(RejectionKind::UnknownChannel)
    );
}

#[tokio::test]
async fn other_client_errors_are_definite() {
    let (outcome, _) = create(vec![json_reply(
        400,
        json!({ "code": 50035, "message": "Invalid Form Body" }),
    )])
    .await;
    assert_eq!(
        outcome,
        Outcome::DefinitelyRejected(RejectionKind::Http {
            status: 400,
            code: Some(50035)
        })
    );
}

#[tokio::test]
async fn server_errors_are_ambiguous_and_not_retried() {
    let (outcome, stub) = create(vec![json_reply(
        500,
        json!({ "code": 0, "message": "oops" }),
    )])
    .await;
    assert_eq!(
        outcome,
        Outcome::Ambiguous(AmbiguousKind::ServerError { status: 500 })
    );
    assert_eq!(stub.seen().len(), 1);
}

#[tokio::test]
async fn non_json_error_body_is_ambiguous() {
    let (outcome, _) = create(vec![raw_reply(502, "<html>bad gateway</html>")]).await;
    assert_eq!(
        outcome,
        Outcome::Ambiguous(AmbiguousKind::UnreadableResponse)
    );
}

#[tokio::test]
async fn dropped_connection_after_write_is_ambiguous_and_not_retried() {
    let (outcome, stub) = create(vec![Reply::Drop]).await;
    assert_eq!(outcome, Outcome::Ambiguous(AmbiguousKind::Connection));
    assert_eq!(stub.seen().len(), 1, "the written request is not replayed");
}

#[tokio::test]
async fn delivered_but_unreadable_id_is_ambiguous() {
    let (outcome, _) = create(vec![raw_reply(200, "{\"no_id\":1}")]).await;
    assert_eq!(
        outcome,
        Outcome::Ambiguous(AmbiguousKind::UnreadableResponse)
    );
}

#[tokio::test]
async fn attempt_timeout_is_ambiguous() {
    let (stub, addr) = Stub::start(vec![Reply::Stall]).await;
    let outcome = transport(addr, Duration::from_millis(200), Duration::from_secs(10))
        .create_message(Id::new(CHANNEL), &post())
        .await;
    assert_eq!(outcome, Outcome::Ambiguous(AmbiguousKind::Timeout));
    assert_eq!(stub.seen().len(), 1);
}

#[tokio::test]
async fn overall_deadline_is_ambiguous() {
    let (_, addr) = Stub::start(vec![Reply::Stall]).await;
    let outcome = transport(addr, Duration::from_secs(10), Duration::from_millis(200))
        .create_message(Id::new(CHANNEL), &post())
        .await;
    assert_eq!(outcome, Outcome::Ambiguous(AmbiguousKind::Timeout));
}

fn rate_limited() -> Reply {
    json_reply(
        429,
        json!({ "global": false, "message": "slow", "retry_after": 0.0 }),
    )
}

/// Twilight re-sends after 429 (Discord did not process it) once the rate
/// limiter grants a permit; this pins that behaviour across upgrades.
#[tokio::test]
async fn rate_limited_request_is_resent_through_the_limiter() {
    let (outcome, stub) =
        create(vec![rate_limited(), json_reply(200, json!({ "id": "77" }))]).await;
    assert_eq!(outcome, Outcome::Delivered(Id::new(77)));
    assert_eq!(stub.seen().len(), 2);
}

#[tokio::test]
async fn persistent_429_stops_at_the_send_cap() {
    let (outcome, stub) = create(vec![rate_limited(); 10]).await;
    assert_eq!(
        outcome,
        Outcome::DefinitelyRejected(RejectionKind::RateLimited)
    );
    assert_eq!(stub.seen().len(), MAX_SENDS as usize);
}

#[tokio::test]
async fn deadline_while_waiting_for_a_permit_is_not_sent() {
    let exhausted = Reply::Respond {
        status: 200,
        headers: vec![
            ("x-ratelimit-scope", "user"),
            ("x-ratelimit-bucket", "abc"),
            ("x-ratelimit-limit", "1"),
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset-after", "5"),
        ],
        body: json!({ "id": "1" }).to_string(),
    };
    let (stub, addr) = Stub::start(vec![exhausted, json_reply(200, json!({ "id": "2" }))]).await;
    let transport = transport(addr, Duration::from_secs(5), Duration::from_millis(300));
    assert_eq!(
        transport.create_message(Id::new(CHANNEL), &post()).await,
        Outcome::Delivered(Id::new(1))
    );
    assert_eq!(
        transport.create_message(Id::new(CHANNEL), &post()).await,
        Outcome::DefinitelyRejected(RejectionKind::NotSent)
    );
    assert_eq!(stub.seen().len(), 1, "the second request never left");
}

#[tokio::test]
async fn refused_connection_is_not_sent() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let outcome = quick(addr).create_message(Id::new(CHANNEL), &post()).await;
    assert_eq!(outcome, Outcome::DefinitelyRejected(RejectionKind::NotSent));
}

#[tokio::test]
async fn interaction_responses_use_their_own_short_deadline() {
    let (_, addr) = Stub::start(vec![Reply::Stall]).await;
    let transport = transport_with(
        addr,
        TransportConfig {
            attempt_timeout: Duration::from_secs(10),
            deadline: Duration::from_secs(30),
            respond_deadline: Duration::from_millis(200),
        },
    );
    let interaction = InteractionRef::new(Id::new(7700), "interaction-secret-token".into());
    let started = std::time::Instant::now();
    assert_eq!(
        transport
            .respond(&interaction, &InteractionReply::ephemeral("hi"))
            .await,
        Outcome::Ambiguous(AmbiguousKind::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(
        TransportConfig::default().respond_deadline,
        Duration::from_millis(2_500)
    );
}

#[tokio::test]
async fn deferred_response_then_completion() {
    let (stub, addr) = Stub::start(vec![
        raw_reply(204, ""),
        json_reply(200, json!({ "id": "8" })),
    ])
    .await;
    let transport = quick(addr);
    let interaction = InteractionRef::new(Id::new(7700), "interaction-secret-token".into());
    assert_eq!(
        transport.defer(&interaction, true).await,
        Outcome::Delivered(())
    );
    assert_eq!(
        transport
            .complete_deferred(&interaction, &InteractionReply::ephemeral("done <@1001>"))
            .await,
        Outcome::Delivered(())
    );
    let seen = stub.seen();
    let defer: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(defer["type"], json!(5));
    assert_eq!(defer["data"]["flags"], json!(64));
    assert!(
        seen[1]
            .request_line
            .starts_with("PATCH /api/v10/webhooks/9/")
    );
    assert!(seen[1].request_line.contains("/messages/@original"));
    let completion: Value = serde_json::from_slice(&seen[1].body).unwrap();
    assert_eq!(completion["content"], json!("done <@1001>"));
    assert_eq!(completion["allowed_mentions"], json!({ "parse": [] }));
}

#[tokio::test]
async fn presence_and_delete_classify_unknown_message() {
    let (_, addr) = Stub::start(vec![
        json_reply(404, json!({ "code": 10008, "message": "Unknown Message" })),
        json_reply(404, json!({ "code": 10008, "message": "Unknown Message" })),
        raw_reply(204, ""),
    ])
    .await;
    let transport = quick(addr);
    let (channel, message) = (Id::new(CHANNEL), Id::new(55));
    assert_eq!(
        transport.message_presence(channel, message).await,
        Outcome::Delivered(Presence::Absent)
    );
    assert_eq!(
        transport.delete_message(channel, message).await,
        Outcome::DefinitelyRejected(RejectionKind::UnknownMessage)
    );
    assert_eq!(
        transport.add_own_reaction(channel, message, "✅").await,
        Outcome::Delivered(())
    );
}

#[tokio::test]
async fn interaction_reply_is_ephemeral_and_mention_free() {
    let (stub, addr) = Stub::start(vec![raw_reply(204, "")]).await;
    let interaction = InteractionRef::new(Id::new(7700), "interaction-secret-token".into());
    let outcome = quick(addr)
        .respond(&interaction, &InteractionReply::ephemeral("❌ no <@1001>"))
        .await;
    assert_eq!(outcome, Outcome::Delivered(()));
    let seen = stub.seen();
    let body: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(body["type"], json!(4));
    assert_eq!(body["data"]["flags"], json!(64));
    assert_eq!(body["data"]["allowed_mentions"], json!({ "parse": [] }));
    assert!(!format!("{interaction:?}").contains("secret"));
}

#[tokio::test]
async fn debug_output_never_contains_the_token() {
    let (_, addr) = Stub::start(Vec::new()).await;
    let transport = quick(addr);
    assert!(!format!("{transport:?}").contains(TOKEN));
    let production =
        TwilightTransport::new(TOKEN.to_owned(), Id::new(9), TransportConfig::default());
    assert!(!format!("{production:?}").contains(TOKEN));
}
