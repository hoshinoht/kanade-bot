//! The whole post-loop reply shaping (v4 `generate`): reground, scrub, bound.

use super::ground::ground;
use super::member::member_facing;
use super::tidy::{normalise, tidy_whole};
use crate::chat::tools::ToolOutcome;
use crate::domain::catalog::BossTable;

/// v4 bounded an over-long grounded reply by keeping the listing and dropping
/// the model's text around it, and cut any other reply at the bound.
/// `D-GROUND-FILTERED`: over the bound, runs that already happened leave the
/// listing; a reply still over it is kept whole and posted as follow-ups
/// (`reply_parts`).
pub fn shape_reply(reply: &str, outcomes: &[ToolOutcome], catalog: &BossTable) -> String {
    let grounded = ground(reply, outcomes, catalog);
    let Some(block) = &grounded.block else {
        return normalise(&member_facing(&grounded.text));
    };
    let whole = |text: &str, listing: &str| tidy_whole(&member_facing(text), Some(listing));
    if let Some(shaped) = whole(&grounded.text, &block.text) {
        return shaped;
    }
    let upcoming = block
        .upcoming()
        .filter(|_| grounded.text.contains(&block.text))
        .map(|listing| (grounded.text.replacen(&block.text, &listing, 1), listing));
    let Some((text, listing)) = upcoming else {
        return normalise(&member_facing(&grounded.text));
    };
    whole(&text, &listing).unwrap_or_else(|| normalise(&member_facing(&text)))
}
