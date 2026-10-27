//! The redesigned countdown: boss, live countdown, phrase and who in the
//! content; one embed coloured by state (red at risk, amber while someone
//! is still to answer, else green) with the lead portrait and the answers;
//! "Open in portal" to the run while the portal is open.

use crate::cards::art::lead_portrait;
use crate::cards::common::{CardContext, People, REACT_HINT, declined, not_declined, unanswered};
use crate::cards::heading::COUNTDOWN_PHRASE_SEED;
use crate::cards::redesign::portal::add_portal_button;
use crate::cards::redesign::vocab::{
    AT_RISK_RED, SETTLED_GREEN, WAITING_AMBER, boss_labels, relative_time, short_time,
};
use crate::cards::{Card, CardEmbed};
use kanade_domain::schedule::{Run, RunStatus};

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
    let mut rows = Vec::new();
    add_portal_button(&mut rows, ctx.v2, &[run.id.as_str()]);
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
        components: Vec::new(),
        rows: Some(rows),
    }
}
