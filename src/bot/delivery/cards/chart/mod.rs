//! The estimate share chart ("Who carries") of a redesigned day-of or
//! countdown card: per run, one 100% bar per boss split between the party
//! members by their cached effective % (party order, numbered, never a 0%
//! part), as a PNG posted with the card and never re-rendered. Labels are
//! IGNs only, so the bundled Latin fonts always have the glyphs.

mod raster;
mod svg;
mod theme;

use crate::domain::chardata::PartyShares;

pub use raster::{RasterError, SCALE, png};
pub use svg::{Drawn, MAX_BARS, WIDTH, svg};

/// The attachment's file name.
pub const FILENAME: &str = "estimate.png";
/// Discord's cap on an attachment description.
const MAX_ALT: usize = 1024;

/// One run's block.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChartRun {
    /// `21:30`, or `own time`.
    pub time: String,
    pub party: PartyShares,
    /// The party's estimated minutes per boss, from a cached lookup of the
    /// whole party; `None` without one.
    pub minutes: Vec<Option<f64>>,
}

/// One card's image.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chart {
    pub runs: Vec<ChartRun>,
    /// `Tue 29 Sep 08:41`: the oldest reading drawn, else the post time.
    pub as_of: String,
    /// The public portal is open (the overflow line points there).
    pub portal: bool,
}

/// The attachment's description: the same facts by IGN.
pub fn alt(chart: &Chart) -> String {
    let runs: Vec<String> = chart
        .runs
        .iter()
        .map(|run| {
            let party = &run.party;
            let bosses: Vec<String> = party
                .bosses
                .iter()
                .enumerate()
                .map(|(boss, token)| match party.total(boss) {
                    None => format!("{token} no estimate yet"),
                    Some(total) => {
                        let shares = party.shares(boss);
                        let parts: Vec<String> = party
                            .carriers
                            .iter()
                            .zip(&shares)
                            .filter_map(|(carrier, share)| {
                                let crit = if carrier.crit_assumed {
                                    " (assumed crit)"
                                } else {
                                    ""
                                };
                                share.map(|share| format!("{} {share}%{crit}", carrier.ign))
                            })
                            .collect();
                        format!("{token} {}, {}", svg::percent(total), parts.join(", "))
                    }
                })
                .collect();
            let mut line = format!(
                "{} {}: {}",
                run.time,
                party.bosses.join(" + "),
                bosses.join("; ")
            );
            let estimated = (0..party.bosses.len()).any(|boss| party.total(boss).is_some());
            if estimated && party.no_estimate() > 0 {
                line.push_str(&format!("; {} with no estimate", party.no_estimate()));
            }
            line
        })
        .collect();
    let text = format!("{}. As of {}.", runs.join(". "), chart.as_of);
    match text.char_indices().nth(MAX_ALT - 1) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}
