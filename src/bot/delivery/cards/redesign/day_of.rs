//! The redesigned day-of card: the heading and who is pinged in the
//! content, then one embed per run in its lead boss's colour with its own
//! portrait, In / Waiting / Out as inline fields and the run id in the
//! footer. Only the first run carries the entry art. Runs past Discord's
//! limits share one last embed as compact lines.

use super::super::art::{lead_entry_art, lead_portrait};
use super::super::common::{CardContext, People, everyone_on, lead_colour};
use super::super::day_of::card_runs;
use super::super::{Card, CardEmbed, CardField};
use super::limits::{
    MAX_DESCRIPTION, MAX_EMBEDS, MAX_FIELD_VALUE, MAX_TITLE, MAX_TOTAL_CHARS, clip, clip_lines,
    embed_chars,
};
use super::vocab::{INK_BLUE, boss_labels, names, relative_time, roster, short_time, subtext};
use crate::domain::attendance::{AttendanceMode, Tally, status_label};
use crate::domain::ids::short_id;
use crate::domain::schedule::{Run, RunStatus};

const REACT: &str = "react ✅ in / ❌ out";

pub fn day_of_card(
    ctx: &CardContext<'_>,
    run_ids: &[String],
    heading: &str,
    mentioned: &[String],
) -> Card {
    let runs = card_runs(ctx, run_ids);
    let everyone = everyone_on(&runs);
    let who = People::new(ctx, &everyone, mentioned);
    let pinged: Vec<String> = everyone
        .iter()
        .filter(|user| !ctx.quiet && mentioned.contains(user))
        .cloned()
        .collect();
    let line = match (pinged.is_empty(), ctx.attendance.mode) {
        (true, _) => REACT.to_owned(),
        (false, AttendanceMode::V5) => format!("Waiting on {} · {REACT}", who.list(&pinged)),
        (false, AttendanceMode::V4Compat) => format!("{} · {REACT}", who.list(&pinged)),
    };
    let full: Vec<CardEmbed> = runs
        .iter()
        .enumerate()
        .map(|(index, run)| run_embed(ctx, &who, run, index == 0))
        .collect();
    Card {
        content: format!("📅 **{heading}**\n{}", subtext(&line)),
        embeds: fit(ctx, &runs, full),
        components: Vec::new(),
    }
}

/// `✅ all in`, `⚠️ waiting on 2`, …, then the v5 status label if any.
fn state(ctx: &CardContext<'_>, run: &Run) -> String {
    let states = ctx.states(run);
    let tally = Tally::of(states.iter().map(|(_, state)| state));
    let mut text = match run.status {
        RunStatus::AtRisk => "❗ at risk".to_owned(),
        RunStatus::Done => "🏁 done".to_owned(),
        RunStatus::Cancelled => "🚫 cancelled".to_owned(),
        _ if tally.unknown > 0 => format!("⚠️ waiting on {}", tally.unknown),
        _ if tally.declined == 0 => "✅ all in".to_owned(),
        _ => format!(
            "✅ {} in · {} out",
            tally.confirmed + tally.assumed,
            tally.declined
        ),
    };
    if let Some(label) = status_label(run.status, run.status_pin, &tally, ctx.attendance.mode) {
        text.push_str(&format!(" · {label}"));
    }
    text
}

fn when(run: &Run) -> String {
    if run.status == RunStatus::Otot {
        "🕒 own time".to_owned()
    } else {
        format!(
            "🕘 **{}** · {}",
            short_time(run.datetime),
            relative_time(run.datetime)
        )
    }
}

/// `#a1b2c3d4 · Lv280 + Lv270`; unknown levels are left out.
fn footer(ctx: &CardContext<'_>, run: &Run) -> String {
    let levels: Vec<String> = run
        .bosses
        .iter()
        .filter_map(|token| {
            let (_, boss) = ctx.catalog?.split(token)?;
            boss.level().map(|level| format!("Lv{level}"))
        })
        .collect();
    let id = format!("#{}", short_id(&run.id));
    if levels.is_empty() {
        id
    } else {
        format!("{id} · {}", levels.join(" + "))
    }
}

fn run_embed(ctx: &CardContext<'_>, who: &People, run: &Run, first: bool) -> CardEmbed {
    let split = roster(who, &ctx.states(run));
    let field =
        |name: &str, list: &[String]| CardField::inline(name, clip(&names(list), MAX_FIELD_VALUE));
    CardEmbed {
        title: Some(clip(
            &boss_labels(&run.bosses, ctx.catalog, ctx.marks),
            MAX_TITLE,
        )),
        description: Some(format!("{} · {}", when(run), state(ctx, run))),
        fields: vec![
            field("In ✅", &split.in_),
            field("Waiting", &split.waiting),
            field("Out ❌", &split.out),
        ],
        footer: Some(footer(ctx, run)),
        colour: lead_colour(&run.bosses, ctx.catalog, INK_BLUE),
        thumbnail: lead_portrait(&run.bosses, ctx.catalog),
        image: if first {
            lead_entry_art(&run.bosses, ctx.catalog)
        } else {
            None
        },
        lead: run.bosses.first().cloned(),
    }
}

/// One compact line per run, for runs past the limits.
fn compact(ctx: &CardContext<'_>, run: &Run) -> String {
    format!(
        "{} · {} · {}",
        when(run),
        boss_labels(&run.bosses, ctx.catalog, ctx.marks),
        state(ctx, run)
    )
}

/// The full embeds when they fit; else as many as fit (at most nine) and
/// one last embed listing the rest.
fn fit(ctx: &CardContext<'_>, runs: &[&Run], full: Vec<CardEmbed>) -> Vec<CardEmbed> {
    let total = |embeds: &[CardEmbed]| embeds.iter().map(embed_chars).sum::<usize>();
    if full.len() <= MAX_EMBEDS && total(&full) <= MAX_TOTAL_CHARS {
        return full;
    }
    let overflow = |rest: &[&Run]| {
        let lines: Vec<String> = rest.iter().map(|run| compact(ctx, run)).collect();
        CardEmbed {
            title: Some(format!("{} more runs", rest.len())),
            description: Some(clip_lines(&lines, MAX_DESCRIPTION)),
            colour: INK_BLUE,
            ..CardEmbed::default()
        }
    };
    for kept in (0..MAX_EMBEDS.min(full.len())).rev() {
        let last = overflow(&runs[kept..]);
        if total(&full[..kept]) + embed_chars(&last) <= MAX_TOTAL_CHARS {
            let mut embeds = full[..kept].to_vec();
            embeds.push(last);
            return embeds;
        }
    }
    vec![overflow(runs)]
}
