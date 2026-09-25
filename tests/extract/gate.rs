use kanade::extract::gate::{self, BossHit, BossLexicon, GateResult};
use serde_json::{Value, json};

use crate::support::{Outcome, catalog, flag, replay_family, strings, text, unknown_op};

fn hit_json(hit: &BossHit) -> Value {
    json!({
        "token": hit.token,
        "short": hit.short,
        "difficulty": hit.difficulty,
        "canonical": hit.canonical,
        "fuzzy": hit.fuzzy,
    })
}

fn result_json(result: &GateResult) -> Value {
    json!({
        "signals": result.signals.iter().map(|signal| signal.as_str()).collect::<Vec<_>>(),
        "bosses": result.bosses.iter().map(hit_json).collect::<Vec<_>>(),
        "times": result.times,
        "days": result.days,
        "mentions": result.mentions,
        "hit": result.hit(),
        "strong": result.strong(),
        "reasons": result.reasons(),
    })
}

fn replay(input: &Value, step: &Value) -> Outcome {
    let table = catalog(&input["catalog"]);
    let lexicon = BossLexicon::new(&table);
    let roster = strings(&input["roster_ids"]);
    let evaluate = |text: &str| gate::evaluate(text, &lexicon, &roster);
    let value = match text(&step["op"]) {
        "find_bosses" => {
            let hits = gate::find_bosses(text(&step["text"]), &lexicon);
            json!(hits.iter().map(hit_json).collect::<Vec<_>>())
        }
        "canonical_bosses" => json!(gate::canonical_bosses(&gate::find_bosses(
            text(&step["text"]),
            &lexicon
        ))),
        "find_times" => json!(gate::find_times(text(&step["text"]))),
        "find_days" => json!(gate::find_days(text(&step["text"]))),
        "find_mentions" => json!(gate::find_mentions(
            text(&step["text"]),
            &strings(&step["roster_ids"])
        )),
        "explicit_rsvp" => {
            json!(gate::explicit_rsvp(text(&step["text"])).map(|state| state.as_str()))
        }
        "evaluate" => result_json(&evaluate(text(&step["text"]))),
        "should_extract" => {
            let burst: Vec<GateResult> = strings(&step["texts"])
                .iter()
                .map(|text| evaluate(text))
                .collect();
            json!(gate::should_extract(
                &burst,
                flag(&step["context_is_scheduling"])
            ))
        }
        "urgent" => json!(gate::urgent(&evaluate(text(&step["text"])))),
        other => unknown_op("gate", other),
    };
    Ok(value)
}

#[test]
fn gate_vectors_replay_exactly() {
    assert_eq!(replay_family("gate", replay), (6, 72));
}
