//! Exact assembled-byte parity with the frozen v4 persona oracle.

use std::{fs, path::Path};

use chrono::DateTime;
use chrono_tz::Tz;
use kanade::chat::persona::{
    CompiledPersona, ExampleSource, Profile, Staging, TurnContext, VoiceSource,
};
use serde_json::Value;

use crate::support::{Fixture, pid, prof};

/// The explicit YAML voice each synthetic profile carries in v5. v4 scraped
/// `**Voice:**` from the Markdown; the v5 converter moves it into `voice`.
const VOICES: [(&str, Option<&str>); 5] = [
    (
        "profile-voice-examples-partial-staging",
        Some("Brisk and upbeat, like a synthetic test announcer."),
    ),
    ("profile-without-voice-or-examples", None),
    // Unfilled template slots are ignored, as v4 ignores them.
    (
        "profile-placeholder-voice-and-example",
        Some("<A short voice cue for this profile.>"),
    ),
    (
        "profile-example-budget-and-section-rules",
        Some("Rapid-fire synthetic commentary"),
    ),
    ("profile-example-count-limit", None),
];

pub fn cases() -> Vec<Value> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/v5/vectors/persona/persona.json");
    let document: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    document["cases"].as_array().unwrap().clone()
}

fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

/// A v5 profile file carrying the vector's Markdown as a block scalar.
pub fn profile_yaml(profile: &Value, voice: Option<&str>) -> String {
    let id = profile["id"].as_str().unwrap();
    let mut yaml = format!("schema_version: 1\nid: {id}\nlabel: Synthetic {id}\n");
    if let Some(voice) = voice {
        yaml += &format!("voice: {}\n", quoted(voice));
    }
    yaml += "prompt: |\n";
    for line in profile["markdown"].as_str().unwrap().split('\n') {
        if line.is_empty() {
            yaml.push('\n');
        } else {
            yaml += &format!("  {line}\n");
        }
    }
    if let Some(staging) = profile["staging"].as_object() {
        yaml += "staging:";
        if staging.is_empty() {
            yaml += " {}";
        }
        yaml.push('\n');
        for (key, line) in staging {
            yaml += &format!("  {key}: {}\n", quoted(line.as_str().unwrap()));
        }
    }
    yaml
}

pub fn turn(input: &Value) -> TurnContext {
    let clock = &input["clock"];
    let now = DateTime::parse_from_rfc3339(clock["now"].as_str().unwrap()).unwrap();
    let week = DateTime::parse_from_rfc3339(clock["week_start"].as_str().unwrap()).unwrap();
    let zone: Tz = clock["timezone"].as_str().unwrap().parse().unwrap();
    TurnContext::new(
        &now,
        zone,
        &week,
        input["model"].as_str().unwrap(),
        input["focus_card"].as_str().unwrap(),
    )
}

/// Load the tracked bundle and the case's profile through the real loader.
pub fn compile_case(case: &Value) -> (CompiledPersona, Option<Profile>) {
    let input = &case["input"];
    let fixture = Fixture::new();
    let root = fixture.root();
    let bundle = root
        .load_bundle(&pid(input["bundle_id"].as_str().unwrap()))
        .unwrap()
        .value;
    let profile = input["profile"].as_object().map(|_| {
        let id = case["case_id"].as_str().unwrap();
        let voice = VOICES
            .iter()
            .find(|(case_id, _)| *case_id == id)
            .unwrap_or_else(|| panic!("{id}: no v5 voice recorded"))
            .1;
        let profile = &input["profile"];
        let name = profile["id"].as_str().unwrap();
        fixture.write(
            &format!("profiles/{name}.yaml"),
            profile_yaml(profile, voice),
        );
        root.load_profile(&prof(name)).unwrap().value
    });
    (CompiledPersona::compile(&bundle, profile.as_ref()), profile)
}

fn staging_json(staging: &Staging) -> Value {
    serde_json::json!({
        "schedule": staging.schedule,
        "guide": staging.guide,
        "guide_named": staging.guide_named,
        "write": staging.write,
        "generic": staging.generic,
    })
}

#[test]
fn compiled_prompts_match_the_v4_oracle_bytes() {
    let cases = cases();
    let mut replayed = 0;
    for case in &cases {
        let id = case["case_id"].as_str().unwrap();
        let expected = &case["expected"];
        let (compiled, _) = compile_case(case);
        let turn = turn(&case["input"]);
        assert_eq!(turn.header(), expected["header"].as_str().unwrap(), "{id}");
        assert_eq!(
            turn.runtime(),
            expected["runtime"].as_str().unwrap(),
            "{id}"
        );
        assert_eq!(turn.focus(), expected["focus"].as_str().unwrap(), "{id}");
        assert_eq!(
            compiled.system_prompt(&turn),
            expected["system_prompt"].as_str().unwrap(),
            "{id}"
        );
        assert_eq!(
            compiled.voice_reminder(),
            expected["voice_reminder"].as_str().unwrap(),
            "{id}"
        );
        assert_eq!(
            staging_json(compiled.staging_lines()),
            expected["staging"],
            "{id}"
        );
        replayed += 1;
    }
    assert_eq!(replayed, cases.len());
    let profiled = cases
        .iter()
        .filter(|case| case["input"]["profile"].is_object());
    assert_eq!(profiled.count(), VOICES.len());
}

#[test]
fn oracle_cases_exercise_each_voice_and_example_source() {
    let mut seen = Vec::new();
    for case in cases() {
        let (compiled, _) = compile_case(&case);
        let provenance = compiled.provenance();
        seen.push((
            case["case_id"].as_str().unwrap().to_owned(),
            provenance.voice,
            provenance.examples,
        ));
    }
    let find = |id: &str| seen.iter().find(|(case, ..)| case == id).unwrap().clone();
    assert_eq!(
        find("bundle-default-unnamed-model"),
        (
            "bundle-default-unnamed-model".into(),
            VoiceSource::Default,
            ExampleSource::None
        )
    );
    let voiced = find("profile-voice-examples-partial-staging");
    assert_eq!(
        (voiced.1, voiced.2),
        (VoiceSource::Profile, ExampleSource::Profile)
    );
    let placeholder = find("profile-placeholder-voice-and-example");
    assert_eq!(
        (placeholder.1, placeholder.2),
        (VoiceSource::Default, ExampleSource::None)
    );
}

#[test]
fn profile_examples_replace_defaults_within_the_budgets() {
    let cases = cases();
    let case = |id: &str| {
        let case = cases.iter().find(|case| case["case_id"] == id).unwrap();
        compile_case(case).0
    };
    let voiced = case("profile-voice-examples-partial-staging");
    assert_eq!(
        voiced.examples(),
        [
            "Synthetic run is at 21:00. Card's up, go react!",
            "Nothing on tonight. Rest up!"
        ]
    );
    let busy = case("profile-example-budget-and-section-rules");
    assert!(
        busy.examples()
            .iter()
            .all(|example| !example.contains("ignored"))
    );
    assert!(
        busy.examples()
            .iter()
            .map(|e| e.chars().count())
            .sum::<usize>()
            <= 600
    );
    assert_eq!(busy.examples()[0], "First section one.");
    assert!(busy.examples()[1].starts_with("Second section one"));
    assert_eq!(case("profile-example-count-limit").examples().len(), 8);
}
