//! The redesigned weekly digest: the phrase in the content; one ink-blue
//! embed titled with the boss week's first and last day, a cleared bar, one
//! field per day with a line per run (state, guild-zone time, bosses,
//! headcount) and the channel only when the week spans several.
//!
//! With the card kit's V2 parts in the context it also carries the same
//! week as a Components V2 layout (the digest pings nobody): one ink-blue
//! container with the header (title, phrase, progress; the bot's avatar
//! beside it when it has an http(s) URL), a text display per day and a
//! "My runs" / "Open portal" row ("Open portal" only while the public
//! portal is open). A layout over Discord's budget is left off (logged once
//! per week) and the embed is sent instead.

use std::collections::BTreeSet;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::json;
use twilight_model::channel::message::Component;
use twilight_model::channel::message::component::ButtonStyle;

use super::super::common::{CardContext, local_day, local_time};
use super::super::digest::DIGEST_EMPTY;
use super::super::heading::DIGEST_PHRASE_SEED;
use super::super::{Card, CardEmbed, CardField};
use super::limits::{MAX_FIELD_VALUE, clip_lines};
use super::live::V2Kit;
use super::v2::{
    DIGEST_MINE, action_row, button, container, link_button, section_with_thumbnail, separator,
    text, within_budget,
};
use super::vocab::{INK_BLUE, boss_labels};
use crate::domain::attendance::Tally;
use crate::domain::notify::DigestInclusion;
use crate::domain::schedule::{Run, RunStatus};
use crate::runtime::logging;

pub const DIGEST_FOOTER: &str = "Edited live · /schedule scope:mine for just your runs";
/// The V2 digest's buttons.
pub const MY_RUNS: &str = "📋 My runs";
pub const OPEN_PORTAL: &str = "Open portal";

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

/// The V2 layout of a digest: header, the days, the button row.
fn digest_layout(kit: &V2Kit, header: Vec<String>, days: Vec<String>) -> Vec<Component> {
    let mut children = vec![match kit.avatar_url() {
        Some(url) => section_with_thumbnail(header, &url),
        None => text(header.join("\n")),
    }];
    children.push(separator());
    children.extend(days.into_iter().map(text));
    children.push(separator());
    let mut buttons = vec![button(
        ButtonStyle::Primary,
        MY_RUNS,
        DIGEST_MINE.to_owned(),
        false,
    )];
    if let Some(portal) = kit.portal_url() {
        buttons.push(link_button(OPEN_PORTAL, portal));
    }
    children.push(action_row(buttons));
    vec![container(INK_BLUE, children)]
}

/// The layout when the context asks for V2 and it fits; else nothing.
fn v2_or_nothing(
    ctx: &CardContext<'_>,
    week_start: DateTime<Utc>,
    header: Vec<String>,
    days: Vec<String>,
) -> Vec<Component> {
    let Some(kit) = ctx.v2 else {
        return Vec::new();
    };
    let layout = digest_layout(kit, header, days);
    if within_budget(&layout) {
        return layout;
    }
    if kit
        .formats
        .first_note(&format!("digest-budget:{}", week_start.timestamp()))
    {
        logging::event(
            "WARN",
            "digest_v2_over_budget",
            json!({"week_start": week_start.timestamp(), "fallback": "embed"}),
        );
    }
    Vec::new()
}

pub fn digest_card(
    ctx: &CardContext<'_>,
    week_start: DateTime<Utc>,
    inclusion: &DigestInclusion,
    phrase: Option<&str>,
) -> Card {
    let phrase = phrase.unwrap_or(DIGEST_PHRASE_SEED);
    let content = format!("🗓️ **{phrase}**");
    let last_day = week_start + TimeDelta::days(7) - TimeDelta::seconds(1);
    let title = format!(
        "Boss week · {} → {}",
        local_day(week_start, ctx.zone),
        local_day(last_day, ctx.zone)
    );
    if inclusion.live == 0 {
        let components = v2_or_nothing(
            ctx,
            week_start,
            vec![format!("## {title}\n{phrase}")],
            vec![DIGEST_EMPTY.to_owned()],
        );
        return Card {
            components,
            ..Card::single(
                content,
                CardEmbed {
                    title: Some(title),
                    description: Some(DIGEST_EMPTY.to_owned()),
                    colour: INK_BLUE,
                    ..CardEmbed::default()
                },
            )
        };
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
    let day_lines: Vec<(String, Vec<String>)> = days
        .iter()
        .filter_map(|runs| {
            let first = runs.first()?;
            let lines: Vec<String> = runs.iter().map(|run| line(ctx, run, channels)).collect();
            Some((local_day(first.datetime, ctx.zone), lines))
        })
        .collect();
    let fields = day_lines
        .iter()
        .map(|(day, lines)| CardField::wide(day.clone(), clip_lines(lines, MAX_FIELD_VALUE)))
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
    let components = v2_or_nothing(
        ctx,
        week_start,
        vec![format!("## {title}\n{phrase}\n{description}")],
        day_lines
            .iter()
            .map(|(day, lines)| format!("### {day}\n{}", lines.join("\n")))
            .collect(),
    );
    Card {
        components,
        ..Card::single(
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
