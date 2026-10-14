//! `LateTransport` delegates every `DiscordTransport` method once ready,
//! including those with a default body (which would otherwise answer
//! `Invalid` without sending anything).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use twilight_http::Client;
use twilight_model::id::Id;

use super::*;
use crate::bot::mentions;

/// Trait methods of `DiscordTransport` that have a default body, read from
/// the trait's source: a signature line ending `+ Send {`.
fn default_bodied() -> Vec<String> {
    let source = include_str!("../../../bot/transport/mod.rs");
    let start = source
        .find("pub trait DiscordTransport")
        .expect("the trait");
    let mut names = Vec::new();
    let mut current: Option<String> = None;
    for line in source[start..].lines() {
        if let Some(rest) = line.trim_start().strip_prefix("fn ") {
            current = rest.split('(').next().map(str::to_owned);
        }
        if line.trim_end().ends_with("+ Send {")
            && let Some(name) = current.take()
        {
            names.push(name);
        }
        if line == "}" {
            break;
        }
    }
    names
}

/// A compile-time-sourced listing check: a new default-bodied method fails
/// here until `LateTransport` delegates it (and the behavioural test below
/// calls it).
#[test]
fn every_default_bodied_method_is_delegated() {
    let names = default_bodied();
    assert_eq!(
        names,
        [
            "create_flagged_message",
            "trigger_typing",
            "reaction_users",
            "message_flags",
            "defer_update",
            "followup",
            "autocomplete",
            "current_user",
            "application_emojis",
            "create_application_emoji"
        ],
        "update `defaults_reach_discord` for new default-bodied methods"
    );
    let late = include_str!("late.rs")
        .split_whitespace()
        .collect::<String>();
    for name in &names {
        // `application_emojis` has its own pre-READY path but still hands
        // over to the ready transport.
        assert!(
            late.contains(&format!("delegate!(self,{name}("))
                || late.contains(&format!("inner.{name}()")),
            "LateTransport does not delegate `{name}`"
        );
    }
}

/// A loopback Discord answering every request `200 {}` and counting them.
async fn stub() -> (Arc<AtomicUsize>, std::net::SocketAddr) {
    let seen = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    let Some(end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") else {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                        }
                        continue;
                    };
                    let head = String::from_utf8_lossy(&buffer[..end]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    while buffer.len() < end + 4 + length {
                        match stream.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                        }
                    }
                    buffer.drain(..end + 4 + length);
                    counter.fetch_add(1, Ordering::SeqCst);
                    let reply = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}";
                    if stream.write_all(reply.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (seen, addr)
}

#[tokio::test]
async fn defaults_reach_discord_once_ready_and_are_refused_unsent_before() {
    crate::runtime::tls::install_ring_provider().ok();
    let (seen, addr) = stub().await;
    let config = TransportConfig {
        attempt_timeout: Duration::from_secs(5),
        deadline: Duration::from_secs(10),
        respond_deadline: Duration::from_secs(10),
    };
    let client = Client::builder()
        .token("synthetic-token-never-real".to_owned())
        .proxy(addr.to_string(), true)
        .timeout(config.attempt_timeout)
        .build();
    let path = std::env::temp_dir().join(format!("kanade-late-{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, "synthetic-token-never-real").unwrap();
    let token = Redacted::read(&path, "KANADE_TEST_TOKEN_FILE").unwrap();
    std::fs::remove_file(&path).ok();
    let late = LateTransport::new(token);
    let channel: ChannelId = Id::new(50);
    let message = OutgoingMessage {
        content: Some("x".into()),
        embeds: Vec::new(),
        allowed_mentions: mentions::none(),
        reply_to: None,
        attachments: Vec::new(),
        components: Vec::new(),
    };
    let interaction = InteractionRef::new(Id::new(7), "synthetic-interaction".into());
    let reply = InteractionReply::public("part two");

    assert_eq!(
        late.followup(&interaction, &reply).await,
        Outcome::DefinitelyRejected(RejectionKind::NotSent),
        "not ready: refused unsent"
    );
    assert!(
        late.inner
            .set(TwilightTransport::from_client(client, Id::new(9), config))
            .is_ok()
    );

    // Each must be sent (counted by the stub), never refused locally.
    let not_invalid = |label: Option<String>| assert_ne!(label.as_deref(), Some("invalid"));
    not_invalid(
        late.create_flagged_message(channel, &message, crate::bot::transport::SILENT)
            .await
            .failure_label(),
    );
    not_invalid(late.trigger_typing(channel).await.failure_label());
    not_invalid(
        late.reaction_users(
            channel,
            Id::new(123),
            "✅",
            twilight_model::channel::message::ReactionType::Normal,
            None,
            100,
        )
        .await
        .failure_label(),
    );
    not_invalid(
        late.message_flags(channel, Id::new(123))
            .await
            .failure_label(),
    );
    not_invalid(late.defer_update(&interaction).await.failure_label());
    not_invalid(late.followup(&interaction, &reply).await.failure_label());
    not_invalid(late.autocomplete(&interaction, &[]).await.failure_label());
    not_invalid(late.current_user().await.failure_label());
    not_invalid(late.application_emojis().await.failure_label());
    not_invalid(
        late.create_application_emoji("diff_n", b"\x89PNG\r\n\x1a\n")
            .await
            .failure_label(),
    );
    assert_eq!(
        seen.load(Ordering::SeqCst),
        10,
        "every default-bodied call was sent"
    );
}
