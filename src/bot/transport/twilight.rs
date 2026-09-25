//! [`DiscordTransport`] over `twilight-http`.
//!
//! Retry behaviour, verified against twilight-http 0.17.1 and hyper-util 0.1.20:
//! * Twilight re-sends only after HTTP 429, which Discord returns for requests
//!   it did not process, so the re-send is not a replay of a delivered post.
//!   Each send first waits for a rate-limiter permit; a pre-flight check then
//!   allows at most [`MAX_SENDS`] sends within the deadline and otherwise
//!   cancels before sending. Without a rate limiter (tests only) re-sends
//!   are immediate and uncounted.
//! * hyper-util's `retry_canceled_requests` re-sends only requests that were
//!   never written to a reused connection.
//! * Nothing else is retried. Hitting the overall deadline before any send
//!   is `NotSent`; after one it is [`AmbiguousKind::Timeout`].
//!
//! Errors are reduced to [`Outcome`]; response bodies, request payloads and
//! the token never leave this module.

use std::error::Error as _;
use std::fmt;
use std::future::{Future, IntoFuture};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use serde::Deserialize;
use tokio::time::Instant;
use twilight_http::error::{Error, ErrorType};
use twilight_http::request::channel::reaction::RequestReactionType;
use twilight_http::response::{Response, ResponseFuture};
use twilight_http::{Client, api_error::ApiError};
use twilight_model::application::command::Command;
use twilight_model::channel::message::MessageFlags;
use twilight_model::http::interaction::{
    InteractionResponse, InteractionResponseData, InteractionResponseType,
};
use twilight_model::id::{
    Id,
    marker::{ApplicationMarker, GuildMarker},
};

use super::{
    AmbiguousKind, ChannelId, DiscordTransport, InteractionRef, InteractionReply, MessageEdit,
    MessageId, Outcome, OutgoingMessage, Presence, RejectionKind, classify_status,
};
use crate::bot::mentions;

/// One send plus at most three re-sends after 429.
pub const MAX_SENDS: u32 = 4;

/// Timeouts for [`TwilightTransport`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportConfig {
    /// One HTTP attempt, until response headers (Twilight's own timeout).
    pub attempt_timeout: Duration,
    /// The whole call: rate-limit waits, 429 re-sends and body reads.
    pub deadline: Duration,
    /// Initial interaction responses; Discord allows 3 s from the event.
    pub respond_deadline: Duration,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            attempt_timeout: Duration::from_secs(10),
            deadline: Duration::from_secs(30),
            respond_deadline: Duration::from_millis(2_500),
        }
    }
}

pub struct TwilightTransport {
    client: Client,
    application_id: Id<ApplicationMarker>,
    config: TransportConfig,
}

impl fmt::Debug for TwilightTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TwilightTransport")
            .field("application_id", &self.application_id)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// Counts sends the pre-flight check allowed.
struct Sends(Arc<AtomicU32>);

impl Sends {
    fn count(&self) -> u32 {
        self.0.load(Ordering::SeqCst)
    }
}

impl TwilightTransport {
    /// A production client with the rate limiter enabled. The process Rustls
    /// provider must already be installed (`runtime::tls`); the client's
    /// default allow-list mentions nobody as a backstop.
    pub fn new(
        token: String,
        application_id: Id<ApplicationMarker>,
        config: TransportConfig,
    ) -> Self {
        let client = Client::builder()
            .token(token)
            .timeout(config.attempt_timeout)
            .default_allowed_mentions(mentions::none())
            .build();
        Self::from_client(client, application_id, config)
    }

    /// Wrap a prepared client, e.g. one pointed at a loopback stub.
    pub fn from_client(
        client: Client,
        application_id: Id<ApplicationMarker>,
        config: TransportConfig,
    ) -> Self {
        Self {
            client,
            application_id,
            config,
        }
    }

    /// Install the send guard on a request.
    fn guard<T>(
        request: impl IntoFuture<IntoFuture = ResponseFuture<T>>,
        deadline: Duration,
    ) -> (ResponseFuture<T>, Sends) {
        let mut future = request.into_future();
        let sends = Arc::new(AtomicU32::new(0));
        let counter = Arc::clone(&sends);
        let send_by = Instant::now() + deadline;
        let guarded = future.set_pre_flight(move || {
            if Instant::now() >= send_by || counter.load(Ordering::SeqCst) >= MAX_SENDS {
                return false;
            }
            counter.fetch_add(1, Ordering::SeqCst);
            true
        });
        if !guarded {
            // No rate limiter: sends cannot be observed, so assume one.
            sends.store(1, Ordering::SeqCst);
        }
        (future, Sends(sends))
    }

    /// Send `request` and finish with `read`, all within `deadline`.
    async fn send<T, U, F>(
        request: impl IntoFuture<IntoFuture = ResponseFuture<T>>,
        deadline: Duration,
        read: impl FnOnce(Response<T>) -> F,
    ) -> Outcome<U>
    where
        T: Unpin,
        F: Future<Output = Outcome<U>>,
    {
        let (future, sends) = Self::guard(request, deadline);
        let call = async {
            match future.await {
                Ok(response) => read(response).await,
                // Cancelled after sends: every earlier send was answered 429.
                Err(error)
                    if matches!(error.kind(), ErrorType::RequestCanceled) && sends.count() > 0 =>
                {
                    Outcome::DefinitelyRejected(RejectionKind::RateLimited)
                }
                Err(error) => classify_error(&error),
            }
        };
        match tokio::time::timeout(deadline, call).await {
            Ok(outcome) => outcome,
            Err(_) if sends.count() == 0 => Outcome::DefinitelyRejected(RejectionKind::NotSent),
            Err(_) => Outcome::Ambiguous(AmbiguousKind::Timeout),
        }
    }

    async fn settle<T: Unpin>(
        &self,
        request: impl IntoFuture<IntoFuture = ResponseFuture<T>>,
    ) -> Outcome<()> {
        Self::send(request, self.config.deadline, |_| async {
            Outcome::Delivered(())
        })
        .await
    }

    async fn settle_interaction<T: Unpin>(
        &self,
        request: impl IntoFuture<IntoFuture = ResponseFuture<T>>,
    ) -> Outcome<()> {
        Self::send(request, self.config.respond_deadline, |_| async {
            Outcome::Delivered(())
        })
        .await
    }
}

/// The only field read back from a created message.
#[derive(Deserialize)]
struct Created {
    id: MessageId,
}

/// Classify a failed Twilight request.
fn classify_error<T>(error: &Error) -> Outcome<T> {
    match error.kind() {
        ErrorType::Validation
        | ErrorType::BuildingRequest
        | ErrorType::CreatingHeader { .. }
        | ErrorType::Json => Outcome::DefinitelyRejected(RejectionKind::Invalid),
        ErrorType::RequestCanceled => Outcome::DefinitelyRejected(RejectionKind::NotSent),
        ErrorType::Unauthorized => Outcome::DefinitelyRejected(RejectionKind::Unauthorized),
        ErrorType::RequestTimedOut => Outcome::Ambiguous(AmbiguousKind::Timeout),
        ErrorType::RequestError if never_connected(error) => {
            Outcome::DefinitelyRejected(RejectionKind::NotSent)
        }
        // Includes error-body read failures, whose source is a body error.
        ErrorType::RequestError => Outcome::Ambiguous(AmbiguousKind::Connection),
        // The status is lost when the error body is not JSON.
        ErrorType::Parsing { .. } => Outcome::Ambiguous(AmbiguousKind::UnreadableResponse),
        ErrorType::Response { error, status, .. } => {
            let code = match error {
                ApiError::General(general) => Some(general.code),
                _ => None,
            };
            classify_status(status.get(), code)
        }
        _ => Outcome::Ambiguous(AmbiguousKind::UnreadableResponse),
    }
}

/// hyper-util reports `Connect` only while obtaining a connection (DNS,
/// TCP, TLS handshake), before the request is handed to it.
fn never_connected(error: &Error) -> bool {
    error
        .source()
        .and_then(|source| source.downcast_ref::<hyper_util::client::legacy::Error>())
        .is_some_and(hyper_util::client::legacy::Error::is_connect)
}

fn unicode(emoji: &str) -> RequestReactionType<'_> {
    RequestReactionType::Unicode { name: emoji }
}

fn interaction_data(content: Option<String>, ephemeral: bool) -> InteractionResponseData {
    InteractionResponseData {
        allowed_mentions: Some(mentions::none()),
        content,
        flags: ephemeral.then_some(MessageFlags::EPHEMERAL),
        ..InteractionResponseData::default()
    }
}

impl DiscordTransport for TwilightTransport {
    async fn create_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
    ) -> Outcome<MessageId> {
        let mut request = self
            .client
            .create_message(channel)
            .allowed_mentions(Some(&message.allowed_mentions));
        if let Some(content) = &message.content {
            request = request.content(content);
        }
        if !message.embeds.is_empty() {
            request = request.embeds(&message.embeds);
        }
        Self::send(request, self.config.deadline, |response| async {
            // Delivered from here on; an unreadable id cannot be bound.
            match response.bytes().await {
                Ok(body) => serde_json::from_slice::<Created>(&body).map_or(
                    Outcome::Ambiguous(AmbiguousKind::UnreadableResponse),
                    |created| Outcome::Delivered(created.id),
                ),
                Err(_) => Outcome::Ambiguous(AmbiguousKind::UnreadableResponse),
            }
        })
        .await
    }

    async fn edit_message(
        &self,
        channel: ChannelId,
        message: MessageId,
        edit: &MessageEdit,
    ) -> Outcome<()> {
        let mut request = self
            .client
            .update_message(channel, message)
            .allowed_mentions(Some(&edit.allowed_mentions));
        if let Some(content) = &edit.content {
            request = request.content(Some(content));
        }
        if let Some(embeds) = &edit.embeds {
            request = request.embeds(Some(embeds));
        }
        self.settle(request).await
    }

    async fn delete_message(&self, channel: ChannelId, message: MessageId) -> Outcome<()> {
        self.settle(self.client.delete_message(channel, message))
            .await
    }

    async fn add_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> Outcome<()> {
        let emoji = unicode(emoji);
        self.settle(self.client.create_reaction(channel, message, &emoji))
            .await
    }

    async fn remove_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> Outcome<()> {
        let emoji = unicode(emoji);
        self.settle(
            self.client
                .delete_current_user_reaction(channel, message, &emoji),
        )
        .await
    }

    async fn message_presence(&self, channel: ChannelId, message: MessageId) -> Outcome<Presence> {
        match self.settle(self.client.message(channel, message)).await {
            Outcome::Delivered(()) => Outcome::Delivered(Presence::Present),
            Outcome::DefinitelyRejected(RejectionKind::UnknownMessage) => {
                Outcome::Delivered(Presence::Absent)
            }
            other => other.map(|()| Presence::Present),
        }
    }

    async fn respond(&self, interaction: &InteractionRef, reply: &InteractionReply) -> Outcome<()> {
        let response = InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(interaction_data(
                Some(reply.content.clone()),
                reply.ephemeral,
            )),
        };
        self.settle_interaction(
            self.client
                .interaction(self.application_id)
                .create_response(interaction.id, interaction.token(), &response),
        )
        .await
    }

    async fn defer(&self, interaction: &InteractionRef, ephemeral: bool) -> Outcome<()> {
        let response = InteractionResponse {
            kind: InteractionResponseType::DeferredChannelMessageWithSource,
            data: Some(interaction_data(None, ephemeral)),
        };
        self.settle_interaction(
            self.client
                .interaction(self.application_id)
                .create_response(interaction.id, interaction.token(), &response),
        )
        .await
    }

    async fn complete_deferred(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> Outcome<()> {
        let none = mentions::none();
        self.settle(
            self.client
                .interaction(self.application_id)
                .update_response(interaction.token())
                .content(Some(&reply.content))
                .allowed_mentions(Some(&none)),
        )
        .await
    }

    async fn register_guild_commands(
        &self,
        guild: Id<GuildMarker>,
        commands: &[Command],
    ) -> Outcome<()> {
        self.settle(
            self.client
                .interaction(self.application_id)
                .set_guild_commands(guild, commands),
        )
        .await
    }
}
