//! Complete, two-sided reaction reads shared by offline replayers.

use std::collections::BTreeSet;

use twilight_model::channel::message::ReactionType;
use twilight_model::id::{Id, marker::UserMarker};

use crate::bot::transport::{ChannelId, DiscordTransport, MAX_REACTIONS_PAGE, MessageId, Outcome};

/// A complete snapshot of the two RSVP reaction sides.
#[derive(Clone, Debug, Default)]
pub(crate) struct ReactionAnswers {
    pub yes: BTreeSet<Id<UserMarker>>,
    pub no: BTreeSet<Id<UserMarker>>,
    pub own_yes: bool,
    pub own_no: bool,
    pub requests: usize,
}

/// A failed read still reports the requests already made for pass telemetry.
#[derive(Clone, Debug)]
pub(crate) struct ReadError {
    pub reason: String,
    pub requests: usize,
}

/// Read both normal and burst reactions for each RSVP emoji. A full final page
/// is deliberately incomplete: absence is never inferred beyond the cap.
pub(crate) async fn read_answers<T: DiscordTransport>(
    transport: &T,
    channel: ChannelId,
    message: MessageId,
    self_id: Id<UserMarker>,
    pages: usize,
    current: &impl Fn() -> bool,
) -> Result<ReactionAnswers, ReadError> {
    let mut result = ReactionAnswers::default();
    for (emoji, users, own) in [
        ("✅", &mut result.yes, &mut result.own_yes),
        ("❌", &mut result.no, &mut result.own_no),
    ] {
        for kind in [ReactionType::Normal, ReactionType::Burst] {
            let mut after = None;
            let mut complete = false;
            for _ in 0..pages {
                if !current() {
                    return Err(ReadError {
                        reason: "stale_generation".into(),
                        requests: result.requests,
                    });
                }
                result.requests += 1;
                let page = match transport
                    .reaction_users(channel, message, emoji, kind, after, MAX_REACTIONS_PAGE)
                    .await
                {
                    Outcome::Delivered(page) => page,
                    outcome => {
                        return Err(ReadError {
                            reason: outcome.failure_label().unwrap_or_default(),
                            requests: result.requests,
                        });
                    }
                };
                if page.len() > usize::from(MAX_REACTIONS_PAGE)
                    || page
                        .iter()
                        .any(|id| after.is_some_and(|after| *id <= after))
                {
                    return Err(ReadError {
                        reason: "invalid_page".into(),
                        requests: result.requests,
                    });
                }
                *own |= page.contains(&self_id);
                users.extend(page.iter().filter(|id| **id != self_id).copied());
                if page.len() < usize::from(MAX_REACTIONS_PAGE) {
                    complete = true;
                    break;
                }
                after = page.iter().max().copied();
            }
            if !complete {
                return Err(ReadError {
                    reason: "page_cap".into(),
                    requests: result.requests,
                });
            }
        }
    }
    Ok(result)
}
