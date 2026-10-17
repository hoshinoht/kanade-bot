//! The estimate context a day-of or countdown header rewrite may carry in
//! the redesigned style (D1–D7): per boss on the card the party's total %,
//! its clear minutes and "technically possible" when a whole-party total is
//! stored, the assumed-crit and not-complete flags, and up to two top
//! carriers offered as `{c1}`/`{c2}` with their share. The model sees only
//! [`HeaderFacts`]; the names behind the placeholders stay here, and code
//! fills them in after the gate. A stale estimate, a missing one or a card
//! with no total at all sends no context, so the rewrite is today's.
//!
//! Every reader of a stored heading goes through [`resolve_heading`]: it
//! re-checks the live switch and the card's party at each render (D6) and
//! fills the placeholders, or falls back to the kind's seed, so no output
//! ever carries an unfilled `{cN}`.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::super::{CardContext, CardRecord, card_runs};
use crate::bot::delivery::RunEstimate;
use crate::chat::nudge::{BossFigures, HeaderFacts, OfferedSlot, safe_value};
use crate::chat::persona::NudgeMood;
use crate::domain::notify::IntentContent;
use crate::runtime::logging;

/// The header switches as configured now (Config › Notifications), read at
/// every rewrite and render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeaderSwitches {
    /// `notifications.header_estimates` (D8, default off).
    pub estimates: bool,
}

/// The live switches; `None` in a kit is all off.
pub type HeaderSwitchSource = Arc<dyn Fn() -> HeaderSwitches + Send + Sync>;

/// The line `record` shows on the card of `content` now: its heading with
/// any placeholders filled, or `None` for the kind's seed (what
/// `cards::build` draws without a heading). The seed is used when the line
/// was written with estimates and the switch is now off, a slotted member
/// is no longer in the card's party, the map is missing or unreadable for a
/// line with a placeholder, or anything is left unfilled.
pub fn resolve_heading(
    record: &CardRecord,
    content: &IntentContent,
    ctx: &CardContext<'_>,
    switches: HeaderSwitches,
) -> Option<String> {
    let heading = record.heading.as_deref()?;
    let seed = |reason: &str| {
        logging::log!(info, "header_seed_at_render", reason = reason);
        None
    };
    let Some(slots) = &record.slots else {
        return if heading.contains('{') || heading.contains('}') {
            seed("no_slot_map")
        } else {
            Some(heading.to_owned())
        };
    };
    if slots.used_estimates && !switches.estimates {
        return seed("estimates_off");
    }
    let runs = match content {
        IntentContent::DayOf { run_ids } => card_runs(ctx, run_ids),
        IntentContent::Countdown { run_id, .. } => ctx.run(run_id).into_iter().collect(),
        _ => Vec::new(),
    };
    let in_party = |member: &str| {
        runs.iter()
            .any(|run| run.participants.iter().any(|id| id == member))
    };
    if !slots.slots.iter().all(|slot| in_party(&slot.member_id)) {
        return seed("left_party");
    }
    match slots.fill(heading) {
        Some(line) => Some(line),
        None => seed("unfilled"),
    }
}

/// Top carriers offered per card (D3).
pub const MAX_SLOTS: usize = 2;
/// Names shorter than this are not refused in a line: a one-letter name
/// would refuse almost every line and singles nobody out.
const MIN_NAME_CHARS: usize = 2;

/// One offered placeholder and the carrier code fills it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotFill {
    /// `{c1}`, `{c2}`.
    pub placeholder: String,
    pub member_id: String,
    pub character_id: String,
    /// What Who carries labels them with (D4: the IGN, no mention).
    pub ign: String,
}

/// What a header rewrite with estimates is built from: the model-facing
/// facts, the offered slots' fills and every party name the line must not
/// carry raw.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderContext {
    pub facts: HeaderFacts,
    pub fills: Vec<SlotFill>,
    /// Party members' names and aliases and every active character's IGN.
    pub names: Vec<String>,
    /// The rewrite's mood; gentle offers no slots (D7).
    pub mood: NudgeMood,
}

#[derive(Default)]
struct Tally {
    ign: String,
    character_id: String,
    sum: u32,
    count: u32,
    order: usize,
}

impl HeaderContext {
    /// The context of a card from each of its runs' estimates (card order).
    /// `None` without estimates, when any is stale (D2), or when no boss on
    /// the card has a total. `names` are the party members' names and
    /// aliases; the estimates add their characters' IGNs.
    pub fn build(
        estimates: &[RunEstimate],
        mood: NudgeMood,
        names: impl IntoIterator<Item = String>,
    ) -> Option<Self> {
        if estimates.is_empty() || estimates.iter().any(|run| run.older_than.is_some()) {
            return None;
        }
        let mut bosses = Vec::new();
        let mut tallies: BTreeMap<String, Tally> = BTreeMap::new();
        for estimate in estimates {
            let party = &estimate.party;
            for boss in 0..party.bosses.len() {
                let Some(total) = party.total(boss) else {
                    continue;
                };
                bosses.push(BossFigures {
                    total,
                    minutes: estimate.minutes.get(boss).copied().flatten(),
                    crit_assumed: party.crit_at(boss),
                    possible: estimate.possible.get(boss).copied().flatten(),
                    complete: party.complete(boss),
                });
                let shares = party.shares(boss);
                for ((carrier, owner), share) in
                    party.carriers.iter().zip(&estimate.owners).zip(shares)
                {
                    let Some(share) = share else { continue };
                    let order = tallies.len();
                    let tally = tallies
                        .entry(owner.member_id.clone())
                        .or_insert_with(|| Tally {
                            ign: carrier.ign.clone(),
                            character_id: owner.character_id.clone(),
                            order,
                            ..Tally::default()
                        });
                    tally.sum += share;
                    tally.count += 1;
                }
            }
        }
        if bosses.is_empty() {
            return None;
        }
        let mut ranked: Vec<(String, Tally)> = tallies
            .into_iter()
            .filter(|(_, tally)| tally.sum > 0)
            .collect();
        // Highest card share first (the mean of their per-boss shares),
        // ties to the earlier carrier.
        ranked.sort_by(|(_, a), (_, b)| {
            let mean = |tally: &Tally| f64::from(tally.sum) / f64::from(tally.count);
            mean(b).total_cmp(&mean(a)).then(a.order.cmp(&b.order))
        });
        let mut fills = Vec::new();
        let mut slots = Vec::new();
        if mood == NudgeMood::Playful {
            for (member_id, tally) in ranked
                .into_iter()
                .filter(|(_, tally)| safe_value(&tally.ign))
                .take(MAX_SLOTS)
            {
                let placeholder = format!("{{c{}}}", fills.len() + 1);
                slots.push(OfferedSlot {
                    placeholder: placeholder.clone(),
                    share: (tally.sum + tally.count / 2) / tally.count,
                });
                fills.push(SlotFill {
                    placeholder,
                    member_id,
                    character_id: tally.character_id,
                    ign: tally.ign,
                });
            }
        }
        let mut all: Vec<String> = Vec::new();
        for name in names
            .into_iter()
            .chain(estimates.iter().flat_map(|run| run.igns.iter().cloned()))
            .chain(fills.iter().map(|fill| fill.ign.clone()))
        {
            let name = name.trim().to_owned();
            if name.chars().count() >= MIN_NAME_CHARS
                && !all
                    .iter()
                    .any(|seen| seen.to_lowercase() == name.to_lowercase())
            {
                all.push(name);
            }
        }
        Some(Self {
            facts: HeaderFacts { bosses, slots },
            fills,
            names: all,
            mood,
        })
    }

    /// The offered placeholders, in order.
    pub fn offered(&self) -> Vec<&str> {
        self.fills
            .iter()
            .map(|fill| fill.placeholder.as_str())
            .collect()
    }
}

/// One filled placeholder of a stored line (migration 0043).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredSlot {
    /// `{c1}`.
    pub slot: String,
    pub member_id: String,
    pub character_id: String,
    pub ign: String,
    /// A tease slot (`tease-prompt`); none is offered yet.
    pub tease: bool,
}

/// The slot map stored beside a header line written with estimate context:
/// the placeholders the line uses and whether context was sent, so every
/// render can re-check the switch and the party before filling (D6).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderSlots {
    pub slots: Vec<StoredSlot>,
    pub used_estimates: bool,
}

impl HeaderSlots {
    /// The map of a line accepted with `context`, using `used`.
    pub fn of(context: &HeaderContext, used: &[String]) -> Self {
        Self {
            slots: used
                .iter()
                .filter_map(|slot| {
                    let fill = context
                        .fills
                        .iter()
                        .find(|fill| &fill.placeholder == slot)?;
                    Some(StoredSlot {
                        slot: fill.placeholder.clone(),
                        member_id: fill.member_id.clone(),
                        character_id: fill.character_id.clone(),
                        ign: fill.ign.clone(),
                        tease: false,
                    })
                })
                .collect(),
            used_estimates: true,
        }
    }

    /// `template` with each slot's IGN; `None` when an IGN is unsafe or any
    /// brace is left (an unfilled `{cN}` never reaches a reader). Day-of
    /// templates are stored with their day already filled.
    pub fn fill(&self, template: &str) -> Option<String> {
        let mut line = template.to_owned();
        for slot in &self.slots {
            if !safe_value(&slot.ign) {
                return None;
            }
            line = line.replace(&slot.slot, &slot.ign);
        }
        (!line.contains('{') && !line.contains('}')).then_some(line)
    }
}

/// Whether `line` carries any of `names` as whole words (case-insensitive;
/// a multi-word name must appear in order).
pub(super) fn names_any(line: &str, names: &[String]) -> bool {
    let words = |text: &str| -> Vec<String> {
        text.split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect()
    };
    let tokens = words(line);
    names.iter().any(|name| {
        let name = words(name);
        !name.is_empty()
            && tokens
                .windows(name.len())
                .any(|window| window.iter().zip(&name).all(|(left, right)| left == right))
    })
}
