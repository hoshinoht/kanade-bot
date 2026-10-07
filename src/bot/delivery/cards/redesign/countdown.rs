//! The redesigned countdown: boss, live countdown, phrase and who in the
//! content; one embed coloured by state (red at risk, amber while someone
//! is still to answer, else green) with the lead portrait and the answers.

use super::super::art::lead_portrait;
use super::super::common::{CardContext, People, REACT_HINT, declined, not_declined, unanswered};
use super::super::heading::COUNTDOWN_PHRASE_SEED;
use super::super::{Card, CardEmbed};
use super::vocab::{
    AT_RISK_RED, SETTLED_GREEN, WAITING_AMBER, boss_labels, relative_time, short_time,
};
use crate::domain::schedule::{Run, RunStatus};

pub fn countdown_card(
    ctx: &CardContext<'_>,
    run: &Run,
    mentioned: &[String],
    phrase: Option<&str>,
) -> Card {
    let states = ctx.states(run);
    let pending = unanswered(&states);
    let out = declined(&states);
    let still_on = not_declined(&states);
    let who = People::new(ctx, &run.participants, mentioned);
    let content = format!(
        "⏰ **{}** {} · {} {}",
        boss_labels(&run.bosses, ctx.catalog, ctx.marks),
        relative_time(run.datetime),
        phrase.unwrap_or(COUNTDOWN_PHRASE_SEED),
        who.list(&still_on)
    );
    let mut detail = format!(
        "🕘 **{}** · ✅ {} in",
        short_time(run.datetime),
        still_on.len() - pending.len()
    );
    if !out.is_empty() {
        detail.push_str(&format!(" · {} out", who.each(&out).join(", ")));
    }
    if !pending.is_empty() {
        detail.push_str(&format!(" · waiting on {}", who.each(&pending).join(", ")));
    }
    let colour = if run.status == RunStatus::AtRisk {
        AT_RISK_RED
    } else if !pending.is_empty() {
        WAITING_AMBER
    } else {
        SETTLED_GREEN
    };
    Card {
        content,
        embeds: vec![CardEmbed {
            description: Some(detail),
            footer: (!pending.is_empty()).then(|| REACT_HINT.to_owned()),
            colour,
            thumbnail: lead_portrait(&run.bosses, ctx.catalog),
            lead: run.bosses.first().cloned(),
            ..CardEmbed::default()
        }],
    }
}
