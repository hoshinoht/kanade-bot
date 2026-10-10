//! v4 `day_of_card`: the grouped morning ping, one embed field per run.
//! The heading line (v4 `Today — <day>`) is passed in: it may be the
//! persona rewrite (see `heading.rs`).

use super::art::{lead_entry_art, lead_portrait};
use super::common::{
    COLOUR_DAY_OF, CardContext, People, REACT_HINT, boss_detail, everyone_on, format_bosses,
    lead_colour, local_time, status_line, unanswered,
};
use super::{Card, CardEmbed, CardField};
use crate::domain::schedule::{Run, RunStatus};

/// The runs a day-of card names, in time order (missing runs skipped).
pub fn card_runs<'a>(ctx: &CardContext<'a>, run_ids: &[String]) -> Vec<&'a Run> {
    let mut runs: Vec<&Run> = run_ids
        .iter()
        .filter_map(|id| ctx.schedule.runs.iter().find(|run| &run.id == id))
        .collect();
    runs.sort_by_key(|run| run.datetime);
    runs
}

pub fn day_of_card(
    ctx: &CardContext<'_>,
    run_ids: &[String],
    heading: &str,
    mentioned: &[String],
) -> Card {
    let runs = card_runs(ctx, run_ids);
    let everyone = everyone_on(&runs);
    let who = People::new(ctx, &everyone, mentioned);
    let fields = runs
        .iter()
        .map(|run| {
            let when = if run.status == RunStatus::Otot {
                "🕒 own time".to_owned()
            } else {
                format!("🕘 {}", local_time(run.datetime, ctx.zone))
            };
            let mut lines = vec![boss_detail(&run.bosses, ctx.catalog), status_line(ctx, run)];
            let waiting = unanswered(&ctx.states(run));
            if !waiting.is_empty() {
                lines.push(format!("Still to answer: {}", who.list(&waiting)));
            }
            let value = lines.join("\n");
            CardField::wide(format!("{when}  ·  {}", format_bosses(&run.bosses)), value)
        })
        .collect();
    let lead: &[String] = runs.first().map_or(&[], |run| &run.bosses);
    Card::single(
        format!("📅 **{heading}**\n{}", who.list(&everyone)),
        CardEmbed {
            title: None,
            description: None,
            fields,
            footer: Some(REACT_HINT.to_owned()),
            colour: lead_colour(lead, ctx.catalog, COLOUR_DAY_OF),
            thumbnail: lead_portrait(lead, ctx.catalog),
            // The one card with the big picture (v4): read once, scrolled back
            // to; repeated or list cards stay small.
            image: lead_entry_art(lead, ctx.catalog),
            lead: lead.first().cloned(),
        },
    )
}
