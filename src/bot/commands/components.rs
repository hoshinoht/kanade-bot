//! Button presses (`MessageComponent` interactions) on the bot's Components
//! V2 messages, answered through the dispatcher so they share its guild
//! scope, redelivery dedupe and gates. Custom ids are parsed strictly
//! (`delivery::cards::redesign::ButtonId`); anything else is answered
//! ephemerally as no longer active.
//!
//! * `digest:mine`: `/schedule scope:mine` for the presser, through the
//!   command's own gate, answered ephemerally.
//! * `card:apply:<id>` / `card:reject:<id>`: acknowledged at once with a
//!   deferred update (the card itself shows the result), then answered by
//!   the reaction worker exactly as ✅/❌ by the presser would be. Only a
//!   refusal or a stale button gets an ephemeral follow-up.
//! * `run:done|missed|later:<run>:<ask>`: a run completion prompt's Done /
//!   Didn't happen / Not yet, as the presser's internal `/status` call
//!   (`run_prompt.rs`), answered ephemerally; the tick edits the prompt.

use std::sync::Arc;

use twilight_model::application::interaction::application_command::{
    CommandDataOption, CommandOptionValue,
};
use twilight_model::application::interaction::{Interaction, InteractionData, InteractionType};
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, GuildMarker, MessageMarker},
};

use super::access::Invoker;
use super::context::PortFuture;
use super::invocation::Invocation;
use crate::bot::cards::{CardPress, CardReaction, Pressed};
use crate::bot::events::RsvpAnswer;
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionRef;
use kanade_domain::completion::PromptOutcome;

/// The answer to a stale, forged or unknown button.
pub const INACTIVE: &str = "This button is no longer active.";
/// The answer to a press by someone who may not answer the card.
pub const NOT_YOURS: &str = "❌ That proposal is not yours to answer.";

/// Where Apply/Reject presses are answered (serve: the sequential reaction
/// worker). `None` when it is gone (shutting down).
pub type CardPresses = Arc<dyn Fn(CardPress) -> PortFuture<'static, Option<Pressed>> + Send + Sync>;

/// One guild button press.
#[derive(Clone, Debug, PartialEq)]
pub struct Press {
    pub interaction: InteractionRef,
    pub guild_id: Id<GuildMarker>,
    pub channel_id: Option<Id<ChannelMarker>>,
    /// The message the button is on.
    pub message_id: Option<Id<MessageMarker>>,
    pub invoker: Invoker,
    pub invoker_name: Option<String>,
    pub custom_id: String,
}

impl Press {
    /// `None` for anything but a guild button press with a member.
    pub fn from_interaction(interaction: &Interaction) -> Option<Self> {
        if interaction.kind != InteractionType::MessageComponent {
            return None;
        }
        let guild_id = interaction.guild_id?;
        let (invoker, invoker_name) = super::invocation::invoker(interaction)?;
        let Some(InteractionData::MessageComponent(data)) = &interaction.data else {
            return None;
        };
        Some(Self {
            interaction: InteractionRef::new(interaction.id, interaction.token.clone()),
            guild_id,
            channel_id: interaction.channel.as_ref().map(|channel| channel.id),
            message_id: interaction.message.as_ref().map(|message| message.id),
            invoker,
            invoker_name,
            custom_id: data.custom_id.clone(),
        })
    }

    /// The presser's internal call of the command at `path` with text
    /// `options` (through that command's gate).
    fn call<'o>(
        &self,
        path: &[&str],
        options: impl IntoIterator<Item = (&'o str, &'o str)>,
    ) -> Invocation {
        Invocation {
            interaction: self.interaction.clone(),
            guild_id: self.guild_id,
            channel_id: self.channel_id,
            invoker: self.invoker.clone(),
            invoker_name: self.invoker_name.clone(),
            path: path.iter().map(|name| (*name).to_owned()).collect(),
            options: options
                .into_iter()
                .map(|(name, value)| CommandDataOption {
                    name: name.to_owned(),
                    value: CommandOptionValue::String(value.to_owned()),
                })
                .collect(),
            autocomplete: false,
            owner_id: None,
        }
    }

    /// `/schedule scope:mine` as the presser asked it.
    pub fn schedule_mine(&self) -> Invocation {
        self.call(&["schedule"], [("scope", "mine")])
    }

    /// An ownership request's Accept/Decline press, as the presser's
    /// internal `/fixed` call.
    pub fn owner_answer(&self, request_id: &str, accept: bool) -> Invocation {
        let answer = if accept { "accept" } else { "decline" };
        self.call(
            &["fixed", super::fixed_owner::OWNER_PRESS],
            [("request", request_id), ("answer", answer)],
        )
    }

    /// A run completion prompt's press, as the presser's internal `/status`
    /// call.
    pub fn prompt_answer(&self, run_id: &str, ask: u32, outcome: PromptOutcome) -> Invocation {
        let ask = ask.to_string();
        self.call(
            &["status", super::run_prompt::PROMPT_PRESS],
            [("run", run_id), ("ask", &ask), ("answer", outcome.as_str())],
        )
    }

    /// A `/character link` notice's Link or Cancel, as the presser's
    /// internal `/character <leaf>` call.
    pub fn character_answer(&self, leaf: &str, options: &[(&str, String)]) -> Invocation {
        self.call(
            &["character", leaf],
            options.iter().map(|(name, value)| (*name, value.as_str())),
        )
    }

    /// The card press for the proposal a button names.
    pub fn card(&self, proposal_id: &str, answer: RsvpAnswer) -> Option<CardPress> {
        Some(CardPress {
            message_id: id_text(self.message_id?),
            proposal_id: proposal_id.to_owned(),
            user_id: id_text(self.invoker.user_id),
            answer,
        })
    }
}

/// The presser's ephemeral follow-up for what a press did, if any.
pub fn press_follow_up(pressed: &Pressed) -> Option<&'static str> {
    match pressed {
        Pressed::NotYours => Some(NOT_YOURS),
        Pressed::Inactive | Pressed::Answered(CardReaction::Ignored) => Some(INACTIVE),
        Pressed::Answered(_) => None,
    }
}
