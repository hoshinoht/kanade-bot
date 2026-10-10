//! Reply finishing after the loop (v4 `_finalize_write_reply`,
//! `_finalize_read_claim` and `generate`'s shaping).

use super::{Generation, RoundOutcome};
use crate::chat::context::strip_block_copies;
use crate::chat::sanitize::{
    claims_new_card, looks_like_clarification, member_facing, shape_reply, strip_context_copies,
    strip_false_card_claim, tidy,
};
use crate::chat::tools::{REFUSED, ToolName, ToolOutcome};

/// A posted card whose question then failed: the change is recorded, so the
/// member must not be invited to ask again.
pub(super) const POSTED_UNFINISHED: &str =
    "The requested card was posted, but the request did not finish cleanly.";

/// A write claim must never outlive the write it claims: the last write call
/// decides, and a refused one overwrites the reply unless it already asks.
fn last_write(generation: &Generation) -> Option<&RoundOutcome> {
    generation
        .outcomes
        .iter()
        .rev()
        .find(|o| ToolName::parse(&o.outcome.name).is_some_and(ToolName::is_write))
}

/// The last write call succeeded and posted its card.
fn posted_card(generation: &Generation) -> bool {
    last_write(generation).is_some_and(|last| last.outcome.ok && !last.posted.is_empty())
}

/// `true` when the model's reply was replaced by the fixed status text.
fn finalize_write_reply(generation: &mut Generation) -> bool {
    let Some(last) = last_write(generation) else {
        return false;
    };
    let posted = !last.posted.is_empty();
    if last.outcome.ok && posted {
        return false;
    }
    if last.outcome.error == Some(REFUSED) && looks_like_clarification(&generation.reply) {
        return false;
    }
    if generation.reply.is_empty() {
        return false;
    }
    let detail = tidy(&member_facing(&last.outcome.output), None);
    let status = if posted {
        POSTED_UNFINISHED
    } else {
        "The requested card was not posted."
    };
    let text = if detail.is_empty() {
        status.to_owned()
    } else {
        format!("{status} {detail}")
    };
    generation.reply = tidy(&text, None);
    true
}

/// No new-card embroidery on a turn that posted nothing.
fn finalize_read_claim(generation: &mut Generation) {
    let posted =
        !generation.posted.is_empty() || generation.outcomes.iter().any(|o| !o.posted.is_empty());
    if !posted && !generation.reply.is_empty() && claims_new_card(&generation.reply) {
        generation.reply = strip_false_card_claim(&generation.reply);
    }
}

/// `D-GROUND-WRITE`: a turn whose last write posted a card keeps the model's
/// card reply; v4 regrounded it, so a time in it pulled in the lookup listing.
/// `listing` (`D-MIXED-PEOPLE`) is grounded against after every call, as the
/// latest listing. `run_context` is the question's `D-RUN-CONTEXT` block:
/// any copy of it leaves the reply first. Returns `true` when the model's
/// words were replaced whole by fixed text (an unposted write), so members
/// never see them.
pub(super) fn finish(
    generation: &mut Generation,
    listing: Option<&ToolOutcome>,
    run_context: &str,
) -> bool {
    let replaced = finalize_write_reply(generation);
    finalize_read_claim(generation);
    if !generation.reply.is_empty() {
        let reply = strip_block_copies(&generation.reply, run_context);
        let mut outcomes = generation.tool_outcomes();
        if posted_card(generation) {
            // Not regrounded, but a copied model-only context line still goes.
            let reply = strip_context_copies(&reply, &outcomes);
            generation.reply = shape_reply(&reply, &[]);
        } else {
            outcomes.extend(listing.cloned());
            generation.reply = shape_reply(&reply, &outcomes);
        }
    }
    replaced
}
