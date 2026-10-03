//! Reply finishing after the loop (v4 `_finalize_write_reply`,
//! `_finalize_read_claim` and `generate`'s shaping).

use super::{Generation, RoundOutcome};
use crate::chat::sanitize::{
    claims_new_card, looks_like_clarification, member_facing, shape_reply, strip_false_card_claim,
    tidy,
};
use crate::chat::tools::{REFUSED, ToolName};
use crate::domain::catalog::BossTable;

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

fn finalize_write_reply(generation: &mut Generation) {
    let Some(last) = last_write(generation) else {
        return;
    };
    let posted = !last.posted.is_empty();
    if last.outcome.ok && posted {
        return;
    }
    if last.outcome.error == Some(REFUSED) && looks_like_clarification(&generation.reply) {
        return;
    }
    if generation.reply.is_empty() {
        return;
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
/// The catalog lets grounding check boss names in a kept sentence.
pub(super) fn finish(generation: &mut Generation, catalog: &BossTable) {
    finalize_write_reply(generation);
    finalize_read_claim(generation);
    if !generation.reply.is_empty() {
        let outcomes = if posted_card(generation) {
            Vec::new()
        } else {
            generation.tool_outcomes()
        };
        generation.reply = shape_reply(&generation.reply, &outcomes, catalog);
    }
}
