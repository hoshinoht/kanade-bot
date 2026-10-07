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
    AmbiguousKind, DiscordTransport, HistoryPage, InteractionRef, InteractionReply, MAX_SENDS,
    Outcome, OutgoingMessage, Presence, RejectionKind, SILENT, TransportConfig, TwilightTransport,
    Upload,
};
use twilight_model::channel::message::MessageFlags;

use super::support::{
    ALICE, BOB, CHANNEL, GUILD, TEXT, channel_json, member_json, message_json, user_json,
};

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
        reply_to: None,
        attachments: Vec::new(),
        components: Vec::new(),
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
async fn attachments_go_multipart_with_the_payload_and_files() {
    let (stub, addr) = Stub::start(vec![json_reply(200, json!({ "id": "7" }))]).await;
    let message = OutgoingMessage {
        attachments: vec![Upload {
            filename: "MaleficStar.png".into(),
            bytes: Arc::from(&b"star-bytes"[..]),
        }],
        ..post()
    };
    let outcome = quick(addr).create_message(Id::new(CHANNEL), &message).await;
    assert_eq!(outcome, Outcome::Delivered(Id::new(7)));
    let seen = stub.seen();
    assert!(
        seen[0]
            .headers
            .to_ascii_lowercase()
            .contains("content-type: multipart/form-data")
    );
    let body = String::from_utf8_lossy(&seen[0].body);
    assert!(body.contains("filename=\"MaleficStar.png\""), "{body}");
    assert!(body.contains("star-bytes"));
    assert!(
        body.contains(r#""allowed_mentions":{"parse":[],"users":["1001"]}"#),
        "the allow-list travels in payload_json: {body}"
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
async fn autocomplete_and_embed_replies_serialise_as_discord_expects() {
    let (stub, addr) = Stub::start(vec![raw_reply(204, ""), raw_reply(204, "")]).await;
    let transport = quick(addr);
    let interaction = InteractionRef::new(Id::new(7700), "interaction-secret-token".into());
    let choices = [kanade::bot::commands::choice(
        "XKalos · Tue 22:00",
        "run-id",
    )];
    assert_eq!(
        transport.autocomplete(&interaction, &choices).await,
        Outcome::Delivered(())
    );
    let embed: twilight_model::channel::message::Embed = serde_json::from_value(json!({
        "type": "rich", "title": "Boss week of Thu 24 Sep (all)"
    }))
    .unwrap();
    let reply = InteractionReply::public("").with_embed(embed);
    assert_eq!(
        transport.respond(&interaction, &reply).await,
        Outcome::Delivered(())
    );
    let seen = stub.seen();
    let suggested: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(suggested["type"], json!(8));
    assert_eq!(
        suggested["data"]["choices"],
        json!([{ "name": "XKalos · Tue 22:00", "value": "run-id" }])
    );
    let public: Value = serde_json::from_slice(&seen[1].body).unwrap();
    assert_eq!(public["type"], json!(4));
    assert!(public["data"].get("flags").is_none());
    assert!(public["data"].get("content").is_none());
    assert_eq!(
        public["data"]["embeds"][0]["title"],
        json!("Boss week of Thu 24 Sep (all)")
    );
    assert_eq!(public["data"]["allowed_mentions"], json!({ "parse": [] }));
}

#[tokio::test]
async fn follow_ups_post_to_the_interaction_webhook() {
    let (stub, addr) = Stub::start(vec![json_reply(200, json!({ "id": "8" }))]).await;
    let interaction = InteractionRef::new(Id::new(7700), "interaction-secret-token".into());
    assert_eq!(
        quick(addr)
            .followup(&interaction, &InteractionReply::ephemeral("more <@1001>"))
            .await,
        Outcome::Delivered(())
    );
    let seen = stub.seen();
    assert!(
        seen[0]
            .request_line
            .starts_with("POST /api/v10/webhooks/9/interaction-secret-token"),
        "{}",
        seen[0].request_line
    );
    let body: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(body["content"], json!("more <@1001>"));
    assert_eq!(body["flags"], json!(64));
    assert_eq!(body["allowed_mentions"], json!({ "parse": [] }));
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

#[tokio::test]
async fn a_reply_references_its_target_without_failing_or_pinging() {
    let (stub, addr) = Stub::start(vec![json_reply(200, json!({ "id": "90" }))]).await;
    let mut message = post();
    message.reply_to = Some(Id::new(55));
    assert_eq!(
        quick(addr).create_message(Id::new(CHANNEL), &message).await,
        Outcome::Delivered(Id::new(90))
    );
    let body: Value = serde_json::from_slice(&stub.seen()[0].body).unwrap();
    assert_eq!(body["message_reference"]["message_id"], json!("55"));
    assert_eq!(
        body["message_reference"]["fail_if_not_exists"],
        json!(false)
    );
    assert_eq!(
        body["allowed_mentions"],
        json!({ "parse": [], "users": ["1001"] }),
        "the replied author is not pinged unless listed"
    );
}

#[tokio::test]
async fn a_plain_post_has_no_message_reference() {
    let (outcome, stub) = create(vec![json_reply(200, json!({ "id": "91" }))]).await;
    assert!(outcome.is_delivered());
    let body: Value = serde_json::from_slice(&stub.seen()[0].body).unwrap();
    assert!(body.get("message_reference").is_none());
}

#[tokio::test]
async fn a_silent_post_carries_suppress_notifications_and_a_plain_one_no_flags() {
    let (stub, addr) = Stub::start(vec![
        json_reply(200, json!({ "id": "92" })),
        json_reply(200, json!({ "id": "93" })),
    ])
    .await;
    let transport = quick(addr);
    assert_eq!(
        transport
            .create_flagged_message(Id::new(CHANNEL), &post(), SILENT)
            .await,
        Outcome::Delivered(Id::new(92))
    );
    assert!(
        transport
            .create_message(Id::new(CHANNEL), &post())
            .await
            .is_delivered()
    );
    let seen = stub.seen();
    let silent: Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(silent["flags"], json!(1 << 12));
    assert_eq!(
        silent["allowed_mentions"],
        json!({ "parse": [], "users": ["1001"] })
    );
    let plain: Value = serde_json::from_slice(&seen[1].body).unwrap();
    assert!(plain.get("flags").is_none(), "{plain}");
}

#[tokio::test]
async fn flags_discord_refuses_on_creates_are_refused_unsent() {
    let (stub, addr) = Stub::start(Vec::new()).await;
    assert_eq!(
        quick(addr)
            .create_flagged_message(Id::new(CHANNEL), &post(), SILENT | MessageFlags::EPHEMERAL)
            .await,
        Outcome::DefinitelyRejected(RejectionKind::Invalid)
    );
    assert!(stub.seen().is_empty());
}

#[tokio::test]
async fn typing_posts_to_the_channel_typing_route_and_is_classified() {
    let (stub, addr) = Stub::start(vec![
        raw_reply(204, ""),
        json_reply(
            403,
            json!({ "code": 50013, "message": "Missing Permissions" }),
        ),
        json_reply(500, json!({ "code": 0, "message": "oops" })),
    ])
    .await;
    let transport = quick(addr);
    let channel = Id::new(CHANNEL);
    assert_eq!(
        transport.trigger_typing(channel).await,
        Outcome::Delivered(())
    );
    assert_eq!(
        transport.trigger_typing(channel).await,
        Outcome::DefinitelyRejected(RejectionKind::MissingPermissions)
    );
    assert_eq!(
        transport.trigger_typing(channel).await,
        Outcome::Ambiguous(AmbiguousKind::ServerError { status: 500 })
    );
    let seen = stub.seen();
    assert_eq!(seen.len(), 3, "never retried");
    assert_eq!(
        seen[0].request_line,
        format!("POST /api/v10/channels/{CHANNEL}/typing HTTP/1.1")
    );
}

#[tokio::test]
async fn commands_are_registered_on_the_guild_endpoint_only() {
    let (stub, addr) = Stub::start(vec![json_reply(200, json!([]))]).await;
    assert_eq!(
        quick(addr)
            .register_guild_commands(Id::new(GUILD), &[])
            .await,
        Outcome::Delivered(())
    );
    let seen = stub.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].request_line,
        format!("PUT /api/v10/applications/9/guilds/{GUILD}/commands HTTP/1.1")
    );
}

/// Global commands would appear in every guild the production token is in.
#[test]
fn no_source_touches_global_commands() {
    fn scan(dir: &std::path::Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                scan(&path, hits);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                if text.contains("global_command") {
                    hits.push(path.display().to_string());
                }
            }
        }
    }
    let mut hits = Vec::new();
    scan(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut hits,
    );
    assert!(hits.is_empty(), "global command use in {hits:?}");
}

#[tokio::test]
async fn members_are_paged_after_a_user_id() {
    let members = json!([
        member_json(user_json(ALICE, "alice", None, false), None, &[]),
        member_json(user_json(BOB, "bob", None, false), Some("bobby"), &[]),
    ]);
    let (stub, addr) = Stub::start(vec![json_reply(200, members)]).await;
    let Outcome::Delivered(page) = quick(addr)
        .list_members(Id::new(GUILD), Some(Id::new(1000)), 2)
        .await
    else {
        panic!("page delivered");
    };
    assert_eq!(
        page.iter().map(|m| m.user.id.get()).collect::<Vec<_>>(),
        vec![ALICE, BOB]
    );
    let line = &stub.seen()[0].request_line;
    assert!(
        line.starts_with(&format!("GET /api/v10/guilds/{GUILD}/members?")),
        "{line}"
    );
    assert!(
        line.contains("after=1000") && line.contains("limit=2"),
        "{line}"
    );
}

#[tokio::test]
async fn reactors_use_encoded_emoji_type_limit_and_after_and_classify_deleted_cards() {
    use twilight_model::channel::message::ReactionType;
    let (stub, addr) = Stub::start(vec![
        json_reply(200, json!([user_json(ALICE, "alice", None, false)])),
        json_reply(404, json!({"code": 10008, "message": "Unknown Message"})),
    ])
    .await;
    let transport = quick(addr);
    assert_eq!(
        transport
            .reaction_users(
                Id::new(CHANNEL),
                Id::new(123),
                "❌",
                ReactionType::Burst,
                Some(Id::new(1000)),
                100
            )
            .await,
        Outcome::Delivered(vec![Id::new(ALICE)])
    );
    let line = &stub.seen()[0].request_line;
    assert!(
        line.starts_with(&format!(
            "GET /api/v10/channels/{CHANNEL}/messages/123/reactions/%E2%9D%8C?"
        )),
        "{line}"
    );
    for query in ["after=1000", "limit=100", "type=1"] {
        assert!(line.contains(query), "{line}");
    }
    assert_eq!(
        transport
            .reaction_users(
                Id::new(CHANNEL),
                Id::new(123),
                "✅",
                ReactionType::Normal,
                None,
                100
            )
            .await,
        Outcome::DefinitelyRejected(RejectionKind::UnknownMessage)
    );
    assert_eq!(
        transport
            .reaction_users(
                Id::new(CHANNEL),
                Id::new(123),
                "✅",
                ReactionType::Normal,
                None,
                101
            )
            .await,
        Outcome::DefinitelyRejected(RejectionKind::Invalid)
    );
    assert_eq!(stub.seen().len(), 2, "invalid limit is never sent");
}

#[tokio::test]
async fn history_pages_carry_their_cursor() {
    let page = json!([
        message_json(12, CHANNEL, Some(GUILD), "newer"),
        message_json(11, CHANNEL, Some(GUILD), "older"),
    ]);
    let (stub, addr) = Stub::start(vec![
        json_reply(200, page.clone()),
        json_reply(200, page.clone()),
        json_reply(200, page),
    ])
    .await;
    let transport = quick(addr);
    for (cursor, query) in [
        (HistoryPage::Latest, "limit=50".to_owned()),
        (HistoryPage::Before(Id::new(13)), "before=13".to_owned()),
        (HistoryPage::After(Id::new(10)), "after=10".to_owned()),
    ] {
        let Outcome::Delivered(messages) = transport
            .channel_messages(Id::new(CHANNEL), cursor, 50)
            .await
        else {
            panic!("page delivered");
        };
        assert_eq!(messages[0].content, "newer");
        let line = stub.seen().last().unwrap().request_line.clone();
        assert!(
            line.starts_with(&format!("GET /api/v10/channels/{CHANNEL}/messages?")),
            "{line}"
        );
        assert!(line.contains(&query) && line.contains("limit=50"), "{line}");
    }
}

#[tokio::test]
async fn guild_channels_are_read() {
    let (stub, addr) = Stub::start(vec![json_reply(
        200,
        json!([channel_json(CHANNEL, TEXT, "kalos", None, &[])]),
    )])
    .await;
    let Outcome::Delivered(channels) = quick(addr).guild_channels(Id::new(GUILD)).await else {
        panic!("channels delivered");
    };
    assert_eq!(channels[0].name.as_deref(), Some("kalos"));
    assert_eq!(
        stub.seen()[0].request_line,
        format!("GET /api/v10/guilds/{GUILD}/channels HTTP/1.1")
    );
}

#[tokio::test]
async fn reads_are_classified_like_every_other_call() {
    let (stub, addr) = Stub::start(vec![
        json_reply(503, json!({ "code": 0, "message": "down" })),
        json_reply(403, json!({ "code": 50001, "message": "Missing Access" })),
        raw_reply(200, "{\"not\":\"a list\"}"),
    ])
    .await;
    let transport = quick(addr);
    let channel = Id::new(CHANNEL);
    assert_eq!(
        transport
            .channel_messages(channel, HistoryPage::Latest, 10)
            .await,
        Outcome::Ambiguous(AmbiguousKind::ServerError { status: 503 })
    );
    assert_eq!(
        transport.list_members(Id::new(GUILD), None, 10).await,
        Outcome::DefinitelyRejected(RejectionKind::MissingAccess)
    );
    assert_eq!(
        transport.guild_channels(Id::new(GUILD)).await,
        Outcome::Ambiguous(AmbiguousKind::UnreadableResponse)
    );
    assert_eq!(stub.seen().len(), 3, "nothing retried");
}

#[tokio::test]
async fn persistently_rate_limited_reads_stop_at_the_send_cap() {
    let (stub, addr) = Stub::start(vec![rate_limited(); 10]).await;
    assert_eq!(
        quick(addr)
            .channel_messages(Id::new(CHANNEL), HistoryPage::Latest, 10)
            .await,
        Outcome::DefinitelyRejected(RejectionKind::RateLimited)
    );
    assert_eq!(stub.seen().len(), MAX_SENDS as usize);
}

#[tokio::test]
async fn out_of_range_page_sizes_are_refused_unsent() {
    let (stub, addr) = Stub::start(Vec::new()).await;
    let transport = quick(addr);
    let invalid = Outcome::DefinitelyRejected(RejectionKind::Invalid);
    assert_eq!(
        transport
            .channel_messages(Id::new(CHANNEL), HistoryPage::Latest, 101)
            .await,
        invalid
    );
    assert_eq!(
        transport
            .channel_messages(Id::new(CHANNEL), HistoryPage::Before(Id::new(5)), 0)
            .await,
        invalid
    );
    assert_eq!(
        transport.list_members(Id::new(GUILD), None, 1001).await,
        Outcome::DefinitelyRejected(RejectionKind::Invalid)
    );
    assert!(stub.seen().is_empty());
}
