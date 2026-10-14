//! The single authority boundary for chatbot cards about existing schedule
//! objects (v4 `tools/authority.py`). Enforced by the dispatcher whatever
//! tools were offered; only an admin is exempt.

use crate::chat::gate::{ChannelDirectory, PilotSettings, is_chat_channel};
use crate::chat::tools::read::format::channel_reference;
use crate::chat::tools::{ToolError, ToolResult};
use crate::domain::ids::short_id;
use crate::domain::schedule::{FixedRun, Run, ScheduleSnapshot};

/// A change to somebody else's run; names the run and nobody on it.
const NOT_THEIRS_RUN: &str = "They are not on run {sid} and do not own the weekly timing behind it, so it is not theirs to change. Say that only the people on a run -- or the owner of the weekly timing it comes from -- can propose a change to it, and that putting somebody on a run is not something you can do. Do not name anybody on it.";

const NOT_THEIRS_FIXED: &str = "They are not on the weekly timing {sid} and do not own it, so it is not theirs to remove. Say that only the people on it, or whoever owns it, can propose that. Do not name anybody on it.";

/// A change proposed from another pilot channel; the channel is the only
/// thing it may name.
const ELSEWHERE: &str = "That {noun} lives in {where}, and changes to it are proposed from its own channel. Tell them which channel it lives in and to ask there. Say nothing else about it.";

/// What a card would change.
#[derive(Clone, Copy, Debug)]
pub enum Subject<'a> {
    Run(&'a Run),
    Fixed(&'a FixedRun),
}

/// The trusted asker and the channels to judge a home channel by.
#[derive(Clone, Copy)]
pub struct Asker<'a> {
    pub author_id: &'a str,
    pub channel_id: &'a str,
    pub is_admin: bool,
    pub channels: &'a (dyn ChannelDirectory + Sync),
    pub pilot: &'a PilotSettings,
}

/// Refuse cross-party cards, and cards about something whose home is a
/// different pilot channel, unless the asker is an admin.
///
/// # Errors
/// A [`ToolError`] the model reads back to the member.
pub fn require_authority(
    asker: Asker<'_>,
    subject: Subject<'_>,
    snapshot: &ScheduleSnapshot,
) -> ToolResult<()> {
    if asker.is_admin {
        return Ok(());
    }
    let (id, participants, owner, home, noun) = match subject {
        Subject::Run(run) => {
            let owner = run.fixed_run_id.as_deref().and_then(|fixed| {
                snapshot
                    .fixed_runs
                    .iter()
                    .find(|row| row.id == fixed)
                    .map(|row| row.owner())
            });
            (&run.id, &run.participants, owner, &run.channel_id, "run")
        }
        Subject::Fixed(fixed) => (
            &fixed.id,
            &fixed.participants,
            Some(fixed.owner()),
            &fixed.channel_id,
            "weekly timing",
        ),
    };
    let on_it = participants.iter().any(|p| p == asker.author_id);
    if !on_it && owner != Some(asker.author_id) {
        let template = match subject {
            Subject::Run(_) => NOT_THEIRS_RUN,
            Subject::Fixed(_) => NOT_THEIRS_FIXED,
        };
        return Err(ToolError(template.replace("{sid}", &short_id(id))));
    }
    let home = home.as_deref().unwrap_or_default();
    if !home.is_empty() && home != asker.channel_id && pilot_channel(asker, home) {
        let where_ =
            channel_reference(asker.channels, home).unwrap_or_else(|| "another channel".to_owned());
        return Err(ToolError(
            ELSEWHERE
                .replace("{noun}", noun)
                .replace("{where}", &where_),
        ));
    }
    Ok(())
}

/// A channel the pilot answers in; unknown channels are not.
fn pilot_channel(asker: Asker<'_>, channel_id: &str) -> bool {
    let known = asker.channels.channel(channel_id);
    is_chat_channel(known.as_ref(), asker.channels, asker.pilot)
}
