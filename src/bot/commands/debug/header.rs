//! `/debug header`: try the live persona header rewrite a few times and post
//! each line with its verdict and latency. Nothing is stored.

use twilight_model::application::command::CommandOption;

use super::super::build::{choices, integer, text_channel};
use super::super::context::{CommandContext, HeaderRequest, HeaderTrialKind, HeaderTrials};
use super::super::dispatch::CommandError;
use super::super::invocation::Invocation;
use super::super::options::Args;
use crate::bot::delivery::MAX_HEADER_TRIES;
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;

const DEFAULT_TRIES: u8 = 3;

pub fn options() -> Vec<CommandOption> {
    let kinds: Vec<(&str, &str)> = HeaderTrialKind::ALL
        .iter()
        .map(|kind| (kind.as_str(), kind.as_str()))
        .collect();
    vec![
        choices("kind", "Which header to rewrite", true, &kinds),
        integer(
            "tries",
            "How many rewrites (default 3)",
            1,
            i64::from(MAX_HEADER_TRIES),
        ),
        text_channel("channel", "Post the results here"),
    ]
}

pub async fn run(
    ctx: &CommandContext,
    invocation: &Invocation,
) -> Result<InteractionReply, CommandError> {
    let args = Args(&invocation.options);
    let wanted = args.text("kind").unwrap_or_default();
    let kind = HeaderTrialKind::parse(wanted)
        .ok_or_else(|| CommandError::User(format!("Don't know the `{wanted}` header.")))?;
    let tries = args
        .integer("tries")
        .and_then(|tries| u8::try_from(tries).ok())
        .unwrap_or(DEFAULT_TRIES)
        .clamp(1, MAX_HEADER_TRIES);
    let cards = ctx
        .debug_cards
        .as_ref()
        .ok_or_else(|| CommandError::User(super::TEST_CARDS_UNAVAILABLE.into()))?;
    let done = cards
        .headers(HeaderRequest {
            kind,
            tries,
            channel: args.channel("channel"),
            invoked_in: invocation.channel_id.map(id_text),
        })
        .await
        .map_err(CommandError::Internal)?;
    Ok(InteractionReply::ephemeral(match done {
        HeaderTrials::Posted {
            channel_id,
            accepted,
        } => format!(
            "✅ Tried the `{}` header rewrite {tries} time(s), {accepted} accepted; results \
             posted in <#{channel_id}>. Nothing was stored.",
            kind.as_str()
        ),
        HeaderTrials::Disabled => "Header rewrites aren't set up here (no rewrite model or \
                                   persona), so there is nothing to try."
            .to_owned(),
        HeaderTrials::Unreachable => {
            return Err(CommandError::User("That channel isn't reachable.".into()));
        }
        HeaderTrials::Unconfirmed => "⚠️ Delivery of the results was not confirmed. Check the \
                                      channel before retrying."
            .to_owned(),
    }))
}
