//! The estimate context a day-of or countdown header rewrite may carry in
//! the redesigned style (D1–D7): per boss on the card the party's total %,
//! its clear minutes and "technically possible" when a whole-party total is
//! stored, the assumed-crit and not-complete flags, and up to two top
//! carriers offered as `{c1}`/`{c2}` with their share. With teasing on
//! (D11/D12) one opted-in party member with an estimate (the lowest share
//! among them) is offered as the next `{cN}`, with their share and the
//! admins' running-joke note. The model sees only [`HeaderFacts`]; the
//! names behind the placeholders stay here, and code fills them in after
//! the gate. A stale estimate, a missing one or a card with no total at all
//! sends no context, so the rewrite is today's.
//!
//! Every reader of a stored heading goes through [`resolve_heading`]: it
//! re-checks the live switches, the card's party and a teased member's
//! opt-in at each render (D6) and fills the placeholders, or falls back to
//! the kind's seed, so no output ever carries an unfilled `{cN}`.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::cards::estimates::RunEstimate;
use crate::cards::{CardContext, card_runs};
use crate::chat::nudge::{BossFigures, HeaderFacts, OfferedSlot, TeaseSlot, safe_value};
use crate::chat::persona::NudgeMood;
use kanade_domain::members::MemberProfile;
use kanade_domain::notify::{CardRecord, HeaderSlots, IntentContent, StoredSlot};
use kanade_domain::tease::note_problem;

/// The header switches as configured now (Config › Notifications), read at
/// every rewrite and render.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeaderSwitches {
    /// `notifications.header_estimates` (D8, default off).
    pub estimates: bool,
    /// `notifications.header_teasing` (D12, default off; only on with
    /// `estimates`).
    pub teasing: bool,
}

/// The live switches; `None` in a kit is all off.
pub type HeaderSwitchSource = Arc<dyn Fn() -> HeaderSwitches + Send + Sync>;

/// The line `record` shows on the card of `content` now: its heading with
/// any placeholders filled, or `None` for the kind's seed (what
/// `cards::build` draws without a heading). The seed is used when the line
/// was written with estimates and the switch is now off, a slotted member
/// is no longer in the card's party, a tease slot's teasing switch is now
/// off or its member has opted out (D6/D12), the map is missing or
/// unreadable for a line with a placeholder, or anything is left unfilled.
pub fn resolve_heading(
    record: &CardRecord,
    content: &IntentContent,
    ctx: &CardContext<'_>,
    switches: HeaderSwitches,
) -> Option<String> {
    let heading = record.heading.as_deref()?;
    let seed = |reason: &str| {
        kanade_log::log!(info, "header_seed_at_render", reason = reason);
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
    for slot in slots.slots.iter().filter(|slot| slot.tease) {
        if !switches.teasing {
            return seed("teasing_off");
        }
        let opted_in = ctx
            .members
            .profile(&slot.member_id)
            .is_some_and(|profile| profile.tease_ok);
        if !opted_in {
            return seed("tease_opt_out");
        }
    }
    match fill_slots(slots, heading) {
        Some(line) => Some(line),
        None => seed("unfilled"),
    }
}

/// Top carriers offered per card (D3), and the most placeholders one line
/// may use, a tease slot included (D5).
pub const MAX_SLOTS: usize = 2;
/// Names shorter than this are not refused in a line: a one-letter name
/// would refuse almost every line and singles nobody out.
const MIN_NAME_CHARS: usize = 2;

/// One offered placeholder and the member code fills it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotFill {
    /// `{c1}`, `{c2}`; a tease slot takes the next number.
    pub placeholder: String,
    pub member_id: String,
    pub character_id: String,
    /// What Who carries labels them with (D4: the IGN, no mention).
    pub ign: String,
    /// The slot offered for light teasing (D11), not as a top carrier.
    pub tease: bool,
}

/// A party member who opted in to teasing (D9), with the admins' note if
/// it may still be sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Teasable {
    pub member_id: String,
    pub note: Option<String>,
}

impl Teasable {
    /// `profile` as a tease candidate: `None` unless they opted in. Their
    /// note is checked again with the save-time rules (D10) against
    /// `roster`, every roster member's name, alias and IGN as they are now,
    /// and left out when it now names someone or the roster could not be
    /// read (`None`).
    pub fn of(profile: &MemberProfile, roster: Option<&[String]>) -> Option<Self> {
        if !profile.tease_ok {
            return None;
        }
        let note = profile
            .running_joke
            .as_deref()
            .map(str::trim)
            .filter(|note| {
                !note.is_empty()
                    && roster.is_some_and(|names| {
                        note_problem(note, names.iter().map(String::as_str)).is_none()
                    })
            });
        Some(Self {
            member_id: profile.member.user_id.clone(),
            note: note.map(str::to_owned),
        })
    }
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
    /// The rewrite's mood; gentle offers no slots, tease included (D7/D11).
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

impl Tally {
    /// The mean of their per-boss shares on the card.
    fn mean(&self) -> f64 {
        f64::from(self.sum) / f64::from(self.count)
    }

    /// [`Self::mean`] as a whole %.
    fn share(&self) -> u32 {
        (self.sum + self.count / 2) / self.count
    }
}

impl HeaderContext {
    /// The context of a card from each of its runs' estimates (card order).
    /// `None` without estimates, when any is stale (D2), or when no boss on
    /// the card has a total. `names` are the party members' names and
    /// aliases; the estimates add their characters' IGNs. `teasable` are
    /// the party members who opted in to teasing, empty while teasing is
    /// off; one of them with an estimate and no carrier slot gets the tease
    /// slot, the lowest share first (D11).
    pub fn build(
        estimates: &[RunEstimate],
        mood: NudgeMood,
        names: impl IntoIterator<Item = String>,
        teasable: &[Teasable],
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
        let mut ranked: Vec<(String, Tally)> = tallies.into_iter().collect();
        // Highest card share first, ties to the earlier carrier.
        ranked.sort_by(|(_, a), (_, b)| b.mean().total_cmp(&a.mean()).then(a.order.cmp(&b.order)));
        let mut fills = Vec::new();
        let mut slots = Vec::new();
        let mut tease = None;
        if mood == NudgeMood::Playful {
            for (member_id, tally) in ranked
                .iter()
                .filter(|(_, tally)| tally.sum > 0 && safe_value(&tally.ign))
                .take(MAX_SLOTS)
            {
                let placeholder = format!("{{c{}}}", fills.len() + 1);
                slots.push(OfferedSlot {
                    placeholder: placeholder.clone(),
                    share: tally.share(),
                });
                fills.push(SlotFill {
                    placeholder,
                    member_id: member_id.clone(),
                    character_id: tally.character_id.clone(),
                    ign: tally.ign.clone(),
                    tease: false,
                });
            }
            // A member already offered as a carrier is never also teased.
            let teased = ranked
                .iter()
                .filter(|(member_id, tally)| {
                    safe_value(&tally.ign) && !fills.iter().any(|fill| &fill.member_id == member_id)
                })
                .filter_map(|(member_id, tally)| {
                    let who = teasable.iter().find(|who| &who.member_id == member_id)?;
                    Some((who, tally))
                })
                .min_by(|(_, a), (_, b)| a.mean().total_cmp(&b.mean()).then(a.order.cmp(&b.order)));
            if let Some((who, tally)) = teased {
                let placeholder = format!("{{c{}}}", fills.len() + 1);
                tease = Some(TeaseSlot {
                    placeholder: placeholder.clone(),
                    share: tally.share(),
                    note: who.note.clone(),
                });
                fills.push(SlotFill {
                    placeholder,
                    member_id: who.member_id.clone(),
                    character_id: tally.character_id.clone(),
                    ign: tally.ign.clone(),
                    tease: true,
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
            facts: HeaderFacts {
                bosses,
                slots,
                tease,
            },
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

    /// The slot map of a line accepted with this context, using `used`.
    pub fn slots_for(&self, used: &[String]) -> HeaderSlots {
        HeaderSlots {
            slots: used
                .iter()
                .filter_map(|slot| {
                    let fill = self.fills.iter().find(|fill| &fill.placeholder == slot)?;
                    Some(StoredSlot {
                        slot: fill.placeholder.clone(),
                        member_id: fill.member_id.clone(),
                        character_id: fill.character_id.clone(),
                        ign: fill.ign.clone(),
                        tease: fill.tease,
                    })
                })
                .collect(),
            used_estimates: true,
        }
    }
}

/// `template` with each of `slots`' IGNs; `None` when an IGN is unsafe or
/// any brace is left (an unfilled `{cN}` never reaches a reader). Day-of
/// templates are stored with their day already filled.
pub fn fill_slots(slots: &HeaderSlots, template: &str) -> Option<String> {
    let mut line = template.to_owned();
    for slot in &slots.slots {
        if !safe_value(&slot.ign) {
            return None;
        }
        line = line.replace(&slot.slot, &slot.ign);
    }
    (!line.contains('{') && !line.contains('}')).then_some(line)
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
