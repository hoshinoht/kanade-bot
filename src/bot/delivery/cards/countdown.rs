//! v4 `countdown_card`, v4-exact (no persona phrase): names the party bar
//! whoever already declined. Pending (someone still to answer), someone out,
//! or all set; only the last is green, and the react hint shows only while
//! answers are pending.

use super::art::lead_portrait;
use super::common::{
    COLOUR_ALL_SET, COLOUR_COUNTDOWN, CardContext, People, REACT_HINT, boss_detail, declined,
    format_bosses, format_offset, lead_colour, local_time, not_declined, status_line, unanswered,
};
use super::{Card, CardEmbed};
use crate::domain::schedule::Run;

pub fn countdown_card(
    ctx: &CardContext<'_>,
    run: &Run,
    minutes: i64,
    mentioned: &[String],
) -> Card {
    let states = ctx.states(run);
    let pending = unanswered(&states);
    let out = declined(&states);
    let still_on = not_declined(&states);
    let who = People::new(ctx, &run.participants, mentioned);
    let waiting = if pending.is_empty() && out.is_empty() {
        "everyone's confirmed ✅".to_owned()
    } else {
        let mut parts = Vec::new();
        if !still_on.is_empty() {
            parts.push(who.list(&still_on));
        }
        if !out.is_empty() {
            parts.push(format!("{} out", who.list(&out)));
        }
        if parts.is_empty() {
            "(nobody)".to_owned()
        } else {
            parts.join(" · ")
        }
    };
    let content = format!(
        "⏰ **{}** in {} ({}) — {waiting}",
        format_bosses(&run.bosses),
        format_offset(minutes),
        local_time(run.datetime, ctx.zone)
    );
    let mut detail = vec![boss_detail(&run.bosses, ctx.catalog), status_line(ctx, run)];
    if !pending.is_empty() {
        detail.push(format!("Still to answer: {}", who.list(&pending)));
    }
    let settled = pending.is_empty() && out.is_empty();
    Card::single(
        content,
        CardEmbed {
            title: None,
            description: Some(detail.join("\n")),
            fields: Vec::new(),
            footer: (!pending.is_empty()).then(|| REACT_HINT.to_owned()),
            colour: lead_colour(
                &run.bosses,
                ctx.catalog,
                if settled {
                    COLOUR_ALL_SET
                } else {
                    COLOUR_COUNTDOWN
                },
            ),
            thumbnail: lead_portrait(&run.bosses, ctx.catalog),
            image: None,
            lead: run.bosses.first().cloned(),
        },
    )
}
