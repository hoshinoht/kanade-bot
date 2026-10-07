//! v4 `digest_card`, v4-exact (no persona phrase): the guild's boss week
//! grouped by local day, unsettled runs marked. Names nobody and carries no
//! art (v4).

use chrono::{DateTime, Utc};

use super::common::{
    COLOUR_DIGEST, CardContext, answers_text, format_bosses, local_day, local_time,
};
use super::{Card, CardEmbed, CardField};
use crate::domain::ids::short_id;
use crate::domain::notify::DigestInclusion;
use crate::domain::schedule::{Run, RunStatus};

/// v4 digest footer.
pub const DIGEST_FOOTER: &str =
    "React \u{2705}/\u{274c} on your run's reminders · /schedule scope:mine for just yours";
/// v4's empty-week description.
pub const DIGEST_EMPTY: &str = "Nothing on the schedule yet. Add a baseline with `/fixed add`.";

/// v4 `DIGEST_STATUS_LABEL`.
fn digest_status(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Done => "🏁 **Cleared**",
        RunStatus::Confirmed => "✅ **Confirmed**",
        RunStatus::Planned => "⚠️ **Planned**",
        RunStatus::AtRisk => "❗ **At risk**",
        RunStatus::Otot => "🕒 **Own time**",
        RunStatus::Cancelled => "cancelled",
    }
}

/// v4 `digest_line`: one run, named rather than mentioned.
fn digest_line(ctx: &CardContext<'_>, run: &Run) -> String {
    let when = if run.status == RunStatus::Otot {
        "own time".to_owned()
    } else {
        local_time(run.datetime, ctx.zone)
    };
    let place = run
        .channel_id
        .as_deref()
        .map(|channel| format!(" · <#{channel}>"))
        .unwrap_or_default();
    format!(
        "**{}** · {}\n`{when}` · {}{place} · `#{}`",
        format_bosses(&run.bosses),
        digest_status(run.status),
        answers_text(ctx, run),
        short_id(&run.id)
    )
}

pub fn digest_card(
    ctx: &CardContext<'_>,
    week_start: DateTime<Utc>,
    inclusion: &DigestInclusion,
) -> Card {
    let title = format!("🗓️ Boss week of {}", local_day(week_start, ctx.zone));
    let fields: Vec<CardField> = inclusion
        .days
        .iter()
        .filter_map(|day| {
            let runs: Vec<&Run> = day.run_ids.iter().filter_map(|id| ctx.run(id)).collect();
            let first = runs.first()?;
            let lines: Vec<String> = runs.iter().map(|run| digest_line(ctx, run)).collect();
            Some(CardField::wide(
                local_day(first.datetime, ctx.zone),
                lines.join("\n\n"),
            ))
        })
        .collect();
    if inclusion.live == 0 {
        return Card::single(
            title,
            CardEmbed {
                description: Some(DIGEST_EMPTY.to_owned()),
                colour: COLOUR_DIGEST,
                ..CardEmbed::default()
            },
        );
    }
    let mut summary = format!(
        "**{}/{} Cleared** · {} run(s) across {} day(s)",
        inclusion.cleared,
        inclusion.live,
        inclusion.live,
        fields.len()
    );
    if inclusion.unsettled > 0 {
        summary.push_str(&format!(
            " · **{}** still unconfirmed ⚠️",
            inclusion.unsettled
        ));
    }
    if inclusion.at_risk > 0 {
        summary.push_str(&format!(" · **{}** at risk ❗", inclusion.at_risk));
    }
    Card::single(
        title,
        CardEmbed {
            description: Some(summary),
            fields,
            footer: Some(DIGEST_FOOTER.to_owned()),
            colour: COLOUR_DIGEST,
            ..CardEmbed::default()
        },
    )
}
