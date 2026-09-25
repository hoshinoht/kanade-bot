//! What an import found and did: counts, reason codes and v4 fixed-run ids
//! only, never message text or member names.

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Utc};

/// Per kind: added (or, in a dry run, would add), already present, and
/// skipped rows by reason code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub added: u64,
    pub present: u64,
    pub skipped: BTreeMap<&'static str, u64>,
}

impl Counts {
    pub fn skip(&mut self, reason: &'static str) {
        *self.skipped.entry(reason).or_default() += 1;
    }

    pub fn skipped_total(&self) -> u64 {
        self.skipped.values().sum()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub applied: bool,
    /// Logs at or after this instant were considered.
    pub cutoff: DateTime<Utc>,
    pub fixed_runs: Counts,
    /// Skipped fixed runs: the v4 id (or `row N` when the id itself is
    /// unusable) and the reason.
    pub fixed_skipped: Vec<(String, &'static str)>,
    /// Runs created by materialising the current and next boss weeks
    /// (`None` in a dry run).
    pub materialised: Option<usize>,
    pub chats: Counts,
    pub extractions: Counts,
    pub messages: Counts,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let verb = if self.applied { "added" } else { "would add" };
        if self.applied {
            writeln!(f, "kanade import v4: applied")?;
        } else {
            writeln!(
                f,
                "kanade import v4: dry run, nothing written (re-run with --apply)"
            )?;
        }
        writeln!(
            f,
            "logs from {}",
            crate::domain::time::to_iso(&self.cutoff).unwrap_or_default()
        )?;
        let line = |f: &mut fmt::Formatter<'_>, kind: &str, counts: &Counts| {
            write!(
                f,
                "{kind}: {verb} {}, already present {}, skipped {}",
                counts.added,
                counts.present,
                counts.skipped_total()
            )?;
            if !counts.skipped.is_empty() {
                let reasons: Vec<String> = counts
                    .skipped
                    .iter()
                    .map(|(reason, count)| format!("{reason} {count}"))
                    .collect();
                write!(f, " ({})", reasons.join(", "))?;
            }
            writeln!(f)
        };
        line(f, "fixed runs", &self.fixed_runs)?;
        for (id, reason) in &self.fixed_skipped {
            writeln!(f, "  skipped fixed run {id}: {reason}")?;
        }
        match self.materialised {
            Some(created) => writeln!(f, "materialised runs: {created}")?,
            None => writeln!(f, "materialised runs: on --apply")?,
        }
        line(f, "chat logs", &self.chats)?;
        line(f, "extraction logs", &self.extractions)?;
        line(f, "messages", &self.messages)
    }
}
