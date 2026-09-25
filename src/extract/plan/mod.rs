//! Burst planning (v4 `pipeline.plan_burst` and its helpers): normalise the
//! model's extraction, merge, resolve and match each amendment, and keep or
//! drop it with a reason. No database, no Discord, no clock reads.

mod rules;

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;

pub use rules::{
    STALE_GRACE, already_passed, consolidate, inherit_from_run, is_no_op, one_per_run,
    volunteers_for,
};

use crate::domain::catalog::BossTable;
use crate::domain::schedule::Run;
use crate::domain::time::DateOutOfRange;
use crate::extract::gate::{explicit_rsvp, find_days};
use crate::extract::matching::{NO_BOSS_OVERLAP, match_run, needs_run, reachable, runs_spanned};
use crate::extract::merge::merge;
use crate::extract::resolve::{Resolved, resolve};
use crate::extract::schema::Extraction;
use crate::extract::{Amendment, AmendmentKind};

/// What committing one planned change needs beyond the amendment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Payload {
    Empty,
    /// A weekly timing's slot.
    Fix {
        weekday: Weekday,
        time: NaiveTime,
    },
    /// The bosses moving to the new run and who goes.
    Split {
        bosses: Vec<String>,
        participants: Vec<String>,
    },
    /// Who leaves and who stands in.
    Sub {
        remove: Vec<String>,
        add: Vec<String>,
    },
}

/// One merged amendment, resolved and matched.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned<'a> {
    pub amendment: Amendment,
    pub resolved: Resolved,
    pub run: Option<&'a Run>,
    pub payload: Payload,
    pub match_reason: String,
    pub match_code: &'static str,
    /// Kinds that lost to this one for the same run, named on the card.
    pub also_mentioned: Vec<AmendmentKind>,
    /// Several runs matched equally well.
    pub ambiguous: bool,
    /// The model's summary of the burst this came from.
    pub summary: String,
}

impl Planned<'_> {
    pub fn kind(&self) -> AmendmentKind {
        self.amendment.kind
    }

    /// The card should read as a suggestion, not a decision.
    pub fn needs_answer(&self) -> bool {
        self.amendment.is_question || self.resolved.at.is_none()
    }
}

/// Everything one burst produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan<'a> {
    pub planned: Vec<Planned<'a>>,
    pub dropped: Vec<Planned<'a>>,
    pub summary: String,
}

/// One burst message as planning reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BurstMessage {
    pub id: String,
    pub author_id: String,
    pub content: String,
}

/// Every input `plan_burst` needs; nothing defaults from the wall clock.
/// Planned entries borrow only the runs (`'a`).
#[derive(Clone, Copy, Debug)]
pub struct BurstInputs<'i, 'a> {
    /// The latest evidence message's time, which day/time text resolves against.
    pub anchor: DateTime<Utc>,
    /// What "already passed" is measured against.
    pub now: DateTime<Utc>,
    pub zone: Tz,
    pub channel_runs: &'i [&'a Run],
    pub guild_runs: &'i [&'a Run],
    /// Message ids oldest first, context included.
    pub burst_order: &'i [String],
    /// Message id -> author id.
    pub author_ids: &'i HashMap<String, String>,
    pub min_confidence: f64,
    /// Canonicalises model boss names when given.
    pub boss_table: Option<&'i BossTable>,
    /// Scanned for explicit RSVPs the model did not report.
    pub burst_messages: &'i [BurstMessage],
}

/// Kinds that can be about several runs at once, one candidate per run.
fn splits_across_runs(kind: AmendmentKind) -> bool {
    matches!(
        kind,
        AmendmentKind::Sub | AmendmentKind::Move | AmendmentKind::Cancel | AmendmentKind::Otot
    )
}

/// Kinds still worth acting on after a coin-toss match: an rsvp is an
/// opinion and a stand-in is a `/swap` away.
fn acts_on_ambiguous(kind: AmendmentKind) -> bool {
    matches!(kind, AmendmentKind::Sub | AmendmentKind::Rsvp)
}

/// Canonical boss names, and an `rsvp` for each explicit answer the model did
/// not cite.
fn normalise(
    extraction: &Extraction,
    table: Option<&BossTable>,
    messages: &[BurstMessage],
) -> Extraction {
    let mut result = extraction.clone();
    if let Some(table) = table {
        for amendment in &mut result.amendments {
            let mut bosses: Vec<String> = Vec::new();
            for name in &amendment.bosses {
                let canonical = table.parse_token(name).unwrap_or_else(|_| name.clone());
                if !bosses.contains(&canonical) {
                    bosses.push(canonical);
                }
            }
            amendment.bosses = bosses;
        }
    }
    let cited: HashSet<String> = result
        .amendments
        .iter()
        .filter(|amendment| amendment.kind == AmendmentKind::Rsvp)
        .flat_map(|amendment| amendment.evidence_message_ids.iter().cloned())
        .collect();
    for message in messages {
        let Some(answer) = explicit_rsvp(&message.content) else {
            continue;
        };
        if cited.contains(&message.id) {
            continue;
        }
        result.amendments.push(Amendment {
            day_ref: find_days(&message.content).into_iter().next(),
            participants: vec![message.author_id.clone()],
            rsvp: Some(answer),
            confidence: 0.9,
            evidence_message_ids: vec![message.id.clone()],
            ..Amendment::new(AmendmentKind::Rsvp)
        });
    }
    result
}

fn shares_a_boss(bosses: &HashSet<&str>, amendment: &Amendment) -> bool {
    amendment
        .bosses
        .iter()
        .any(|boss| bosses.contains(boss.as_str()))
}

/// Merge, resolve and match one extraction, then keep or drop each result.
///
/// # Errors
/// [`DateOutOfRange`] when a resolved instant leaves years 1..=9999.
pub fn plan_burst<'a>(
    extraction: &Extraction,
    inputs: &BurstInputs<'_, 'a>,
) -> Result<Plan<'a>, DateOutOfRange> {
    let zone = inputs.zone;
    let extraction = normalise(extraction, inputs.boss_table, inputs.burst_messages);
    let existing: Vec<Vec<String>> = inputs
        .channel_runs
        .iter()
        .map(|run| run.bosses.clone())
        .collect();
    let merged = merge(&extraction.amendments, inputs.burst_order, &existing);
    // Bosses the burst already proposes a new run for: a `move` about them is
    // that proposal settling, not a second run.
    let proposed: HashSet<&str> = merged
        .iter()
        .filter(|a| matches!(a.kind, AmendmentKind::Add | AmendmentKind::Fix))
        .flat_map(|a| a.bosses.iter().map(String::as_str))
        .collect();

    let mut plan = Plan {
        summary: extraction.summary.clone(),
        ..Plan::default()
    };
    for amendment in &merged {
        let resolved = resolve(
            amendment.day_ref.as_deref(),
            amendment.time_ref.as_deref(),
            &inputs.anchor,
            zone,
        )?;
        let author = amendment
            .evidence_message_ids
            .iter()
            .find_map(|id| inputs.author_ids.get(id))
            .map(String::as_str);
        // Runs whose week starts after the night in question are never
        // candidates, on any matching path.
        let here = reachable(inputs.channel_runs, resolved.day, zone);
        let guild_here = reachable(inputs.guild_runs, resolved.day, zone);
        let spanned = if splits_across_runs(amendment.kind) {
            runs_spanned(amendment, &here, author)
        } else {
            Vec::new()
        };

        let mut entries: Vec<Planned<'a>> = Vec::new();
        if spanned.len() > 1 {
            for &run in &spanned {
                let per_run = Amendment {
                    bosses: run.bosses.clone(),
                    ..amendment.clone()
                };
                let volunteers = volunteers_for(&per_run, inputs.author_ids);
                entries.push(Planned {
                    payload: rules::payload_for(&per_run, &resolved, Some(run), &volunteers),
                    amendment: per_run,
                    resolved,
                    run: Some(run),
                    match_reason: format!("one of {} runs it spans", spanned.len()),
                    match_code: "",
                    also_mentioned: Vec::new(),
                    ambiguous: false,
                    summary: String::new(),
                });
            }
        } else {
            let mut result = match_run(
                amendment,
                &here,
                &guild_here,
                author,
                &amendment.participants,
            );
            if result.run.is_none() && !inputs.channel_runs.is_empty() && here.is_empty() {
                result.reason = "every run here belongs to a later boss week".to_owned();
            }
            let mut amendment = amendment.clone();
            if result.run.is_none()
                && result.reason_code == NO_BOSS_OVERLAP
                && matches!(amendment.kind, AmendmentKind::Move | AmendmentKind::Split)
                && resolved.at.is_some()
                // A named day, not a bare time: "amend to 9:45" is no new night.
                && amendment.day_ref.as_deref().is_some_and(|day| !day.is_empty())
                && !shares_a_boss(&proposed, &amendment)
            {
                // Nothing here runs those bosses but a day and time were agreed:
                // one `add` card beats a `move` pointed at a stranger's night.
                amendment.kind = AmendmentKind::Add;
            }
            let volunteers = volunteers_for(&amendment, inputs.author_ids);
            entries.push(Planned {
                payload: rules::payload_for(&amendment, &resolved, result.run, &volunteers),
                amendment,
                resolved,
                run: result.run,
                match_reason: result.reason,
                match_code: result.reason_code,
                also_mentioned: Vec::new(),
                ambiguous: result.ambiguous,
                summary: String::new(),
            });
        }

        for mut entry in entries {
            entry.summary.clone_from(&extraction.summary);
            if entry.amendment.confidence < inputs.min_confidence {
                plan.dropped.push(entry);
                continue;
            }
            if needs_run(entry.kind()) && entry.run.is_none() {
                entry.match_reason = format!("no run matched ({})", entry.match_reason);
                plan.dropped.push(entry);
                continue;
            }
            if entry.ambiguous && !acts_on_ambiguous(entry.kind()) {
                entry.match_reason = format!("ambiguous ({})", entry.match_reason);
                plan.dropped.push(entry);
                continue;
            }
            if entry.kind() == AmendmentKind::Add
                && !entry.resolved.known()
                && !entry.amendment.is_question
            {
                // The shape a truncated extraction leaves; a question with no
                // day or time is exactly what a "when?" card is for.
                entry.match_reason = "a stated add with no day or time".to_owned();
                plan.dropped.push(entry);
                continue;
            }
            // Before the staleness and no-op checks, which judge the instant a
            // half-stated move only has once the run fills in the rest.
            entry.resolved = inherit_from_run(&entry, zone)?;
            if already_passed(&entry, inputs.now, zone) {
                entry.match_reason = "already passed".to_owned();
                plan.dropped.push(entry);
                continue;
            }
            if is_no_op(&entry, zone) {
                entry.match_reason = "already scheduled".to_owned();
                plan.dropped.push(entry);
                continue;
            }
            if entry.kind() == AmendmentKind::Add {
                // An `add` creates a run; nulled only after `is_no_op`, which
                // needs the match to spot a night the channel already has.
                entry.run = None;
            }
            plan.planned.push(entry);
        }
    }
    plan.planned = one_per_run(std::mem::take(&mut plan.planned));
    Ok(plan)
}
