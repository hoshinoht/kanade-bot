//! Compiler round trips, provenance, reminder placement and fallback retention.

use std::collections::BTreeSet;

use kanade::{
    chat::{
        persona::{
            CompiledPersona, ExampleSource, PersonaSnapshot, PersonaStore, ProfileQuery,
            ProfileSource, ReloadError, RoleAssignment, RoleId, SelectionSource, VoiceSource,
            parse_bundle, parse_profile,
        },
        prompts,
    },
    infrastructure::llm::Message,
};
use serde_json::Value;

use crate::{
    oracle::{cases, turn},
    support::{Fixture, bundle, catalog, pid, prof, profile, tracked},
};

#[test]
fn clock_header_ends_the_boss_week_seven_wall_clock_days_later_across_dst() {
    use chrono::DateTime;
    use kanade::chat::persona::TurnContext;

    let header = |now: &str, week: &str| {
        let now = DateTime::parse_from_rfc3339(now).unwrap();
        let week = DateTime::parse_from_rfc3339(week).unwrap();
        TurnContext::new(&now, chrono_tz::Europe::London, &week, "", "")
            .header()
            .to_owned()
    };
    // BST ends 25 Oct 2026: the reset stays at 00:00 local, not 23:00.
    assert_eq!(
        header("2026-10-27T12:00:00Z", "2026-10-21T23:00:00Z"),
        "Right now it is Tuesday 27 October 2026, 12:00 (Europe/London). The calendar week is \
         Monday 26 October to Sunday 01 November. The current boss week runs from Thursday 22 \
         October 00:00 to Thursday 29 October 00:00. Unqualified 'this week' and 'next week' \
         mean calendar weeks."
    );
    // BST starts 29 Mar 2026: likewise 00:00 local, not 01:00.
    assert_eq!(
        header("2026-03-30T12:00:00Z", "2026-03-26T00:00:00Z"),
        "Right now it is Monday 30 March 2026, 13:00 (Europe/London). The calendar week is \
         Monday 30 March to Sunday 05 April. The current boss week runs from Thursday 26 March \
         00:00 to Thursday 02 April 00:00. Unqualified 'this week' and 'next week' mean \
         calendar weeks."
    );
}

/// Normative illustration from the standardize-prompt-format step, verbatim.
const TSUNDERE: &str = r#"# profiles/tsundere.yaml
schema_version: 1
id: tsundere
label: Tsundere
voice: "Flustered classic tsundere denial: sharp outside, visibly helpful underneath."
prompt: |
  Add a classic tsundere ... edge to every reply for this member while
  preserving the base persona.
  (... full delivery rules, owner rule, tone examples, accuracy rule ...)
staging:            # partial allowed — missing keys fall back to bundle default
  schedule: H-Hmph! I'm checking the boss board, okay?! Don't get the wrong idea!
  guide: F-Fine! I'll check the mechanics. It's not like I'm doing it for you!
  guide_named: '{boss}?! I already know that! ...'
  write: I'm updating the card because it looks messy, NOT because you asked!
  generic: I-I'm thinking! Stop staring at me!
"#;

fn kanade() -> kanade::chat::persona::Bundle {
    parse_bundle(&tracked("bundles/kanade.yaml"), &pid("kanade")).unwrap()
}

fn oracle_case(id: &str) -> Value {
    cases()
        .into_iter()
        .find(|case| case["case_id"] == id)
        .unwrap()
}

#[test]
fn tracked_kanade_compiles_to_its_components() {
    let bundle = kanade();
    let compiled = CompiledPersona::compile(&bundle, None);
    assert_eq!(compiled.identity(), bundle.identity.trim_end());
    assert_eq!(compiled.behaviour(), bundle.prompt.trim_end());
    assert_eq!(compiled.profile_prompt(), None);
    assert_eq!(compiled.profile_voice(), None);
    assert_eq!(compiled.effective_voice(), prompts::DEFAULT_VOICE);
    assert!(compiled.examples().is_empty());
    assert_eq!(
        compiled.prompt_compact(),
        bundle.compact.as_ref().unwrap().header_rewrite.as_deref()
    );
    assert!(!compiled.prompt().contains("Rewrite only the header line"));
    let scope = prompts::ASSISTANT_SCOPE
        .trim()
        .replace("{assistant_name}", "OtonoseKanade");
    assert!(compiled.prompt().starts_with(&format!(
        "{}\n\n{}\n\n{scope}\n\n",
        compiled.identity(),
        compiled.behaviour()
    )));
    assert!(
        compiled
            .prompt()
            .ends_with(prompts::BOSS_KNOWLEDGE_POLICY.trim())
    );
    let turn = turn(&oracle_case("bundle-default-with-focus")["input"]);
    assert!(compiled.system_prompt(&turn).starts_with(compiled.prompt()));
    assert_eq!(compiled, CompiledPersona::compile(&kanade(), None));
}

#[test]
fn normative_tsundere_profile_round_trips() {
    let style = parse_profile(TSUNDERE, &prof("tsundere")).unwrap();
    let compiled = CompiledPersona::compile(&kanade(), Some(&style));
    assert_eq!(
        compiled.effective_voice(),
        "Flustered classic tsundere denial: sharp outside, visibly helpful underneath."
    );
    assert_eq!(compiled.provenance().voice, VoiceSource::Profile);
    assert_eq!(
        compiled.profile_prompt(),
        Some(
            "Add a classic tsundere ... edge to every reply for this member while\n\
             preserving the base persona.\n\
             (... full delivery rules, owner rule, tone examples, accuracy rule ...)"
        )
    );
    let staging = compiled.staging_lines();
    assert_eq!(
        staging.schedule,
        "H-Hmph! I'm checking the boss board, okay?! Don't get the wrong idea!"
    );
    assert_eq!(staging.generic, "I-I'm thinking! Stop staring at me!");
    assert_eq!(
        staging.guide_named_for("Lotus"),
        "Lotus?! I already know that! ..."
    );
    let prompt = compiled.prompt();
    let behaviour = prompt.find(compiled.behaviour()).unwrap();
    let own = prompt.find("Add a classic tsundere").unwrap();
    let scope = prompt.find("# Assistant scope").unwrap();
    assert!(behaviour < own && own < scope);
    assert!(
        compiled
            .voice_reminder()
            .contains("visibly helpful underneath. Every reply")
    );
}

#[test]
fn voice_falls_back_from_profile_to_bundle_to_default() {
    let alpha = parse_bundle(&bundle("alpha", "Alpha"), &pid("alpha")).unwrap();
    let voiced = parse_profile(&profile("style", None), &prof("style")).unwrap();
    let silent = parse_profile(
        &profile("quiet", None).replace("voice: Synthetic quiet voice.\n", ""),
        &prof("quiet"),
    )
    .unwrap();
    let compile = |profile| CompiledPersona::compile(&alpha, profile);
    assert_eq!(
        compile(Some(&voiced)).effective_voice(),
        "Synthetic style voice."
    );
    assert_eq!(
        compile(Some(&silent)).effective_voice(),
        "Synthetic alpha voice."
    );
    assert_eq!(
        compile(Some(&silent)).provenance().voice,
        VoiceSource::Bundle
    );
    assert_eq!(compile(None).effective_voice(), "Synthetic alpha voice.");
    let unvoiced = parse_bundle(
        &bundle("alpha", "Alpha").replace("  voice: Synthetic alpha voice.\n", ""),
        &pid("alpha"),
    )
    .unwrap();
    let compiled = CompiledPersona::compile(&unvoiced, Some(&silent));
    assert_eq!(compiled.effective_voice(), prompts::DEFAULT_VOICE);
    assert!(
        compile(None)
            .prompt()
            .contains("Alpha is primarily the guild's")
    );
    assert_eq!(compile(None).provenance().examples, ExampleSource::None);
}

#[test]
fn provenance_and_persona_text_stay_out_of_model_messages_and_logs() {
    let fixture = Fixture::new();
    fixture.write("catalog.yaml", catalog("alpha", &[("alpha", &[])]));
    fixture.write("bundles/alpha.yaml", bundle("alpha", "Alpha"));
    fixture.write(
        "profiles/zeta-marker.yaml",
        "schema_version: 1\nid: zeta-marker\nlabel: Zeta\nprompt: Synthetic profile text.\n",
    );
    let snapshot = PersonaSnapshot::startup(&fixture.root(), None);
    let roles = [RoleId::new("role-zeta")];
    let assignments = [RoleAssignment {
        role: RoleId::new("role-zeta"),
        profile: prof("zeta-marker"),
    }];
    let selectable = BTreeSet::new();
    let query = ProfileQuery {
        member_roles: &roles,
        role_assignments: &assignments,
        saved_selection: None,
        selectable: &selectable,
    };
    let compiled = snapshot.resolve(&query).unwrap().compile();
    let provenance = compiled.provenance();
    assert_eq!(
        provenance.profile_source,
        Some(ProfileSource::RoleAssignment)
    );
    assert_eq!(provenance.profile, Some(prof("zeta-marker")));
    let bundle_file = provenance.bundle_file.clone().unwrap();
    let profile_file = provenance.profile_file.clone().unwrap();
    assert_eq!(bundle_file.basename, "alpha.yaml");
    assert_eq!(profile_file.basename, "zeta-marker.yaml");

    let turn = turn(&oracle_case("bundle-default-with-focus")["input"]);
    let messages = compiled.messages(
        &turn,
        [Message::User {
            content: "hi".into(),
        }],
    );
    let wire = serde_json::to_string(&messages).unwrap();
    for secret in [
        "zeta-marker",
        "role-zeta",
        "alpha.yaml",
        &bundle_file.sha256,
        &profile_file.sha256,
        "RoleAssignment",
        "Configured",
        "CatalogDefault",
    ] {
        assert!(!wire.contains(secret), "{secret}");
    }
    let debug = format!("{compiled:?}");
    assert!(!debug.contains("Synthetic identity"));
    assert!(!debug.contains("Synthetic profile"));
    assert!(!debug.contains("Operating"));
}

fn tool_round() -> Vec<Message> {
    vec![
        Message::User {
            content: "what's on tonight?".into(),
        },
        Message::Assistant {
            content: None,
            tool_calls: vec![kanade::infrastructure::llm::ToolCallRequest {
                id: "call-1".into(),
                name: "get_schedule".into(),
                arguments: "{}".into(),
            }],
        },
        Message::Tool {
            tool_call_id: "call-1".into(),
            content: "{\"runs\":[]}".into(),
        },
    ]
}

#[test]
fn the_voice_reminder_is_the_final_user_message_after_tool_results() {
    let case = oracle_case("profile-voice-examples-partial-staging");
    let (compiled, _) = crate::oracle::compile_case(&case);
    let turn = turn(&case["input"]);
    let messages = compiled.messages(&turn, tool_round());
    assert_eq!(messages.len(), 5);
    assert_eq!(
        messages[0],
        Message::System {
            content: case["expected"]["system_prompt"].as_str().unwrap().into()
        }
    );
    assert_eq!(messages[1..4], tool_round()[..]);
    assert_eq!(
        messages[4],
        Message::User {
            content: case["expected"]["voice_reminder"].as_str().unwrap().into()
        }
    );
}

#[cfg(feature = "test-support")]
#[tokio::test]
async fn fake_provider_captures_the_compiled_request() {
    use kanade::infrastructure::llm::{ChatRequest, FakeProvider, LlmProvider};

    let case = oracle_case("bundle-default-with-focus");
    let (compiled, _) = crate::oracle::compile_case(&case);
    let request = ChatRequest {
        model: "synthetic-model".into(),
        messages: compiled.messages(&turn(&case["input"]), tool_round()),
        tools: Vec::new(),
        output_schema: None,
        max_output_tokens: 256,
        reasoning: None,
        sampling: None,
    };
    let provider = FakeProvider::new([]);
    let _ = provider.complete(&request).await;
    let captured = provider.requests();
    assert_eq!(captured.len(), 1);
    let messages = &captured[0].messages;
    let Message::System { content } = &messages[0] else {
        panic!("system prompt first")
    };
    assert_eq!(content, case["expected"]["system_prompt"].as_str().unwrap());
    let Some(Message::User { content }) = messages.last() else {
        panic!("reminder last")
    };
    assert_eq!(
        content,
        case["expected"]["voice_reminder"].as_str().unwrap()
    );
}

const STRICT_BROKEN: &str = "  guide_named: '{boss} alpha guide'";

#[test]
fn trusted_fallback_compiles_to_the_oracle_kanade_prompt() {
    let fixture = Fixture::new();
    fixture.write(
        "catalog.yaml",
        catalog("alpha", &[("alpha", &[]), ("beta", &[])]),
    );
    // Both catalog candidates break only the strict staging rules.
    fixture.write(
        "bundles/alpha.yaml",
        bundle("alpha", "Alpha").replace(STRICT_BROKEN, "  guide_named: 'alpha guide'"),
    );
    fixture.write(
        "bundles/beta.yaml",
        bundle("beta", "Beta").replace("  generic: beta generic", "  generic: '@everyone wait'"),
    );
    let snapshot = PersonaSnapshot::startup(&fixture.root(), Some(&pid("beta")));
    assert_eq!(
        snapshot.provenance().source,
        Some(SelectionSource::TrackedFallback)
    );
    let case = oracle_case("bundle-default-with-focus");
    let selectable = BTreeSet::new();
    let query = ProfileQuery {
        member_roles: &[],
        role_assignments: &[],
        saved_selection: None,
        selectable: &selectable,
    };
    let compiled = snapshot.resolve(&query).unwrap().compile();
    assert_eq!(
        compiled.system_prompt(&turn(&case["input"])),
        case["expected"]["system_prompt"].as_str().unwrap()
    );
    assert_eq!(
        compiled.voice_reminder(),
        case["expected"]["voice_reminder"].as_str().unwrap()
    );
}

#[test]
fn failed_reload_retains_the_compiled_prompt() {
    let fixture = Fixture::new();
    fixture.write(
        "catalog.yaml",
        catalog("alpha", &[("alpha", &[]), ("beta", &[])]),
    );
    fixture.write("bundles/alpha.yaml", bundle("alpha", "Alpha"));
    fixture.write(
        "bundles/beta.yaml",
        format!(
            "{}nudges:\n  playful: ['see https://x.dev', b, c]\n",
            bundle("beta", "Beta")
        ),
    );
    let root = fixture.root();
    let store = PersonaStore::new(PersonaSnapshot::startup(&root, None));
    let selectable = BTreeSet::new();
    let query = ProfileQuery {
        member_roles: &[],
        role_assignments: &[],
        saved_selection: None,
        selectable: &selectable,
    };
    let turn = turn(&oracle_case("bundle-default-with-focus")["input"]);
    let before = store.pin().resolve(&query).unwrap().compile();
    let mut persisted = false;
    let result = store.reload(&root, &pid("beta"), |_| {
        persisted = true;
        Ok::<_, ()>(())
    });
    assert!(matches!(result, Err(ReloadError::Invalid(_))));
    assert!(!persisted);
    let after = store.pin().resolve(&query).unwrap().compile();
    assert_eq!(after.system_prompt(&turn), before.system_prompt(&turn));
    assert_eq!(after.provenance().bundle, pid("alpha"));
}

#[test]
fn invalid_profiles_are_ignored_whole() {
    let fixture = Fixture::new();
    fixture.write("catalog.yaml", catalog("alpha", &[("alpha", &[])]));
    fixture.write("bundles/alpha.yaml", bundle("alpha", "Alpha"));
    fixture.write("profiles/loud.yaml", profile("loud", Some("'ping @here'")));
    let snapshot = PersonaSnapshot::startup(&fixture.root(), None);
    let active = snapshot.active().unwrap();
    assert!(active.profiles.get(&prof("loud")).is_none());
    assert_eq!(active.profiles.unreadable.len(), 1);
    let selectable = BTreeSet::from([prof("loud")]);
    let query = ProfileQuery {
        member_roles: &[],
        role_assignments: &[],
        saved_selection: Some(&prof("loud")),
        selectable: &selectable,
    };
    let compiled = snapshot.resolve(&query).unwrap().compile();
    assert_eq!(compiled.profile_prompt(), None);
    assert_eq!(
        compiled.provenance().profile_source,
        Some(ProfileSource::BundleDefault {
            saved_selection_unavailable: true
        })
    );
    assert_eq!(compiled.staging_lines().generic, "alpha generic");
}
