//! The canonical `get_schedule` listing as records, so grounding can show
//! only the runs a reply named and shaping can drop runs that already
//! happened (named v5 difference `D-GROUND-FILTERED`).

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::{pattern, pattern_i};
use crate::domain::pytext::strip;

static RUN_ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"\[([0-9a-fA-F]{8})\]"));
static OMITTED: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"^\s*\*?\(and (\d+) more\)\*?\s*$"));
static FOOTER: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"^\s*\*?Every run listed has already happened"));
const PAST: &str = "*already happened*";

#[derive(Clone)]
struct Record {
    id: String,
    text: String,
    past: bool,
}

/// A listing split into its heading, records, omission count and footer.
#[derive(Clone)]
struct Listing {
    heading: Option<String>,
    records: Vec<Record>,
    omitted: usize,
    footer: Option<String>,
}

impl Listing {
    /// `None` for a listing with parts this split does not recognise.
    fn parse(schedule: &str) -> Option<Self> {
        let mut listing = Listing {
            heading: None,
            records: Vec::new(),
            omitted: 0,
            footer: None,
        };
        let paragraphs = schedule
            .split("\n\n")
            .filter(|part| !strip(part).is_empty());
        for (index, paragraph) in paragraphs.enumerate() {
            if let Some(found) = RUN_ID.captures(paragraph) {
                listing.records.push(Record {
                    id: found[1].to_lowercase(),
                    text: paragraph.to_owned(),
                    past: paragraph.contains(PAST),
                });
            } else if let Some(found) = OMITTED.captures(paragraph) {
                listing.omitted += found[1].parse::<usize>().ok()?;
            } else if FOOTER.is_match(paragraph) {
                listing.footer = Some(paragraph.to_owned());
            } else if index == 0 {
                listing.heading = Some(paragraph.to_owned());
            } else {
                return None;
            }
        }
        Some(listing)
    }

    fn upcoming(&self) -> Option<Vec<&Record>> {
        let upcoming: Vec<&Record> = self.records.iter().filter(|r| !r.past).collect();
        (!upcoming.is_empty() && upcoming.len() < self.records.len()).then_some(upcoming)
    }

    fn render(&self, kept: &[&Record]) -> String {
        let omitted = self.omitted + self.records.len() - kept.len();
        let mut parts: Vec<String> = self.heading.iter().cloned().collect();
        parts.extend(kept.iter().map(|record| record.text.clone()));
        if omitted > 0 {
            parts.push(format!("*(and {omitted} more)*"));
        }
        parts.extend(self.footer.iter().cloned());
        parts.join("\n\n")
    }
}

/// The schedule text grounding put into a reply.
pub(super) struct Block {
    pub text: String,
    listing: Option<Listing>,
}

impl Block {
    /// The tool's listing verbatim, as v4 inserted it.
    pub fn full(schedule: &str) -> Self {
        Block {
            text: schedule.to_owned(),
            listing: Listing::parse(schedule),
        }
    }

    /// Only the records for `ids`, in the tool's order; `None` when the
    /// listing cannot be split.
    pub fn only(schedule: &str, ids: &BTreeSet<String>) -> Option<Self> {
        let listing = Listing::parse(schedule)?;
        let records: Vec<Record> = listing
            .records
            .into_iter()
            .filter(|record| ids.contains(&record.id))
            .collect();
        let text = records
            .iter()
            .map(|record| record.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        Some(Block {
            text,
            listing: Some(Listing {
                heading: None,
                records,
                omitted: 0,
                footer: None,
            }),
        })
    }

    /// The listing without runs that already happened (counted in
    /// `*(and N more)*`), while an upcoming one remains.
    pub fn upcoming(&self) -> Option<String> {
        let listing = self.listing.as_ref()?;
        let upcoming = listing.upcoming()?;
        Some(listing.render(&upcoming))
    }
}
