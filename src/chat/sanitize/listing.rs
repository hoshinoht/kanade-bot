//! The canonical `get_schedule` listing as records, so grounding can show
//! only the runs a reply named and shaping can drop runs that already
//! happened (named v5 difference `D-GROUND-FILTERED`). A `D-MIXED-PEOPLE`
//! listing is several people's listings one after another, each under its
//! own heading; it keeps those headings whenever it is cut.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::{pattern, pattern_i};
use crate::domain::pytext::strip;

static RUN_ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"\[([0-9a-fA-F]{8})\]"));
static OMITTED: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"^\s*\*?\(and (\d+) more\)\*?\s*$"));
static FOOTER: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"^\s*\*?Every run listed has already happened"));
/// A personal `get_schedule` heading or its one-line answer when the person
/// has no runs: after the first paragraph, only these start another
/// person's listing (one tool output never has two).
static HEADING: LazyLock<Regex> = LazyLock::new(|| {
    pattern(
        r"\A(?:\*\*[^\n]+ · (?:All channels|This channel)\*\*|\*\*No upcoming runs for [^\n]+|(?:You are|[^\n]+ is) not on any runs\b[^\n]*)\z",
    )
});
const PAST: &str = "*already happened*";

#[derive(Clone)]
struct Record {
    id: String,
    text: String,
    past: bool,
}

/// One listing: its heading, records, omission count and footer.
#[derive(Clone)]
struct Section {
    heading: Option<String>,
    records: Vec<Record>,
    omitted: usize,
    footer: Option<String>,
}

impl Section {
    fn new(heading: Option<String>) -> Self {
        Section {
            heading,
            records: Vec::new(),
            omitted: 0,
            footer: None,
        }
    }

    fn upcoming(&self) -> Vec<&Record> {
        self.records.iter().filter(|r| !r.past).collect()
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

/// A listing split into one section per heading.
#[derive(Clone)]
struct Listing {
    sections: Vec<Section>,
}

impl Listing {
    /// `None` for a listing with parts this split does not recognise.
    fn parse(schedule: &str) -> Option<Self> {
        let mut sections = vec![Section::new(None)];
        let paragraphs = schedule
            .split("\n\n")
            .filter(|part| !strip(part).is_empty());
        for (index, paragraph) in paragraphs.enumerate() {
            let section = sections.last_mut()?;
            if let Some(found) = RUN_ID.captures(paragraph) {
                section.records.push(Record {
                    id: found[1].to_lowercase(),
                    text: paragraph.to_owned(),
                    past: paragraph.contains(PAST),
                });
            } else if let Some(found) = OMITTED.captures(paragraph) {
                section.omitted += found[1].parse::<usize>().ok()?;
            } else if FOOTER.is_match(paragraph) {
                section.footer = Some(paragraph.to_owned());
            } else if index == 0 {
                section.heading = Some(paragraph.to_owned());
            } else if HEADING.is_match(paragraph) {
                sections.push(Section::new(Some(paragraph.to_owned())));
            } else {
                return None;
            }
        }
        Some(Listing { sections })
    }

    /// More than one person's listing (`D-MIXED-PEOPLE`).
    fn sectioned(&self) -> bool {
        self.sections.len() > 1
    }

    fn render(&self, kept: impl Fn(&Section) -> Vec<&Record>) -> String {
        self.sections
            .iter()
            .map(|section| section.render(&kept(section)))
            .collect::<Vec<_>>()
            .join("\n\n")
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
    /// listing cannot be split. One listing loses its heading (the reply
    /// line introduces the records); a `D-MIXED-PEOPLE` listing keeps each
    /// person's heading over their records, so whose run is whose stays
    /// visible: a run two people share is under both, a person left with no
    /// named run goes, and a person with no runs keeps their one line.
    pub fn only(schedule: &str, ids: &BTreeSet<String>) -> Option<Self> {
        let listing = Listing::parse(schedule)?;
        if listing.sectioned() {
            let sections: Vec<Section> = listing
                .sections
                .into_iter()
                .filter_map(|section| {
                    let had = !section.records.is_empty();
                    let records: Vec<Record> = section
                        .records
                        .into_iter()
                        .filter(|record| ids.contains(&record.id))
                        .collect();
                    (!had || !records.is_empty()).then_some(Section {
                        records,
                        omitted: 0,
                        ..section
                    })
                })
                .collect();
            if sections.iter().all(|section| section.records.is_empty()) {
                return Some(Block {
                    text: String::new(),
                    listing: None,
                });
            }
            let kept = Listing { sections };
            return Some(Block {
                text: kept.render(|section| section.records.iter().collect()),
                listing: Some(kept),
            });
        }
        let records: Vec<Record> = listing
            .sections
            .into_iter()
            .flat_map(|section| section.records)
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
                sections: vec![Section {
                    records,
                    ..Section::new(None)
                }],
            }),
        })
    }

    /// The listing without runs that already happened (counted in
    /// `*(and N more)*`), while an upcoming one remains.
    pub fn upcoming(&self) -> Option<String> {
        let listing = self.listing.as_ref()?;
        let total: usize = listing.sections.iter().map(|s| s.records.len()).sum();
        let upcoming: usize = listing.sections.iter().map(|s| s.upcoming().len()).sum();
        (upcoming > 0 && upcoming < total).then(|| listing.render(Section::upcoming))
    }
}
