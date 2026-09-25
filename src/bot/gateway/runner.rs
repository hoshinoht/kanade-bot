//! The single-shard event loop.
//!
//! Reconnects and backoff are Twilight's: polling a [`Shard`] reconnects on
//! its own (exponential, 2^attempt seconds) and resumes where Discord allows.
//! The stream ends only after a fatal close (e.g. invalid token or disallowed
//! intents), which the runner reports as [`RunExit::Closed`] with the code of
//! the close frame that preceded it.
//!
//! Handler contract: [`EventHandler::handle`] runs inline in this loop, so it
//! must be short and must not await Discord transport calls; spawn that work
//! (see `commands::spawn_interaction`) so the shard keeps being polled.

use std::future::Future;
use std::time::Duration;

use twilight_gateway::{CloseFrame, Event, Shard};

use super::close::CloseReason;
use super::intents::WANTED_EVENTS;
use crate::bot::events::{EventHandler, GuildScope, route};

/// A receive failure with the payload stripped: Twilight's own error text can
/// quote a whole undecodable event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatewayError {
    /// A reconnect attempt failed; the shard keeps retrying with backoff.
    Reconnect,
    /// An event could not be decoded and was skipped.
    Undecodable,
    Other,
}

/// Where gateway events come from: a Twilight [`Shard`], or a fake in tests.
pub trait EventSource: Send {
    /// The next event; `None` once the connection is permanently closed.
    /// Must be cancel safe.
    fn next_event(&mut self) -> impl Future<Output = Option<Result<Event, GatewayError>>> + Send;

    /// Start a normal close; events keep arriving until the close frame.
    fn close(&mut self);
}

impl EventSource for Shard {
    async fn next_event(&mut self) -> Option<Result<Event, GatewayError>> {
        use twilight_gateway::error::ReceiveMessageErrorType as Kind;
        let item = twilight_gateway::StreamExt::next_event(self, WANTED_EVENTS).await?;
        Some(item.map_err(|error| match error.kind() {
            Kind::Reconnect => GatewayError::Reconnect,
            Kind::Deserializing { .. } => GatewayError::Undecodable,
            _ => GatewayError::Other,
        }))
    }

    fn close(&mut self) {
        Shard::close(self, CloseFrame::NORMAL);
    }
}

/// Why the loop stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunExit {
    /// Shutdown was requested; `drained` says whether the close completed
    /// within the drain timeout, `dropped` counts events not dispatched after
    /// shutdown began (reconciliation must pick them up on the next start).
    Shutdown { drained: bool, dropped: usize },
    /// The gateway closed for good without a shutdown request.
    Closed { reason: CloseReason },
}

/// Loop settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunnerConfig {
    pub scope: GuildScope,
    /// How long to wait for the close handshake after shutdown.
    pub drain_timeout: Duration,
}

/// Dispatch guild events to `handler` until `shutdown` resolves or the
/// gateway closes. Receive errors are reported to `on_error` and the loop
/// continues, since the shard recovers by itself.
///
/// `shutdown` is any future, e.g. the runtime's signal wait.
pub async fn run<S, H>(
    source: &mut S,
    handler: &mut H,
    config: RunnerConfig,
    shutdown: impl Future<Output = ()>,
    mut on_error: impl FnMut(GatewayError),
) -> RunExit
where
    S: EventSource,
    H: EventHandler,
{
    tokio::pin!(shutdown);
    // The code of the latest close frame, cleared by any later event.
    let mut last_close = None;
    loop {
        tokio::select! {
            biased;
            () = &mut shutdown => break,
            item = source.next_event() => match item {
                None => return RunExit::Closed { reason: CloseReason::from_code(last_close) },
                Some(Err(error)) => on_error(error),
                Some(Ok(event)) => {
                    last_close = match &event {
                        Event::GatewayClose(frame) => frame.as_ref().map(|frame| frame.code),
                        _ => None,
                    };
                    if let Some(event) = route(config.scope, event) {
                        handler.handle(event).await;
                    }
                }
            },
        }
    }
    source.close();
    let mut dropped = 0;
    let drain = async {
        loop {
            match source.next_event().await {
                None | Some(Ok(Event::GatewayClose(_))) => return,
                Some(Ok(event)) => dropped += usize::from(route(config.scope, event).is_some()),
                Some(Err(_)) => {}
            }
        }
    };
    let drained = tokio::time::timeout(config.drain_timeout, drain)
        .await
        .is_ok();
    RunExit::Shutdown { drained, dropped }
}
