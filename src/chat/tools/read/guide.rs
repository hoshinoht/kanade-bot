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
                Some(format!(
                    "{} {}",
                    row.get("phase")?.as_str()?,
                    row.get("value")?.as_str()?
                ))
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
        lines.push(format!("- Recommended ({kind}): {text}"));
    }
    if let Some(notes) = strings(facts, "notes") {
        lines.extend(notes.into_iter().map(|note| format!("- {note}")));
    }
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
    note: Option<&'a str>,
}

fn sections<'a>(document: &'a Value, catalog: &BossTable) -> Vec<Section<'a>> {
    let mut notes: Vec<(String, &str)> = document
        .get("difficulty_notes")
        .and_then(Value::as_object)
        .map(|notes| {
            notes
                .iter()
                .filter_map(|(letter, note)| {
                    Some((catalog.difficulty_name(letter), note.as_str()?))
                })
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
    for (heading, field) in [("Core", "core"), ("Danger", "danger"), ("Tips", "tips")] {
        lines.extend([String::new(), format!("## {heading}")]);
        lines.extend(
            strings(document, field)?
                .into_iter()
                .map(|bullet| format!("- {bullet}")),
        );
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
            lines.extend(section.note.map(str::to_owned));
        }
    }
    if let Some(notes) = strings(document, "notes").filter(|notes| !notes.is_empty()) {
        lines.extend([String::new(), "## Notes".to_owned()]);
        lines.extend(notes.into_iter().map(|note| format!("- {note}")));
    }
    Some(lines.join("\n"))
}
