//! The `get_boss_strategy` guide text: v4's `BossKnowledgeBase.render`
//! shape (`include_sources=False`) over one schema v2 knowledge document.

use serde_json::Value;

use crate::domain::catalog::{BossReference, BossTable};

fn strings<'a>(document: &'a Value, field: &str) -> Option<Vec<&'a str>> {
    document
        .get(field)?
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect()
}

/// A bullet: plain text, or `Title: detail` (the bot's long wording),
/// falling back to `Title: text`.
fn item(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_owned());
    }
    let title = value.get("title")?.as_str()?;
    let body = value
        .get("detail")
        .or_else(|| value.get("text"))?
        .as_str()?;
    Some(format!("{title}: {body}"))
}

/// Every bullet in `field`, or `None` when it is missing or one is malformed.
fn items(document: &Value, field: &str) -> Option<Vec<String>> {
    document.get(field)?.as_array()?.iter().map(item).collect()
}

fn bullets(lines: &mut Vec<String>, items: Vec<String>) {
    lines.extend(items.into_iter().map(|bullet| format!("- {bullet}")));
}

/// Mission modifiers and ledger rows: `up` is in the player's favour.
fn favour(direction: &str) -> Option<&'static str> {
    match direction {
        "up" => Some("in your favour"),
        "down" => Some("against you"),
        _ => None,
    }
}

/// A difficulty's solo mission as plain lines.
fn mission(lines: &mut Vec<String>, mission: &Value) -> Option<()> {
    let text = |field: &str| mission.get(field).and_then(Value::as_str);
    let series = match text("series")? {
        "destiny-weapon" => "Destiny Weapon",
        "union-champion" => "Union Champion",
        _ => return None,
    };
    let order = mission.get("order")?.as_u64()?;
    lines.push(format!(
        "- Mission: {} ({series} mission {order})",
        text("title")?
    ));
    if let Some(modifier) = mission.get("modifier") {
        let what = modifier.get("text")?.as_str()?;
        let direction = favour(modifier.get("direction")?.as_str()?)?;
        lines.push(format!("- Mission modifier: {what} ({direction})"));
    }
    if let Some(needs) = mission.get("needs") {
        lines.push(format!("- Mission needs: {}", needs.as_str()?));
    }
    if let Some(rules) = mission.get("rules") {
        let rules: Option<Vec<&str>> = rules.as_array()?.iter().map(Value::as_str).collect();
        lines.push(format!("- Mission rules: {}", rules?.join("; ")));
    }
    Some(())
}

fn difficulty_facts(lines: &mut Vec<String>, facts: &Value) {
    for (field, label, unit) in [
        ("entry_level", "Entry level", ""),
        ("boss_level", "Boss level", ""),
        ("pdr_percent", "PDR", "%"),
        ("party_max", "Party max", ""),
    ] {
        if let Some(value) = facts.get(field) {
            lines.push(format!("- {label}: {value}{unit}"));
        }
    }
    if let Some(force) = facts.get("force")
        && let (Some(kind), Some(value)) = (
            force.get("kind").and_then(Value::as_str),
            force.get("value"),
        )
    {
        let name = match kind {
            // MapleSEA calls sacred symbols Authentic.
            "sacred" => "Authentic Force",
            "arcane" => "Arcane Force",
            _ => "Force",
        };
        lines.push(format!("- {name}: {value}"));
    }
    if let Some(hp) = facts.get("hp").and_then(Value::as_array) {
        let values: Vec<String> = hp
            .iter()
            .filter_map(|row| {
                let mut value = format!(
                    "{} {}",
                    row.get("phase")?.as_str()?,
                    row.get("value")?.as_str()?
                );
                if let Some(count) = row.get("count").and_then(Value::as_u64) {
                    value.push_str(&format!(" ×{count}"));
                }
                if let Some(target) = row.get("target").and_then(Value::as_str) {
                    value.push_str(&format!(" ({target})"));
                }
                Some(value)
            })
            .collect();
        if !values.is_empty() {
            lines.push(format!("- HP: {}", values.join(", ")));
        }
    }
    if let Some(spec) = facts.get("recommended_spec")
        && let (Some(kind), Some(text)) = (
            spec.get("kind").and_then(Value::as_str),
            spec.get("text").and_then(Value::as_str),
        )
    {
        let figures: Vec<&str> = ["value", "basis"]
            .into_iter()
            .filter_map(|field| spec.get(field).and_then(Value::as_str))
            .collect();
        if figures.is_empty() {
            lines.push(format!("- Recommended ({kind}): {text}"));
        } else {
            lines.push(format!(
                "- Recommended ({kind}): {text} ({})",
                figures.join("; ")
            ));
        }
    }
    // A malformed mission is left out whole, like a malformed `hp` row.
    let mut quest = Vec::new();
    if let Some(details) = facts.get("mission")
        && mission(&mut quest, details).is_some()
    {
        lines.extend(quest);
    }
    if let Some(notes) = items(facts, "notes") {
        bullets(lines, notes);
    }
}

fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field)?.as_str()
}

fn tone(value: &Value) -> &'static str {
    match value.get("tone").and_then(Value::as_str) {
        Some("risk") => " (risky)",
        Some("safe") => " (safe)",
        _ => "",
    }
}

/// `## Mechanics`: each diagram block as plain lines, or `None` when a block
/// lacks a required part or has an unknown kind.
fn mechanics(lines: &mut Vec<String>, blocks: &[Value]) -> Option<()> {
    lines.extend([String::new(), "## Mechanics".to_owned()]);
    for block in blocks {
        lines.push(format!("### {}", text(block, "title")?));
        match block.get("kind")?.as_str()? {
            "ledger" => {
                for row in block.get("rows")?.as_array()? {
                    let direction = match row.get("direction") {
                        Some(direction) => format!(" ({})", favour(direction.as_str()?)?),
                        None => String::new(),
                    };
                    lines.push(format!(
                        "- {}: {}{direction}",
                        text(row, "label")?,
                        text(row, "value")?
                    ));
                }
            }
            "zones" => {
                for zone in block.get("zones")?.as_array()? {
                    let sub = text(zone, "sub").map_or_else(String::new, |sub| format!(": {sub}"));
                    lines.push(format!("- {}{sub}{}", text(zone, "name")?, tone(zone)));
                }
            }
            "scale" => {
                let bands = block.get("bands")?.as_array()?;
                let spans: Option<Vec<u64>> = bands
                    .iter()
                    .map(|band| band.get("span")?.as_u64())
                    .collect();
                let spans = spans?;
                let total: u64 = spans.iter().sum();
                for (band, span) in bands.iter().zip(spans) {
                    lines.push(format!(
                        "- {}: {span}/{total}{}",
                        text(band, "label")?,
                        tone(band)
                    ));
                }
            }
            _ => return None,
        }
        if let Some(note) = block.get("note") {
            lines.push(format!("- Note: {}", note.as_str()?));
        }
    }
    Some(())
}

/// `## Strategies`: each route with its trade-off and numbered steps, or
/// `None` when a strategy lacks a required part.
fn strategies(lines: &mut Vec<String>, strategies: &[Value]) -> Option<()> {
    lines.extend([String::new(), "## Strategies".to_owned()]);
    for strategy in strategies {
        let text = |field: &str| strategy.get(field).and_then(Value::as_str);
        lines.extend([
            format!("### {}", text("name")?),
            format!("- When: {}", text("when")?),
            format!(
                "- Risk: {}; damage needed: {}",
                text("risk")?,
                text("damage")?
            ),
            format!("- Payoff: {}", text("payoff")?),
            "- Steps:".to_owned(),
        ]);
        let steps = strings(strategy, "steps")?;
        lines.extend(
            steps
                .into_iter()
                .enumerate()
                .map(|(at, step)| format!("  {}. {step}", at + 1)),
        );
    }
    Some(())
}

/// One `### Name` block: v2 per-difficulty facts and the letter-keyed
/// `difficulty_notes` text v4 printed under the same heading.
struct Section<'a> {
    name: String,
    facts: Option<&'a Value>,
    note: Option<String>,
}

fn sections<'a>(document: &'a Value, catalog: &BossTable) -> Vec<Section<'a>> {
    let mut notes: Vec<(String, String)> = document
        .get("difficulty_notes")
        .and_then(Value::as_object)
        .map(|notes| {
            notes
                .iter()
                .filter_map(|(letter, note)| Some((catalog.difficulty_name(letter), item(note)?)))
                .collect()
        })
        .unwrap_or_default();
    let mut sections: Vec<Section<'a>> = document
        .get("difficulties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|facts| {
            let name = facts.get("name")?.as_str()?.to_owned();
            let note = notes
                .iter()
                .position(|(named, _)| *named == name)
                .map(|at| notes.remove(at).1);
            Some(Section {
                name,
                facts: Some(facts),
                note,
            })
        })
        .collect();
    sections.extend(notes.into_iter().map(|(name, note)| Section {
        name,
        facts: None,
        note: Some(note),
    }));
    sections
}

/// The guide for `reference` from its knowledge `document`, or `None` when
/// the document lacks a required part. `## Sources` is never included.
///
/// A catalog boss is headed by its full name; an event boss (not in the
/// catalog, with an `event` block) by its key and its availability.
pub fn render_guide(
    document: &Value,
    researched_as_of: &str,
    catalog: &BossTable,
    reference: &BossReference,
) -> Option<String> {
    let mut lines = match catalog.boss(&reference.short) {
        Some(boss) => vec![format!("# {} ({})", boss.full(), boss.short())],
        None => {
            let availability = document.get("event")?.get("availability")?.as_str()?;
            vec![
                format!("# {}", reference.short),
                format!("Availability: {availability}"),
            ]
        }
    };
    lines.extend([
        format!("_Researched as of {researched_as_of}._"),
        String::new(),
        document.get("summary")?.as_str()?.to_owned(),
    ]);
    let phases = document.get("phases");
    // With phases, `core` is optional (cross-phase items only).
    if document.get("core").is_some() || phases.is_none() {
        lines.extend([String::new(), "## Core".to_owned()]);
        bullets(&mut lines, items(document, "core")?);
    }
    if let Some(phases) = phases {
        lines.extend([String::new(), "## Phases".to_owned()]);
        for phase in phases.as_array()? {
            lines.push(phase_heading(phase)?);
            bullets(&mut lines, items(phase, "items")?);
        }
    }
    for (heading, field) in [("Danger", "danger"), ("Tips", "tips")] {
        lines.extend([String::new(), format!("## {heading}")]);
        bullets(&mut lines, items(document, field)?);
    }
    if let Some(blocks) = document.get("mechanics") {
        mechanics(&mut lines, blocks.as_array()?)?;
    }
    if let Some(routes) = document.get("strategies") {
        strategies(&mut lines, routes.as_array()?)?;
    }
    let wanted = reference
        .difficulty
        .as_deref()
        .map(|letter| catalog.difficulty_name(letter));
    let selected: Vec<Section<'_>> = sections(document, catalog)
        .into_iter()
        .filter(|section| wanted.as_ref().is_none_or(|name| *name == section.name))
        .collect();
    if !selected.is_empty() {
        lines.extend([String::new(), "## Difficulty notes".to_owned()]);
        for section in selected {
            lines.push(format!("### {}", section.name));
            if let Some(facts) = section.facts {
                difficulty_facts(&mut lines, facts);
            }
            lines.extend(section.note);
        }
    }
    if let Some(notes) = items(document, "notes").filter(|notes| !notes.is_empty()) {
        lines.extend([String::new(), "## Notes".to_owned()]);
        bullets(&mut lines, notes);
    }
    Some(lines.join("\n"))
}

/// `### Noon (Phase 2, repeating): gauge fills`; group and tag only when set.
fn phase_heading(phase: &Value) -> Option<String> {
    let mut heading = format!("### {}", phase.get("name")?.as_str()?);
    if let Some(group) = phase.get("group") {
        let cycle = match phase.get("cycle") {
            Some(cycle) => cycle.as_bool()?,
            None => false,
        };
        let repeat = if cycle { ", repeating" } else { "" };
        heading.push_str(&format!(" ({}{repeat})", group.as_str()?));
    }
    if let Some(tag) = phase.get("tag") {
        heading.push_str(&format!(": {}", tag.as_str()?));
    }
    Some(heading)
}
