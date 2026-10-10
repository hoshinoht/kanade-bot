//! Replays `cards.json` (v4 `bot.agent.formatting` card text and
//! `Pipeline._unanswered`) through `kanade::bot::cards`.

use std::collections::BTreeMap;

use kanade::bot::cards::{self, Audience};
use kanade::domain::proposals::CardDetails;
use kanade::domain::schedule::Run;
use serde_json::{Value, json};

use crate::support::{Outcome, replay_family, run, strings, text, unknown_op, zone};

fn row(value: &Value) -> CardDetails {
    CardDetails::from_json(value).unwrap_or_else(|| panic!("card row {value}"))
}

fn rows(value: &Value) -> Vec<CardDetails> {
    value.as_array().expect("rows").iter().map(row).collect()
}

fn runs(value: &Value) -> Vec<Run> {
    value.as_array().expect("runs").iter().map(run).collect()
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

fn replay(input: &Value, step: &Value) -> Outcome {
    let tz = zone(input);
    let value = match text(&step["op"]) {
        "when_text" => json!(cards::when_text(&row(&step["row"]), tz)),
        "proposal_line" => {
            let run = (!step["run"].is_null()).then(|| run(&step["run"]));
            let who = audience(&step["audience"]);
            let (name, value) =
                cards::proposal_line(&row(&step["row"]), run.as_ref(), tz, who.as_ref());
            json!([name, value])
        }
        "card_kind" => {
            let details = rows(&step["amendments"]);
            let refs: Vec<&CardDetails> = details.iter().collect();
            json!(cards::card_kind(&refs).as_str())
        }
        "unanswered" => {
            let details = rows(&step["amendments"]);
            let refs: Vec<&CardDetails> = details.iter().collect();
            let runs = runs(&step["runs"]);
            let run_refs: Vec<&Run> = runs.iter().collect();
            json!(cards::unanswered(&refs, &run_refs))
        }
        "proposal_card" => {
            let details = rows(&step["amendments"]);
            let refs: Vec<&CardDetails> = details.iter().collect();
            let runs = runs(&step["runs"]);
            let by_id: BTreeMap<String, &Run> =
                runs.iter().map(|run| (run.id.clone(), run)).collect();
            let waiting = (!step["unanswered"].is_null()).then(|| strings(&step["unanswered"]));
            let who = audience(&step["audience"]);
            let card = cards::proposal_card(
                &refs,
                &by_id,
                tz,
                waiting.as_deref(),
                step["confidence"].as_f64(),
                who.as_ref(),
            );
            json!({
                "content": card.content,
                "title": card.title,
                "description": card.description,
                "fields": card.fields.iter().map(|(n, v)| json!([n, v])).collect::<Vec<_>>(),
                "footer": card.footer,
                "colour": card.colour,
                "mention_users": card.mention_users,
            })
        }
        "notices" => {
            let name = text(&step["name"]);
            json!({
                "applied": cards::applied_notice(name),
                "rejected": cards::rejected_notice(name),
                "superseded": cards::SUPERSEDED_NOTICE,
                "confirm_hint": cards::confirm_hint(),
                "tbd": cards::TBD,
            })
        }
        other => unknown_op("cards", other),
    };
    Ok(value)
}

#[test]
fn card_vectors_replay_exactly() {
    assert_eq!(replay_family("cards", replay), (4, 35));
}
