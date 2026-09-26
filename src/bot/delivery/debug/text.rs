//! The plain-text test messages (v4 `formatting.amend_notice` for a move
//! from a day earlier, `formatting.decline_notice` as the invoker).

use chrono::TimeDelta;

use crate::bot::delivery::cards::{
    CardContext, People, REACT_HINT, format_bosses, local_day, local_time,
};
use crate::domain::ids::short_id;
use crate::domain::schedule::Run;

/// v4 `amend_notice(run, run.datetime - 1 day)`.
pub fn amend_text(ctx: &CardContext<'_>, run: &Run, mentioned: &[String]) -> String {
    let who = People::new(ctx, &run.participants, mentioned);
    let old = run.datetime - TimeDelta::days(1);
    format!(
        "🔁 **{}** moved: ~~{} {}~~ → **{} {}** — {}\n{REACT_HINT}",
        format_bosses(&run.bosses),
        local_day(old, ctx.zone),
        local_time(old, ctx.zone),
        local_day(run.datetime, ctx.zone),
        local_time(run.datetime, ctx.zone),
        who.list(&run.participants)
    )
}

/// v4 `decline_notice(run, invoker, display_name)`: the rest of the party,
/// then who can't make it.
pub fn decline_text(
    ctx: &CardContext<'_>,
    run: &Run,
    mentioned: &[String],
    decliner: &str,
    decliner_name: &str,
) -> String {
    let others: Vec<String> = run
        .participants
        .iter()
        .filter(|user| *user != decliner)
        .cloned()
        .collect();
    let tag = if others.is_empty() {
        String::new()
    } else {
        People::new(ctx, &others, mentioned).list(&others)
    };
    format!(
        "{tag} {decliner_name} can't make **{}** ({} {}) — reschedule? `/amend run_id:{} to:...`",
        format_bosses(&run.bosses),
        local_day(run.datetime, ctx.zone),
        local_time(run.datetime, ctx.zone),
        short_id(&run.id)
    )
    .trim()
    .to_owned()
}
