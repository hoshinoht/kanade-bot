//! The read tools and the helpers the proposal tools share with them. Every
//! function is pure over a [`ToolWorld`] the caller loads and the context's
//! clock reading.

mod event;
pub mod format;
mod guide;
pub mod participants;
pub mod resolve;
pub mod roster;
mod schedule;

use chrono::{NaiveTime, Weekday};
use chrono_tz::Tz;
use serde_json::{Map, Value};

pub use event::{EventBoss, match_event};
pub use guide::render_guide;
pub use schedule::get_schedule;

use crate::chat::gate::{ChannelDirectory, PilotSettings};
use crate::chat::tools::{MAX_RUNS, ToolContext, ToolError, ToolResult};
use crate::domain::catalog::{BossReference, BossTable};
use crate::domain::members::{Directory, Member};
use crate::domain::pytext::strip;
use crate::domain::schedule::ScheduleSnapshot;
use format::{boss_label, boss_labels, fixed_line, run_detail};
use resolve::resolve_run;

/// Why [`StrategyGuides::render`] has no guide text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuideError {
    /// No guide is checked in for the boss.
    Missing,
    /// A guide is checked in but could not be read or rendered right now.
    Unreadable,
}

/// Checked-in boss strategy guides (serve: the schema v2 knowledge
/// directory through [`render_guide`]).
pub trait StrategyGuides {
    /// The guide text for one boss and optional difficulty; `reference.short`
    /// is a catalog key or an [`EventBoss::key`].
    fn render(&self, reference: &BossReference) -> Result<String, GuideError>;

    /// Event bosses outside the catalog with a checked-in guide.
    fn events(&self) -> Vec<EventBoss> {
        Vec::new()
    }
}

/// One proposal card still waiting for ✅, as the inbox names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingCard {
    pub short_id: String,
    /// `move`, `new run`, `remove weekly`, …
    pub kind_label: String,
    pub bosses: Vec<String>,
    pub when: String,
}

/// Everything a tool reads, loaded by the caller for one round.
#[derive(Clone, Copy)]
pub struct ToolWorld<'a> {
    pub snapshot: &'a ScheduleSnapshot,
    /// Every known member; name matching uses the role holders.
    pub members: &'a [Member],
    pub directory: &'a (dyn Directory + Sync),
    pub catalog: &'a BossTable,
    pub channels: &'a (dyn ChannelDirectory + Sync),
    pub pilot: &'a PilotSettings,
    pub zone: Tz,
    pub reset_weekday: Weekday,
    pub reset_time: NaiveTime,
    pub pending: &'a [PendingCard],
    /// `None` when strategy knowledge is unavailable.
    pub guides: Option<&'a (dyn StrategyGuides + Sync)>,
}

fn query(args: &Map<String, Value>, name: &str) -> String {
    participants::py_text(args.get(name))
}

/// `get_run`.
pub fn get_run(
    world: &ToolWorld<'_>,
    ctx: &ToolContext,
    args: &Map<String, Value>,
) -> ToolResult<String> {
    let run = resolve_run(world, &query(args, "query"), ctx.now)?;
    Ok(run_detail(world, run))
}

/// `list_bosses`: catalog bosses and any event bosses with checked-in guides.
pub fn list_bosses(world: &ToolWorld<'_>) -> String {
    let rows: Vec<String> = world
        .catalog
        .ordered()
        .into_iter()
        .map(|boss| {
            let forms: Vec<String> = boss
                .difficulties()
                .iter()
                .map(|letter| {
                    let token = boss.canonical(letter);
                    format!("`{token}` = {}", boss_label(&token))
                })
                .collect();
            let level = boss
                .level()
                .map_or_else(|| "None".to_owned(), |l| l.to_string());
            format!(
                "**{}** ({}, lv `{level}`): {}",
                boss.short(),
                boss.full(),
                forms.join(", ")
            )
        })
        .collect();
    let mut lines = vec!["**Bosses this guild runs**".to_owned(), String::new()];
    lines.extend(rows);
    let mut output = lines.join("\n");
    let mut events = world
        .guides
        .map(|guides| guides.events())
        .unwrap_or_default();
    events.sort_by(|left, right| left.key.cmp(&right.key));
    if !events.is_empty() {
        output.push_str("\n\n**Seasonal bosses (guide only, not scheduled)**");
        for event in events {
            output.push_str(&format!(
                "\n**{}** ({}): {}{}",
                event.key,
                event.name,
                event.availability,
                if event.aliases.is_empty() {
                    String::new()
                } else {
                    format!(" Also called {}.", event.aliases.join(", "))
                }
            ));
        }
    }
    output
}

/// `list_fixed`: the recurring weekly timings.
pub fn list_fixed(world: &ToolWorld<'_>) -> String {
    let fixed = &world.snapshot.fixed_runs;
    if fixed.is_empty() {
        return "There are no recurring weekly timings.".to_owned();
    }
    let mut lines = vec!["**Weekly timings**".to_owned(), String::new()];
    lines.extend(
        fixed
            .iter()
            .take(MAX_RUNS)
            .map(|row| fixed_line(world, row)),
    );
    let more = fixed.len().saturating_sub(MAX_RUNS);
    let tail = if more > 0 {
        format!("\n*(and {more} more)*")
    } else {
        String::new()
    };
    lines.join("\n") + &tail
}

/// `get_pending`: proposal cards still waiting for a ✅.
pub fn get_pending(world: &ToolWorld<'_>) -> String {
    if world.pending.is_empty() {
        return "There are no proposal cards waiting.".to_owned();
    }
    let mut lines = vec!["**Waiting for a ✅**".to_owned(), String::new()];
    lines.extend(world.pending.iter().take(MAX_RUNS).map(|card| {
        format!(
            "`[{}]` **{}** **{}** → *{}*",
            card.short_id,
            card.kind_label,
            boss_labels(&card.bosses),
            card.when
        )
    }));
    lines.join("\n")
}

/// One explicitly requested difficulty, normalised without choosing one.
fn difficulty(world: &ToolWorld<'_>, value: Option<&Value>) -> ToolResult<Option<String>> {
    let Some(Value::String(raw)) = value else {
        return Ok(None);
    };
    let key = strip(raw).to_lowercase();
    if key.is_empty() {
        return Ok(None);
    }
    let difficulties = world.catalog.difficulties();
    if let Some(found) = difficulties.iter().find(|d| d.letter() == key) {
        return Ok(Some(found.letter().to_owned()));
    }
    if let Some(found) = difficulties
        .iter()
        .find(|d| d.label().to_lowercase() == key)
    {
        return Ok(Some(found.letter().to_owned()));
    }
    let choices: Vec<&str> = difficulties.iter().map(|d| d.label()).collect();
    Err(ToolError(format!(
        "Unknown difficulty `{raw}`. Use one of: {}.",
        choices.join(", ")
    )))
}

/// `get_boss_strategy`: checked-in notes for one unambiguous boss.
pub fn get_boss_strategy(world: &ToolWorld<'_>, args: &Map<String, Value>) -> ToolResult<String> {
    let raw = match args.get("boss") {
        Some(Value::String(raw)) if !strip(raw).is_empty() => raw,
        _ => return Err(ToolError::new("Ask which boss they want strategy for.")),
    };
    let reference = match world.catalog.resolve_reference(raw) {
        Ok(reference) => reference,
        Err(error) => {
            // Only a name the catalog cannot resolve may be an event boss.
            if let Some(guides) = world.guides {
                let events = guides.events();
                if let Some((event, stated)) =
                    match_event(&events, world.catalog, raw).map_err(ToolError)?
                {
                    return event_strategy(world, guides, args, &event.key, stated);
                }
            }
            return Err(ToolError(error.message().to_owned()));
        }
    };
    let explicit = difficulty(world, args.get("difficulty"))?;
    if let (Some(explicit), Some(stated)) = (&explicit, &reference.difficulty)
        && explicit != stated
    {
        return Err(ToolError(format!(
            "conflicting difficulties: {} and {}",
            world.catalog.difficulty_name(stated),
            world.catalog.difficulty_name(explicit)
        )));
    }
    let chosen = explicit.or_else(|| reference.difficulty.clone());
    let boss = world
        .catalog
        .boss(&reference.short)
        .ok_or_else(|| ToolError(format!("no boss found in `{raw}`")))?;
    if let Some(letter) = &chosen
        && !boss.difficulties().contains(letter)
    {
        return Err(ToolError(format!(
            "{} has no {} difficulty - available forms are {}.",
            boss.full(),
            world.catalog.difficulty_name(letter),
            world
                .catalog
                .valid_forms(&reference.short)
                .unwrap_or_default()
        )));
    }
    let guides = world
        .guides
        .ok_or_else(|| ToolError::new("Boss strategy knowledge is unavailable right now."))?;
    guides
        .render(&BossReference {
            short: reference.short.clone(),
            difficulty: chosen,
        })
        .map_err(|error| match error {
            GuideError::Missing => ToolError(format!(
                "No checked-in strategy guide is available for {}.",
                boss.full()
            )),
            GuideError::Unreadable => ToolError(format!(
                "The strategy guide for {} could not be read right now.",
                boss.full()
            )),
        })
}

/// `get_boss_strategy` for an event boss: the same difficulty handling, minus
/// the catalog's per-boss difficulty list.
fn event_strategy(
    world: &ToolWorld<'_>,
    guides: &(dyn StrategyGuides + Sync),
    args: &Map<String, Value>,
    key: &str,
    stated: Option<String>,
) -> ToolResult<String> {
    let explicit = difficulty(world, args.get("difficulty"))?;
    if let (Some(explicit), Some(stated)) = (&explicit, &stated)
        && explicit != stated
    {
        return Err(ToolError(format!(
            "conflicting difficulties: {} and {}",
            world.catalog.difficulty_name(stated),
            world.catalog.difficulty_name(explicit)
        )));
    }
    guides
        .render(&BossReference {
            short: key.to_owned(),
            difficulty: explicit.or(stated),
        })
        .map_err(|error| match error {
            GuideError::Missing => ToolError(format!(
                "No checked-in strategy guide is available for {key}."
            )),
            GuideError::Unreadable => ToolError(format!(
                "The strategy guide for {key} could not be read right now."
            )),
        })
}
