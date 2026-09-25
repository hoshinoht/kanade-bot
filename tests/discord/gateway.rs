//! The event loop over a scripted event source: guild filtering, error
//! tolerance, fatal close and graceful shutdown.

use std::collections::VecDeque;
use std::time::Duration;

use tokio::sync::oneshot;
use twilight_gateway::{Event, EventTypeFlags, Intents};

use kanade::bot::events::{BotEvent, EventHandler};
use kanade::bot::gateway::{
    CloseReason, EventSource, GatewayError, INTENTS, RunExit, RunnerConfig, WANTED_EVENTS, run,
};
use twilight_gateway::CloseFrame;

use super::support::*;

/// Scripted events; after the script, the source idles (or ends if `ends`).
struct FakeSource {
    items: VecDeque<Result<Event, GatewayError>>,
    ends: bool,
    /// Events delivered after `close`, before the close frame (if any).
    after_close: Option<Vec<Event>>,
    closed: bool,
}

impl FakeSource {
    fn new(items: Vec<Result<Event, GatewayError>>) -> Self {
        Self {
            items: items.into(),
            ends: false,
            after_close: None,
            closed: false,
        }
    }
}

impl EventSource for FakeSource {
    async fn next_event(&mut self) -> Option<Result<Event, GatewayError>> {
        if let Some(item) = self.items.pop_front() {
            return Some(item);
        }
        if self.ends {
            return None;
        }
        std::future::pending().await
    }

    fn close(&mut self) {
        self.closed = true;
        if let Some(events) = self.after_close.take() {
            self.items.extend(events.into_iter().map(Ok));
        }
    }
}

#[derive(Default)]
struct Recorder {
    events: Vec<BotEvent>,
    stop_after: Option<(usize, oneshot::Sender<()>)>,
}

impl EventHandler for Recorder {
    async fn handle(&mut self, event: BotEvent) {
        self.events.push(event);
        if self
            .stop_after
            .as_ref()
            .is_some_and(|(count, _)| *count == self.events.len())
            && let Some((_, stop)) = self.stop_after.take()
        {
            let _ = stop.send(());
        }
    }
}

fn config() -> RunnerConfig {
    RunnerConfig {
        scope: scope(),
        drain_timeout: Duration::from_secs(5),
    }
}

fn alice() -> serde_json::Value {
    user_json(ALICE, "alice", None, false)
}

#[test]
fn intents_are_exactly_v4s_used_set() {
    assert_eq!(
        INTENTS,
        Intents::GUILDS
            | Intents::GUILD_MEMBERS
            | Intents::GUILD_MESSAGES
            | Intents::MESSAGE_CONTENT
            | Intents::GUILD_MESSAGE_REACTIONS
    );
    for absent in [
        Intents::DIRECT_MESSAGES,
        Intents::GUILD_PRESENCES,
        Intents::GUILD_MESSAGE_TYPING,
        Intents::GUILD_VOICE_STATES,
    ] {
        assert!(!INTENTS.contains(absent));
    }
    assert!(
        WANTED_EVENTS.contains(EventTypeFlags::REACTION_ADD | EventTypeFlags::INTERACTION_CREATE)
    );
    assert!(WANTED_EVENTS.contains(
        EventTypeFlags::GUILD_UPDATE
            | EventTypeFlags::ROLE_CREATE
            | EventTypeFlags::ROLE_UPDATE
            | EventTypeFlags::ROLE_DELETE
    ));
    assert!(WANTED_EVENTS.contains(
        EventTypeFlags::MESSAGE_CREATE
            | EventTypeFlags::MESSAGE_UPDATE
            | EventTypeFlags::MESSAGE_DELETE
            | EventTypeFlags::MESSAGE_DELETE_BULK
            | EventTypeFlags::CHANNEL_CREATE
            | EventTypeFlags::CHANNEL_UPDATE
            | EventTypeFlags::CHANNEL_DELETE
            | EventTypeFlags::THREAD_CREATE
            | EventTypeFlags::THREAD_UPDATE
            | EventTypeFlags::THREAD_DELETE
            | EventTypeFlags::THREAD_LIST_SYNC
    ));
    assert!(!WANTED_EVENTS.contains(EventTypeFlags::PRESENCE_UPDATE));
    assert!(!WANTED_EVENTS.contains(EventTypeFlags::TYPING_START));
}

#[tokio::test]
async fn routes_guild_events_and_survives_receive_errors() {
    let mut source = FakeSource::new(vec![
        Ok(ready(SELF_ID)),
        Err(GatewayError::Reconnect),
        Ok(member_remove(OTHER_GUILD, alice())),
        Err(GatewayError::Undecodable),
        Ok(member_remove(GUILD, alice())),
        Ok(Event::GatewayClose(None)),
    ]);
    source.ends = true;
    let mut handler = Recorder::default();
    let mut errors = Vec::new();
    let exit = run(
        &mut source,
        &mut handler,
        config(),
        std::future::pending(),
        |error| errors.push(error),
    )
    .await;

    assert_eq!(
        exit,
        RunExit::Closed {
            reason: CloseReason::Other(None)
        },
        "the stream ended without a shutdown"
    );
    assert_eq!(
        errors,
        vec![GatewayError::Reconnect, GatewayError::Undecodable]
    );
    assert_eq!(handler.events.len(), 2);
    assert_eq!(
        handler.events[0],
        BotEvent::Ready {
            self_id: user(SELF_ID)
        }
    );
    assert!(matches!(handler.events[1], BotEvent::Roster(_)));
    assert!(!source.closed);
}

#[tokio::test]
async fn shutdown_closes_and_drains_without_dispatching() {
    let (stop, stopped) = oneshot::channel();
    let mut source = FakeSource::new(vec![Ok(member_remove(GUILD, alice()))]);
    source.after_close = Some(vec![
        member_remove(GUILD, alice()),
        member_remove(OTHER_GUILD, alice()),
        Event::GatewayClose(None),
    ]);
    let mut handler = Recorder {
        stop_after: Some((1, stop)),
        ..Recorder::default()
    };
    let exit = run(
        &mut source,
        &mut handler,
        config(),
        async {
            let _ = stopped.await;
        },
        |_| {},
    )
    .await;

    assert!(source.closed);
    assert_eq!(handler.events.len(), 1);
    assert_eq!(
        exit,
        RunExit::Shutdown {
            drained: true,
            dropped: 1
        },
        "one in-guild event arrived after shutdown and was not dispatched"
    );
}

#[tokio::test(start_paused = true)]
async fn shutdown_drain_is_bounded() {
    let mut source = FakeSource::new(Vec::new());
    let exit = run(
        &mut source,
        &mut Recorder::default(),
        config(),
        async {},
        |_| {},
    )
    .await;
    assert!(source.closed);
    assert_eq!(
        exit,
        RunExit::Shutdown {
            drained: false,
            dropped: 0
        }
    );
}

async fn close_with(code: u16) -> RunExit {
    let mut source = FakeSource::new(vec![
        Ok(ready(SELF_ID)),
        Ok(Event::GatewayClose(Some(CloseFrame::new(code, "closed")))),
    ]);
    source.ends = true;
    run(
        &mut source,
        &mut Recorder::default(),
        config(),
        std::future::pending(),
        |_| {},
    )
    .await
}

#[tokio::test]
async fn fatal_close_codes_are_reported_with_guidance() {
    let RunExit::Closed { reason } = close_with(4014).await else {
        panic!("closed");
    };
    assert_eq!(reason, CloseReason::DisallowedIntents);
    assert_eq!(reason.code(), Some(4014));
    let text = reason.to_string();
    assert!(text.contains("4014"), "{text}");
    assert!(
        text.contains("Server Members and Message Content"),
        "{text}"
    );

    let RunExit::Closed { reason } = close_with(4004).await else {
        panic!("closed");
    };
    assert_eq!(reason, CloseReason::AuthenticationFailed);
    assert!(reason.to_string().contains("token"));

    assert_eq!(
        close_with(4999).await,
        RunExit::Closed {
            reason: CloseReason::Other(Some(4999))
        }
    );
}

#[tokio::test]
async fn a_recovered_close_is_not_blamed_for_a_later_end() {
    let mut source = FakeSource::new(vec![
        Ok(Event::GatewayClose(Some(CloseFrame::new(
            4000,
            "unknown error",
        )))),
        Ok(ready(SELF_ID)),
    ]);
    source.ends = true;
    let exit = run(
        &mut source,
        &mut Recorder::default(),
        config(),
        std::future::pending(),
        |_| {},
    )
    .await;
    assert_eq!(
        exit,
        RunExit::Closed {
            reason: CloseReason::Other(None)
        }
    );
}
