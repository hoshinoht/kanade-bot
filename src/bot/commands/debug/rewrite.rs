//! `/debug rewrite`: rewrite every header posted this boss week in place
//! (`delivery::ManualRewrite`). The reply comes at once with the count; the
//! summary is posted in the invoking channel when the run ends.

use super::super::context::CommandContext;
use super::super::dispatch::CommandError;
use super::super::invocation::Invocation;
use crate::bot::delivery::{ManualRequest, ManualStart};
use crate::bot::ids::id_text;
use crate::bot::transport::InteractionReply;

/// While Discord delivery is not composed.
pub const REWRITE_UNAVAILABLE: &str = "Header rewrites aren't available right now.";
/// A second trigger while a run is queued or running.
pub const REWRITE_RUNNING: &str =
    "A header rewrite is already running; its summary is posted when it ends.";

pub async fn run(
    ctx: &CommandContext,
    invocation: &Invocation,
) -> Result<InteractionReply, CommandError> {
    let start = ctx
        .header_rewrite
        .as_ref()
        .ok_or_else(|| CommandError::User(REWRITE_UNAVAILABLE.into()))?;
    let started = start(ManualRequest {
        actor: format!("member:{}", id_text(invocation.invoker.user_id)),
        report_to: invocation.channel_id.map(id_text),
    })
    .await;
    Ok(InteractionReply::ephemeral(match started {
        ManualStart::Started(count) => format!(
            "✏️ Rewriting {count} header(s) posted this boss week; the posts are edited in place \
             and a summary follows here when it ends. Every call is in the Rewrites log."
        ),
        ManualStart::Running => return Err(CommandError::User(REWRITE_RUNNING.into())),
        ManualStart::Disabled => "Header rewrites aren't set up here (no rewrite model or \
                                  persona), so there is nothing to rewrite."
            .to_owned(),
        ManualStart::Nothing => "Nothing posted this boss week has a header to rewrite.".to_owned(),
        ManualStart::Unavailable => {
            return Err(CommandError::User(REWRITE_UNAVAILABLE.into()));
        }
    }))
}
