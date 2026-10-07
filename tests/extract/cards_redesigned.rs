//! Replays `cards_redesigned.json` (v5 only, no v4 oracle) through
//! `kanade::bot::cards::styled_card`: the redesigned proposal card open,
//! applied, rejected, superseded and out of date, every change kind, and
//! multi-change cards. Classic cards stay on `cards.json`.

use std::collections::BTreeMap;

use chrono::Utc;
use kanade::bot::cards::{Audience, CardState, Closure, Look, styled_card};
use kanade::bot::delivery::cards::DifficultyMarks;
use kanade::domain::proposals::CardDetails;
use kanade::domain::schedule::Run;
use serde_json::{Value, json};

use crate::support::{
    Outcome, catalog, instant, replay_family, run, strings, text, unknown_op, zone,
};

fn rows(value: &Value) -> Vec<CardDetails> {
    value
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| CardDetails::from_json(row).unwrap_or_else(|| panic!("card row {row}")))
        .collect()
}

fn audience(value: &Value) -> Option<Audience> {
    if value.is_null() {
        return None;
    }
    let names = value["names"]
        .as_object()
        .expect("names")
        .iter()
        .map(|(id, name)| (id.clone(), text(name).to_owned()))
        .collect();
    Some(Audience {
        names,
        mentioned: strings(&value["mentioned"]),
    })
}

fn closure(value: &Value) -> Closure {
    let by = || text(&value["by"]).to_owned();
    let at = || instant(&value["at"]).with_timezone(&Utc);
    match text(&value["kind"]) {
        "applied" => Closure::Applied { by: by(), at: at() },
        "rejected" => Closure::Rejected { by: by(), at: at() },
        "superseded" => Closure::Superseded,
        "stale" => Closure::Stale,
        other => panic!("unknown closure {other}"),
    }
}

fn replay(input: &Value, step: &Value) -> Outcome {
    let table = catalog(&input["catalog"]);
    let value = match text(&step["op"]) {
        "styled_card" => {
            let details = rows(&step["amendments"]);
            let refs: Vec<&CardDetails> = details.iter().collect();
            let runs: Vec<Run> = step["runs"]
                .as_array()
                .expect("runs")
                .iter()
                .map(run)
                .collect();
            let by_id: BTreeMap<String, &Run> =
                runs.iter().map(|run| (run.id.clone(), run)).collect();
            let waiting = (!step["unanswered"].is_null()).then(|| strings(&step["unanswered"]));
            let who = audience(&step["audience"]);
            let mut marks = DifficultyMarks::new();
            for (letter, markup) in step["marks"].as_object().expect("marks") {
                marks.insert(letter, text(markup));
            }
            let state = &step["state"];
            let closures: Vec<Closure> = state["closures"]
                .as_array()
                .expect("closures")
                .iter()
                .map(closure)
                .collect();
            let notes = strings(&state["notes"]);
            let card = styled_card(
                &refs,
                &by_id,
                Look {
                    zone: zone(input),
                    catalog: Some(&table),
                    marks: &marks,
                    who: who.as_ref(),
                },
                waiting.as_deref(),
                step["confidence"].as_f64(),
                CardState {
                    closures: &closures,
                    open: state["open"].as_bool().expect("open"),
                    notes: &notes,
                },
            );
            json!({
                "content": card.content,
                "description": card.description,
                "fields": card
                    .fields
                    .iter()
                    .map(|field| json!([field.name, field.value, field.inline]))
                    .collect::<Vec<_>>(),
                "footer": card.footer,
                "colour": card.colour,
                "mention_users": card.mention_users,
            })
        }
        other => unknown_op("cards_redesigned", other),
    };
    if std::env::var_os("KANADE_PRINT_GOLDEN").is_some() {
        println!("GOLDEN {value}");
    }
    Ok(value)
}

#[test]
fn redesigned_card_vectors_replay_exactly() {
    assert_eq!(replay_family("cards_redesigned", replay), (8, 19));
}
