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
use serde::de::DeserializeOwned;
use tokio::time::Instant;
use twilight_http::error::{Error, ErrorType};
use twilight_http::request::channel::reaction::RequestReactionType;
use twilight_http::response::{Response, ResponseFuture};
use twilight_http::{Client, api_error::ApiError};
use twilight_model::application::EmojiList;
use twilight_model::application::command::{Command, CommandOptionChoice};
use twilight_model::channel::message::{Component, Embed, MessageFlags};
use twilight_model::channel::{Channel, Message};
use twilight_model::guild::{Emoji, Member};
use twilight_model::http::attachment::Attachment;
use twilight_model::http::interaction::{
    InteractionResponse, InteractionResponseData, InteractionResponseType,
};
use twilight_model::id::{
    Id,
    marker::{ApplicationMarker, GuildMarker, UserMarker},
};
use twilight_model::user::CurrentUser;

use super::{
    AmbiguousKind, ApplicationEmoji, COMPONENTS_V2, CREATE_FLAGS, ChannelId, DiscordTransport,
    HistoryPage, InteractionRef, InteractionReply, MessageEdit, MessageId, Outcome,
    OutgoingMessage, Presence, RejectionKind, classify_status, legacy_rows, v2_body_valid,
};
use crate::api::auth::crypto::base64_standard;
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

    /// A production transport for the token's own application, its id read
    /// from `GET /applications/@me` (for callers that run before, or
    /// without, a gateway `READY`).
    pub async fn for_current_application(token: String, config: TransportConfig) -> Outcome<Self> {
        /// The only field read back from the application.
        #[derive(Deserialize)]
        struct Me {
            id: Id<ApplicationMarker>,
        }
        // The placeholder id is never sent: `/applications/@me` names none.
        let mut transport = Self::new(token, Id::new(1), config);
        let me: Outcome<Me> = transport
            .fetch(transport.client.current_user_application())
            .await;
        me.map(|me| {
            transport.application_id = me.id;
            transport
        })
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

    /// A read: the whole body as `U`. An unreadable body is ambiguous, like
    /// any response the transport cannot interpret.
    async fn fetch<T: Unpin, U: DeserializeOwned>(
        &self,
        request: impl IntoFuture<IntoFuture = ResponseFuture<T>>,
    ) -> Outcome<U> {
        Self::send(request, self.config.deadline, |response| async {
            match response.bytes().await {
                Ok(body) => serde_json::from_slice::<U>(&body).map_or(
                    Outcome::Ambiguous(AmbiguousKind::UnreadableResponse),
                    Outcome::Delivered,
                ),
                Err(_) => Outcome::Ambiguous(AmbiguousKind::UnreadableResponse),
            }
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

/// The only field read back from a fetched message's flags.
#[derive(Deserialize)]
struct Flagged {
    #[serde(default)]
    flags: Option<MessageFlags>,
}

fn application_emoji(emoji: Emoji) -> ApplicationEmoji {
    ApplicationEmoji {
        id: emoji.id,
        name: emoji.name,
        animated: emoji.animated,
    }
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

/// A response body; a V2 reply carries its layout and no content or embeds.
fn interaction_data(
    content: Option<String>,
    embeds: &[Embed],
    components: &[Component],
    flags: MessageFlags,
) -> InteractionResponseData {
    InteractionResponseData {
        allowed_mentions: Some(mentions::none()),
        content,
        embeds: (!embeds.is_empty()).then(|| embeds.to_vec()),
        components: (!components.is_empty()).then(|| components.to_vec()),
        flags: (!flags.is_empty()).then_some(flags),
        ..InteractionResponseData::default()
    }
}

/// The content a reply sends: none for a V2 or embed-only reply.
fn reply_content(reply: &InteractionReply) -> Option<&str> {
    (reply.components.is_empty() && (!reply.content.is_empty() || reply.embeds.is_empty()))
        .then_some(reply.content.as_str())
}

impl TwilightTransport {
    async fn create(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
        flags: MessageFlags,
    ) -> Outcome<MessageId> {
        let legacy = legacy_rows(message.content.as_deref(), &message.components);
        let flags = if message.components.is_empty() || legacy {
            flags
        } else {
            flags | COMPONENTS_V2
        };
        // Twilight forwards any bits; refuse what Discord would reject.
        if !CREATE_FLAGS.contains(flags)
            || !v2_body_valid(
                message.content.as_deref(),
                &message.embeds,
                &message.components,
                flags,
            )
        {
            return Outcome::DefinitelyRejected(RejectionKind::Invalid);
        }
        let mut request = self
            .client
            .create_message(channel)
            .allowed_mentions(Some(&message.allowed_mentions));
        // Before the components: Twilight validates them by the V2 flag.
        if !flags.is_empty() {
            request = request.flags(flags);
        }
        if !message.components.is_empty() {
            request = request.components(&message.components);
        }
        if let Some(content) = message
            .content
            .as_deref()
            .filter(|_| message.components.is_empty() || legacy)
        {
            request = request.content(content);
        }
        if !message.embeds.is_empty() {
            request = request.embeds(&message.embeds);
        }
        if let Some(target) = message.reply_to {
            request = request.reply(target).fail_if_not_exists(false);
        }
        let files: Vec<Attachment> = message
            .attachments
            .iter()
            .zip(0_u64..)
            .map(|(upload, id)| {
                Attachment::from_bytes(upload.filename.clone(), upload.bytes.to_vec(), id)
            })
            .collect();
        if !files.is_empty() {
            request = request.attachments(&files);
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
}

impl DiscordTransport for TwilightTransport {
    async fn create_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
    ) -> Outcome<MessageId> {
        self.create(channel, message, MessageFlags::empty()).await
    }

    async fn create_flagged_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
        flags: MessageFlags,
    ) -> Outcome<MessageId> {
        self.create(channel, message, flags).await
    }

    async fn trigger_typing(&self, channel: ChannelId) -> Outcome<()> {
        self.settle(self.client.create_typing_trigger(channel))
            .await
    }

    async fn edit_message(
        &self,
        channel: ChannelId,
        message: MessageId,
        edit: &MessageEdit,
    ) -> Outcome<()> {
        if !edit.valid() {
            return Outcome::DefinitelyRejected(RejectionKind::Invalid);
        }
        let mut request = self
            .client
            .update_message(channel, message)
            .allowed_mentions(Some(&edit.allowed_mentions));
        if let Some(components) = edit.v2_components() {
            // Discord: setting the flag needs `content: null` and `embeds: []`.
            return self
                .settle(
                    request
                        .flags(COMPONENTS_V2)
                        .content(None)
                        .embeds(Some(&[][..]))
                        .components(Some(components)),
                )
                .await;
        }
        if let Some(content) = &edit.content {
            request = request.content(Some(content));
        }
        if edit.is_legacy() {
            request = request.components(edit.components.as_deref());
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

    async fn message_flags(&self, channel: ChannelId, message: MessageId) -> Outcome<MessageFlags> {
        self.fetch(self.client.message(channel, message))
            .await
            .map(|read: Flagged| read.flags.unwrap_or_else(MessageFlags::empty))
    }

    async fn respond(&self, interaction: &InteractionRef, reply: &InteractionReply) -> Outcome<()> {
        if !reply.valid() {
            return Outcome::DefinitelyRejected(RejectionKind::Invalid);
        }
        let response = InteractionResponse {
            kind: InteractionResponseType::ChannelMessageWithSource,
            data: Some(interaction_data(
                // An embed-only reply sends no content at all.
                reply_content(reply).map(str::to_owned),
                &reply.embeds,
                &reply.components,
                reply.flags(),
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
        let flags = if ephemeral {
            MessageFlags::EPHEMERAL
        } else {
            MessageFlags::empty()
        };
        let response = InteractionResponse {
            kind: InteractionResponseType::DeferredChannelMessageWithSource,
            data: Some(interaction_data(None, &[], &[], flags)),
        };
        self.settle_interaction(
            self.client
                .interaction(self.application_id)
                .create_response(interaction.id, interaction.token(), &response),
        )
        .await
    }

    async fn defer_update(&self, interaction: &InteractionRef) -> Outcome<()> {
        let response = InteractionResponse {
            kind: InteractionResponseType::DeferredUpdateMessage,
            data: None,
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
        if !reply.valid() {
            return Outcome::DefinitelyRejected(RejectionKind::Invalid);
        }
        let none = mentions::none();
        let client = self.client.interaction(self.application_id);
        let mut request = client
            .update_response(interaction.token())
            .allowed_mentions(Some(&none));
        if !reply.components.is_empty() {
            // The deferral fixed the visibility; only the V2 flag is added.
            return self
                .settle(
                    request
                        .flags(COMPONENTS_V2)
                        .components(Some(&reply.components)),
                )
                .await;
        }
        request = request
            .content(reply_content(reply))
            .embeds((!reply.embeds.is_empty()).then_some(reply.embeds.as_slice()));
        self.settle(request).await
    }

    async fn followup(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> Outcome<()> {
        if !reply.valid() {
            return Outcome::DefinitelyRejected(RejectionKind::Invalid);
        }
        let none = mentions::none();
        let client = self.client.interaction(self.application_id);
        let flags = reply.flags();
        let mut request = client
            .create_followup(interaction.token())
            .allowed_mentions(Some(&none));
        // Before the components: Twilight validates them by the V2 flag.
        if !flags.is_empty() {
            request = request.flags(flags);
        }
        if !reply.components.is_empty() {
            request = request.components(&reply.components);
        }
        if !reply.content.is_empty() {
            request = request.content(&reply.content);
        }
        if !reply.embeds.is_empty() {
            request = request.embeds(&reply.embeds);
        }
        self.settle(request).await
    }

    async fn autocomplete(
        &self,
        interaction: &InteractionRef,
        choices: &[CommandOptionChoice],
    ) -> Outcome<()> {
        let response = InteractionResponse {
            kind: InteractionResponseType::ApplicationCommandAutocompleteResult,
            data: Some(InteractionResponseData {
                choices: Some(choices.iter().take(25).cloned().collect()),
                ..InteractionResponseData::default()
            }),
        };
        self.settle_interaction(
            self.client
                .interaction(self.application_id)
                .create_response(interaction.id, interaction.token(), &response),
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

    async fn reaction_users(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
        kind: twilight_model::channel::message::ReactionType,
        after: Option<Id<UserMarker>>,
        limit: u16,
    ) -> Outcome<Vec<Id<UserMarker>>> {
        let emoji = RequestReactionType::Unicode { name: emoji };
        let mut request = self
            .client
            .reactions(channel, message, &emoji)
            .kind(kind)
            .limit(limit);
        if let Some(after) = after {
            request = request.after(after);
        }
        self.fetch(request)
            .await
            .map(|users: Vec<twilight_model::user::User>| {
                users.into_iter().map(|user| user.id).collect()
            })
    }

    async fn list_members(
        &self,
        guild: Id<GuildMarker>,
        after: Option<Id<UserMarker>>,
        limit: u16,
    ) -> Outcome<Vec<Member>> {
        let mut request = self.client.guild_members(guild).limit(limit);
        if let Some(after) = after {
            request = request.after(after);
        }
        self.fetch(request).await
    }

    async fn channel_messages(
        &self,
        channel: ChannelId,
        page: HistoryPage,
        limit: u16,
    ) -> Outcome<Vec<Message>> {
        let request = self.client.channel_messages(channel);
        match page {
            HistoryPage::Latest => self.fetch(request.limit(limit)).await,
            HistoryPage::Before(id) => self.fetch(request.before(id).limit(limit)).await,
            HistoryPage::After(id) => self.fetch(request.after(id).limit(limit)).await,
        }
    }

    async fn guild_channels(&self, guild: Id<GuildMarker>) -> Outcome<Vec<Channel>> {
        self.fetch(self.client.guild_channels(guild)).await
    }

    async fn current_user(&self) -> Outcome<CurrentUser> {
        self.fetch(self.client.current_user()).await
    }

    async fn application_emojis(&self) -> Outcome<Vec<ApplicationEmoji>> {
        self.fetch(self.client.get_application_emojis(self.application_id))
            .await
            .map(|list: EmojiList| list.items.into_iter().map(application_emoji).collect())
    }

    async fn create_application_emoji(&self, name: &str, png: &[u8]) -> Outcome<ApplicationEmoji> {
        let image = format!("data:image/png;base64,{}", base64_standard(png));
        self.fetch(
            self.client
                .add_application_emoji(self.application_id, name, &image),
        )
        .await
        .map(application_emoji)
    }
}
