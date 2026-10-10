use kanade::extract::gate::{self, BossHit, BossLexicon, GateResult};
use serde_json::{Value, json};

use crate::support::{
    Deviation, Outcome, catalog, flag, replay_family_with, strings, text, unknown_op,
};

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
        // D-NO-RSVP-SCAN: v5 has no text scan for answers; none is ever found.
        "explicit_rsvp" => Value::Null,
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

/// D-NO-RSVP-SCAN (user decision 2026-10-08): planning no longer adds an
/// `rsvp` for a short chat line the model did not report ("the HFA no run
/// made btw" read as "no"), so the scan is gone and finds nothing.
fn no_rsvp_scan(value: &mut Value) -> usize {
    assert!(
        matches!(value.as_str(), Some("yes" | "no")),
        "frozen v4 answer, got {value}"
    );
    *value = Value::Null;
    1
}

#[test]
fn gate_vectors_replay_exactly() {
    // Steps whose frozen v4 scan found an answer; the rest were already null.
    let deviations = [0, 1, 2, 6, 7, 9].map(|step| Deviation {
        name: "D-NO-RSVP-SCAN",
        case_id: "explicit-rsvp-answers",
        step,
        rewrite: no_rsvp_scan,
    });
    assert_eq!(
        replay_family_with("gate", &deviations, |_, _| {}, replay),
        (6, 72)
    );
}

/// Named known difference K-WORD-MARKS: Rust `regex`'s `\w` admits combining
/// marks such as the emoji variation selector U+FE0F, Python's (`isalnum`)
/// does not, so no `\b` falls between `❤️` and the digits. v4 finds `930`
/// here; v5 finds nothing. Plain emoji and custom-emoji markup are unaffected.
#[test]
fn known_difference_marks_are_word_characters() {
    assert_eq!(gate::find_times("❤️930"), Vec::<String>::new());
    assert_eq!(gate::find_times("❤ 930"), ["930"]);
    assert_eq!(gate::find_times("🔥930"), ["930"]);
}
