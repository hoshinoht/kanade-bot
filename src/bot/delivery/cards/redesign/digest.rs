//! The redesigned weekly digest: the phrase in the content; one ink-blue
//! embed titled with the boss week's first and last day, a cleared bar, one
//! field per day with a line per run (state, guild-zone time, bosses,
//! headcount) and the channel only when the week spans several.

use std::collections::BTreeSet;

use chrono::{DateTime, TimeDelta, Utc};

use super::super::common::{CardContext, local_day, local_time};
use super::super::digest::DIGEST_EMPTY;
use super::super::heading::DIGEST_PHRASE_SEED;
use super::super::{Card, CardEmbed, CardField};
use super::limits::{MAX_FIELD_VALUE, clip_lines};
use super::vocab::{INK_BLUE, boss_labels};
use crate::domain::attendance::Tally;
use crate::domain::notify::DigestInclusion;
use crate::domain::schedule::{Run, RunStatus};

pub const DIGEST_FOOTER: &str = "Edited live · /schedule scope:mine for just your runs";

const BAR: usize = 5;

/// `▰▰▱▱▱`: `cleared` of `live`, rounded to the nearest segment.
fn bar(cleared: usize, live: usize) -> String {
    let filled = if live == 0 {
        0
    } else {
        ((cleared * BAR * 2 + live) / (live * 2)).min(BAR)
    };
    "▰".repeat(filled) + &"▱".repeat(BAR - filled)
}

fn emoji(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Done => "🏁",
        RunStatus::Confirmed => "✅",
        RunStatus::Planned => "⚠️",
        RunStatus::AtRisk => "❗",
        RunStatus::Otot => "🕒",
        RunStatus::Cancelled => "🚫",
    }
}

/// `2 in, 1 out, 1 waiting` (out and waiting only when non-zero).
fn headcount(ctx: &CardContext<'_>, run: &Run) -> String {
    let states = ctx.states(run);
    let tally = Tally::of(states.iter().map(|(_, state)| state));
    let mut parts = vec![format!("{} in", tally.confirmed + tally.assumed)];
    if tally.declined > 0 {
        parts.push(format!("{} out", tally.declined));
    }
    if tally.unknown > 0 {
        parts.push(format!("{} waiting", tally.unknown));
    }
    parts.join(", ")
}

fn line(ctx: &CardContext<'_>, run: &Run, channels: bool) -> String {
    let when = if run.status == RunStatus::Otot {
        "own time".to_owned()
    } else {
        format!("`{}`", local_time(run.datetime, ctx.zone))
    };
    let place = match (&run.channel_id, channels) {
        (Some(channel), true) => format!(" · <#{channel}>"),
        _ => String::new(),
    };
    format!(
        "{} {when}  {} · {}{place}",
        emoji(run.status),
        boss_labels(&run.bosses, ctx.catalog, ctx.marks),
        headcount(ctx, run)
    )
}

pub fn digest_card(
    ctx: &CardContext<'_>,
    week_start: DateTime<Utc>,
    inclusion: &DigestInclusion,
    phrase: Option<&str>,
) -> Card {
    let content = format!("🗓️ **{}**", phrase.unwrap_or(DIGEST_PHRASE_SEED));
    let last_day = week_start + TimeDelta::days(7) - TimeDelta::seconds(1);
    let title = format!(
        "Boss week · {} → {}",
        local_day(week_start, ctx.zone),
        local_day(last_day, ctx.zone)
    );
    if inclusion.live == 0 {
        return Card::single(
            content,
            CardEmbed {
                title: Some(title),
                description: Some(DIGEST_EMPTY.to_owned()),
                colour: INK_BLUE,
                ..CardEmbed::default()
            },
        );
    }
    let days: Vec<Vec<&Run>> = inclusion
        .days
        .iter()
        .map(|day| day.run_ids.iter().filter_map(|id| ctx.run(id)).collect())
        .collect();
    let channels = days
        .iter()
        .flatten()
        .filter_map(|run| run.channel_id.as_deref())
        .collect::<BTreeSet<_>>()
        .len()
        > 1;
    let fields = days
        .iter()
        .filter_map(|runs| {
            let first = runs.first()?;
            let lines: Vec<String> = runs.iter().map(|run| line(ctx, run, channels)).collect();
            Some(CardField::wide(
                local_day(first.datetime, ctx.zone),
                clip_lines(&lines, MAX_FIELD_VALUE),
            ))
        })
        .collect();
    let mut description = format!(
        "**{}  {} of {} cleared**",
        bar(inclusion.cleared, inclusion.live),
        inclusion.cleared,
        inclusion.live
    );
    // `unsettled` counts at-risk runs too; each shows under one heading.
    let waiting = inclusion.unsettled.saturating_sub(inclusion.at_risk);
    let mut notes = Vec::new();
    if waiting > 0 {
        notes.push(format!("⚠️ {waiting} waiting on answers"));
    }
    if inclusion.at_risk > 0 {
        notes.push(format!("❗ {} at risk", inclusion.at_risk));
    }
    if !notes.is_empty() {
        description.push('\n');
        description.push_str(&notes.join(" · "));
    }
    Card::single(
        content,
        CardEmbed {
            title: Some(title),
            description: Some(description),
            fields,
            footer: Some(DIGEST_FOOTER.to_owned()),
            colour: INK_BLUE,
            ..CardEmbed::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::bar;

    #[test]
    fn the_bar_rounds_to_the_nearest_segment() {
        assert_eq!(bar(2, 5), "▰▰▱▱▱");
        assert_eq!(bar(0, 5), "▱▱▱▱▱");
        assert_eq!(bar(5, 5), "▰▰▰▰▰");
        assert_eq!(bar(1, 4), "▰▱▱▱▱");
        assert_eq!(bar(1, 3), "▰▰▱▱▱");
        assert_eq!(bar(0, 0), "▱▱▱▱▱");
    }
}
