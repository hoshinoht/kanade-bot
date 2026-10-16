//! The chart as an SVG string, laid out as the approved board's PNG
//! (`_src/chart.mjs`, `pngCard`): 560 px wide, one block per run (its time
//! and bosses, one 100% bar per boss of that boss's own party, the numbered
//! IGN legend, by bar when a member brings another character to some boss,
//! the no-estimate count), at most [`MAX_BARS`] bars then "+n more", and
//! the "as of" footer. Pure and deterministic: the same chart gives the
//! same string.

use std::fmt::Write;

use super::theme::{COUNTED, DIM, GROUND, INK, LINE, RULE, SPOKEN, STRONG, who};
use super::{Chart, ChartRun};

/// The drawing's width in CSS px (rendered at [`super::SCALE`]).
pub const WIDTH: u32 = 560;
/// Bars drawn across the card's runs; the rest are counted.
pub const MAX_BARS: usize = 8;

const PAD: f64 = 16.0;
/// The boss label column left of the bars.
const LABEL: f64 = 92.0;
const BAR_HEIGHT: f64 = 28.0;

/// An SVG document and its size in CSS px.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drawn {
    pub width: u32,
    pub height: u32,
    pub svg: String,
}

#[derive(Clone, Copy)]
struct Style {
    anchor: Option<&'static str>,
    mono: bool,
    size: f64,
    bold: bool,
    fill: &'static str,
}

impl Style {
    fn spoken(size: f64, fill: &'static str) -> Self {
        Self {
            anchor: None,
            mono: false,
            size,
            bold: false,
            fill,
        }
    }

    fn counted(size: f64, fill: &'static str) -> Self {
        Self {
            mono: true,
            ..Self::spoken(size, fill)
        }
    }

    fn bold(self) -> Self {
        Self { bold: true, ..self }
    }

    fn anchored(self, anchor: &'static str) -> Self {
        Self {
            anchor: Some(anchor),
            ..self
        }
    }
}

fn text(out: &mut String, x: f64, y: f64, body: &str, style: Style) {
    let _ = write!(out, r#"<text x="{x}" y="{y}""#);
    if let Some(anchor) = style.anchor {
        let _ = write!(out, r#" text-anchor="{anchor}""#);
    }
    let _ = write!(
        out,
        r#" font-family="{}" font-size="{}" font-weight="{}" fill="{}">{}</text>"#,
        if style.mono { COUNTED } else { SPOKEN },
        style.size,
        if style.bold { 700 } else { 400 },
        style.fill,
        escape(body),
    );
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;")
}

fn rule(out: &mut String, y: f64) {
    let _ = write!(
        out,
        r#"<line x1="{PAD}" y1="{y}" x2="{}" y2="{y}" stroke="{RULE}" stroke-width="1"/>"#,
        f64::from(WIDTH) - PAD
    );
}

/// `140.5%`.
pub(super) fn percent(value: f64) -> String {
    format!("{value:.1}%")
}

/// The hatch over an assumed-crit member's part and swatch.
fn hatch_id(run: usize, member: usize) -> String {
    format!("hP{run}_{member}")
}

/// One 100% bar of boss `boss`'s shares, parts in party order, each
/// labelled with its number (and share while it fits). A member without a
/// figure draws no part; a boss nobody has one for says so instead.
fn bar(out: &mut String, run: &ChartRun, index: usize, boss: usize, x0: f64, y: f64, width: f64) {
    let party = &run.party;
    let Some(total) = party.total(boss).filter(|total| *total > 0.0) else {
        let _ = write!(
            out,
            r#"<rect x="{}" y="{}" width="{}" height="26.5" rx="7" fill="none" stroke="{LINE}" stroke-width="1.5" stroke-dasharray="5 4"/>"#,
            x0 + 0.75,
            y + 0.75,
            width - 1.5,
        );
        text(
            out,
            x0 + width / 2.0,
            y + 18.5,
            "no estimate yet",
            Style::spoken(13.5, DIM).anchored("middle"),
        );
        return;
    };
    let shares = party.shares(boss);
    let token = &party.bosses[boss];
    let _ = write!(
        out,
        r#"<clipPath id="cr{index}{token}"><rect x="{x0}" y="{y}" width="{width}" height="{BAR_HEIGHT}" rx="7"/></clipPath><g clip-path="url(#cr{index}{token})">"#,
    );
    let parts: Vec<(usize, f64, u32)> = party
        .carriers
        .iter()
        .enumerate()
        .filter_map(|(member, carrier)| {
            let value = carrier.effective.get(boss).copied().flatten()?;
            Some((member, value, shares[member]?))
        })
        .collect();
    let mut acc = 0.0;
    for (at, &(member, value, share)) in parts.iter().enumerate() {
        let span = value / total * width;
        let (first, last) = (at == 0, at + 1 == parts.len());
        // Parts sit 2 px apart; the bar's own ends stay flush.
        let x = x0 + acc + if first { 0.0 } else { 1.0 };
        let w = span - if first { 0.0 } else { 1.0 } - if last { 0.0 } else { 1.0 };
        let (fill, ink) = who(member);
        let _ = write!(
            out,
            r#"<rect x="{x}" y="{y}" width="{w}" height="{BAR_HEIGHT}" fill="{fill}"/>"#
        );
        let crit = party.carriers[member].crit_at(boss);
        if crit {
            let _ = write!(
                out,
                r#"<rect x="{x}" y="{y}" width="{w}" height="{BAR_HEIGHT}" fill="url(#{})"/>"#,
                hatch_id(index, member)
            );
        }
        let label = format!("{}·{share}%{}", member + 1, if crit { "*" } else { "" });
        let style = Style::counted(11.5, ink).bold().anchored("middle");
        // The board's fit rule: the full label, else the number alone.
        if w >= label.chars().count() as f64 * 7.2 + 6.0 {
            text(out, x + w / 2.0, y + 18.5, &label, style);
        } else if w >= 12.0 {
            text(out, x + w / 2.0, y + 18.5, &(member + 1).to_string(), style);
        }
        acc += span;
    }
    out.push_str("</g>");
}

/// One legend entry: the member's place, the IGN shown and whether its
/// critical rate was assumed.
type Entry<'a> = (usize, &'a str, bool);

/// A grid of numbered swatches, three to a row, each with its IGN (and with
/// `shares` its share of the one boss).
fn swatches(
    out: &mut String,
    index: usize,
    y: &mut f64,
    shown: &[Entry<'_>],
    shares: Option<&[Option<u32>]>,
) {
    let column = (f64::from(WIDTH) - 2.0 * PAD) / 3.0;
    for (slot, &(member, ign, crit)) in shown.iter().enumerate() {
        let x = PAD + (slot % 3) as f64 * column;
        let top = *y + (slot / 3) as f64 * 21.0;
        let (fill, ink) = who(member);
        let _ = write!(
            out,
            r#"<rect x="{x}" y="{top}" width="16" height="16" rx="4" fill="{fill}"/>"#
        );
        if crit {
            let _ = write!(
                out,
                r#"<rect x="{x}" y="{top}" width="16" height="16" rx="4" fill="url(#{})"/>"#,
                hatch_id(index, member)
            );
        }
        text(
            out,
            x + 8.0,
            top + 12.0,
            &(member + 1).to_string(),
            Style::counted(11.0, ink).bold().anchored("middle"),
        );
        let star = if crit { "*" } else { "" };
        text(
            out,
            x + 22.0,
            top + 12.5,
            &format!("{ign}{star}"),
            Style::spoken(13.5, STRONG).bold(),
        );
        if let Some(share) = shares.and_then(|shares| shares[member]) {
            text(
                out,
                x + column - 14.0,
                top + 12.5,
                &format!("{share}%"),
                Style::counted(11.5, INK).bold().anchored("end"),
            );
        }
    }
    *y += shown.len().div_ceil(3) as f64 * 21.0 + 2.0;
}

/// The legend: a numbered swatch and IGN for each member with a figure;
/// with one boss, their share too. When a member brings another character
/// to some boss, one such list per bar, each under its boss and naming
/// that bar's characters. Then how many have no estimate.
fn legend(out: &mut String, run: &ChartRun, index: usize, y: &mut f64) {
    let party = &run.party;
    if party.per_boss() {
        let mut any = false;
        for (boss, token) in party.bosses.iter().enumerate() {
            let shown: Vec<Entry<'_>> = party
                .carriers
                .iter()
                .enumerate()
                .filter(|(_, carrier)| {
                    carrier
                        .effective
                        .get(boss)
                        .copied()
                        .flatten()
                        .is_some_and(|value| value.is_finite() && value >= 0.0)
                })
                .map(|(member, carrier)| (member, carrier.ign_at(boss), carrier.crit_at(boss)))
                .collect();
            if shown.is_empty() {
                continue;
            }
            any = true;
            text(out, PAD, *y + 11.0, token, Style::counted(11.0, DIM).bold());
            *y += 16.0;
            swatches(out, index, y, &shown, None);
        }
        if !any {
            return;
        }
    } else {
        let shown: Vec<Entry<'_>> = party
            .carriers
            .iter()
            .enumerate()
            .filter(|(_, carrier)| carrier.estimated())
            .map(|(member, carrier)| (member, carrier.ign.as_str(), carrier.crit_assumed))
            .collect();
        if shown.is_empty() {
            return;
        }
        let shares = (party.bosses.len() == 1).then(|| party.shares(0));
        swatches(out, index, y, &shown, shares.as_deref());
    }
    let none = party.no_estimate();
    if none > 0 {
        let _ = write!(
            out,
            r#"<rect x="{}" y="{}" width="14.5" height="14.5" rx="4" fill="none" stroke="{DIM}" stroke-width="1.5"/>"#,
            PAD + 0.75,
            *y + 0.75,
        );
        text(
            out,
            PAD + 22.0,
            *y + 12.0,
            &format!("{none} with no estimate (not 0%, not in the total)"),
            Style::spoken(12.5, DIM),
        );
        *y += 20.0;
    }
}

/// The hatch patterns of a run's members with an assumed critical rate
/// behind a figure.
fn hatches(defs: &mut String, run: &ChartRun, index: usize) {
    for (member, carrier) in run.party.carriers.iter().enumerate() {
        if carrier.crit_assumed || carrier.crit_shown() {
            let _ = write!(
                defs,
                r#"<pattern id="{}" width="6" height="6" patternUnits="userSpaceOnUse" patternTransform="rotate(45)"><rect width="2.2" height="6" fill="{}" fill-opacity="0.34"/></pattern>"#,
                hatch_id(index, member),
                who(member).1,
            );
        }
    }
}

/// The whole image.
pub fn svg(chart: &Chart) -> Drawn {
    let width = f64::from(WIDTH);
    let bar_width = width - 2.0 * PAD - LABEL;
    let x0 = PAD + LABEL;
    let (mut y, mut body, mut defs) = (PAD, String::new(), String::new());
    let (mut bars, mut hidden) = (0, 0);
    for (index, run) in chart.runs.iter().enumerate() {
        let party = &run.party;
        if bars >= MAX_BARS {
            hidden += party.bosses.len();
            continue;
        }
        hatches(&mut defs, run, index);
        if index > 0 {
            rule(&mut body, y - 6.0);
            y += 4.0;
        }
        text(
            &mut body,
            PAD,
            y + 12.0,
            &format!("{} · {}", run.time, party.bosses.join(" + ")),
            Style::counted(12.5, STRONG).bold(),
        );
        y += 22.0;
        for (boss, token) in party.bosses.iter().enumerate() {
            if bars >= MAX_BARS {
                hidden += 1;
                continue;
            }
            text(
                &mut body,
                PAD,
                y + 12.0,
                token,
                Style::counted(11.5, STRONG).bold(),
            );
            let figure = match (party.total(boss), run.minutes.get(boss).copied().flatten()) {
                (Some(total), Some(minutes)) => format!("{} · {minutes:.1}m", percent(total)),
                (Some(total), None) => percent(total),
                (None, _) => "—".to_owned(),
            };
            text(&mut body, PAD, y + 25.0, &figure, Style::counted(10.5, DIM));
            bar(&mut body, run, index, boss, x0, y, bar_width);
            y += 36.0;
            bars += 1;
        }
        legend(&mut body, run, index, &mut y);
        y += 12.0;
    }
    if hidden > 0 {
        let noun = if hidden == 1 { "boss" } else { "bosses" };
        let place = if chart.portal { " in the portal" } else { "" };
        text(
            &mut body,
            PAD,
            y + 4.0,
            &format!("+{hidden} more {noun}{place}"),
            Style::spoken(12.5, INK).bold(),
        );
        y += 20.0;
    }
    let crit = chart.runs.iter().any(|run| {
        run.party
            .carriers
            .iter()
            .any(|carrier| carrier.crit_shown())
    });
    rule(&mut body, y - 4.0);
    let note = if crit {
        " · * assumes 100% critical rate"
    } else {
        ""
    };
    text(
        &mut body,
        PAD,
        y + 12.0,
        &format!("Clear-time estimate · as of {}{note}", super::as_of(chart)),
        Style::spoken(12.0, DIM),
    );
    y += 18.0;
    // Whole px, as the board's height: `y` only ever grows in halves.
    let height = (y + PAD - 4.0).ceil() as u32;
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {height}" width="{WIDTH}" height="{height}"><defs>{defs}</defs><rect width="{WIDTH}" height="{height}" fill="{GROUND}"/>{body}</svg>"#
    );
    Drawn {
        width: WIDTH,
        height,
        svg,
    }
}
