//! Boundary-scanner rules on their own: code-owned exemptions (wire
//! vocabulary, prompts, the boss table), passthrough and test-codec grants.

use std::sync::Arc;

use kanade::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec};
use kanade::extract::prompt::SYSTEM_PROMPT;
use kanade::infrastructure::llm::{
    ChatRequest, Effort, Message, OutputSchema, OutputValidation, Sampling, ToolCallRequest,
    ToolDefinition,
    governor::XorShift,
    identity::{
        BotIdentity, CodeLexicon, LeakKind, Member, NamePool, Passthrough, PseudonymCodec,
        PseudonymConfig, ScanExemptions, TaggingCodec, open_session,
    },
};
use serde_json::json;

use super::identity::route;

fn member(user_id: &str, display_name: &str) -> Member {
    Member {
        user_id: user_id.into(),
        display_name: display_name.into(),
        nickname: None,
        aliases: Vec::new(),
    }
}

fn codec() -> PseudonymCodec {
    PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::builtin(),
        bot: BotIdentity {
            user_id: None,
            name: "Kanade".into(),
            aliases: vec!["Kanata".into()],
        },
        extra_exclusions: Vec::new(),
        random: Arc::new(XorShift::new(3)),
    })
}

fn chat(system: &str, user: &str) -> ChatRequest {
    ChatRequest {
        model: "Carling".into(),
        messages: vec![
            Message::System {
                content: system.into(),
            },
            Message::User {
                content: user.into(),
            },
        ],
        tools: Vec::new(),
        output_schema: None,
        max_output_tokens: 64,
        reasoning: None,
        sampling: None,
    }
}

fn catalog() -> BossTable {
    BossTable::from_spec(&CatalogSpec {
        difficulties: vec![DifficultySpec {
            prefix: "H".into(),
            label: "Hard".into(),
        }],
        bosses: vec![BossSpec {
            short: "Will".into(),
            full: Some("Will the Spider King".into()),
            aliases: vec!["spider".into()],
            ..BossSpec::default()
        }],
    })
    .expect("valid catalog")
}

#[test]
fn a_name_is_exempt_only_when_every_word_is_code_owned() {
    let exemptions = ScanExemptions::default()
        .with_boss_table(&catalog())
        .with_texts(["Dark Knight"]);
    for name in [
        "Will",
        "will",
        "HWill",
        "Spider King",
        "dark knight",
        "Hard",
    ] {
        assert!(exemptions.covers(name), "{name}");
    }
    for name in ["Ken", "Will Smith", "", "🌸", "Mai"] {
        assert!(!exemptions.covers(name), "{name}");
    }
    assert_eq!(
        format!("{exemptions:?}"),
        format!("ScanExemptions {{ words: {} }}", exemptions.len())
    );
    // Only what the runner itself adds; request structure is never scanned
    // and calendar words come from each role's real sources.
    let builtin = ScanExemptions::builtin();
    for word in ["output", "format", "schema", "no"] {
        assert!(builtin.covers(word), "{word}");
    }
    for word in ["Ken", "Will", "Alice", "Max", "Low", "User", "Sun", "May"] {
        assert!(!builtin.covers(word), "{word}");
    }
}

#[test]
fn wire_vocabulary_never_refuses_a_request() {
    let roster: Vec<Member> = [
        "User", "Tool", "System", "Content", "Low", "Model", "Strict",
    ]
    .iter()
    .enumerate()
    .map(|(i, name)| member(&format!("2000000000000000{i:02}"), name))
    .collect();
    let identity = open_session(&codec(), &route(false), &roster).unwrap();
    let request = ChatRequest {
        model: "local".into(),
        messages: vec![
            Message::System {
                content: "hi".into(),
            },
            Message::Assistant {
                content: None,
                tool_calls: vec![ToolCallRequest {
                    id: "call_1".into(),
                    name: "list_runs".into(),
                    arguments: "{}".into(),
                }],
            },
            Message::Tool {
                tool_call_id: "call_1".into(),
                content: String::new(),
            },
        ],
        tools: vec![ToolDefinition {
            name: "list_runs".into(),
            description: None,
            input_schema: json!({}),
        }],
        output_schema: Some(OutputSchema {
            name: "reply".into(),
            schema: json!({}),
            strict: true,
            validation: OutputValidation::CallerValidates,
        }),
        max_output_tokens: 64,
        reasoning: Some(Effort::Low),
        sampling: Some(Sampling {
            temperature: Some(0.2),
            seed: Some(0),
            top_p: None,
        }),
    };
    assert_eq!(identity.scanner().scan(&request), Ok(()));
}

#[test]
fn code_owned_prompt_text_is_exempt_once_supplied() {
    let roster = [member("200000000000000001", "Carling")];
    let request = chat(SYSTEM_PROMPT, "hi");

    let strict = open_session(&codec(), &route(false), &roster).unwrap();
    let found = strict.scanner().scan(&request).unwrap_err();
    assert_eq!(found.kinds, vec![LeakKind::Name]);
    assert!(strict.scanner().scan(&chat("x", "carling?")).is_err());
    // The route alias is config, never scanned.
    assert_eq!(strict.scanner().scan(&chat("x", "hi")), Ok(()));

    let tailored =
        codec().with_scan_exemptions(&ScanExemptions::default().with_texts([SYSTEM_PROMPT]));
    let identity = open_session(&tailored, &route(false), &roster).unwrap();
    assert_eq!(identity.scanner().scan(&request), Ok(()));
    // Documented residual: an unmasked name equal to a code-owned word.
    assert_eq!(identity.scanner().scan(&chat("x", "carling?")), Ok(()));
}

#[test]
fn bot_names_are_exempt_and_scanning_follows_codec_word_rules() {
    let roster = [
        member("200000000000000001", "Kanata"),
        member("200000000000000002", "Mai"),
    ];
    let identity = open_session(&codec(), &route(false), &roster).unwrap();
    let scanner = identity.scanner();
    assert_eq!(
        scanner.scan(&chat("You are Kanade (Kanata).", "hi")),
        Ok(())
    );
    for clean in ["Maid", "mail", "120000000000000000001", "1234567890123456"] {
        assert_eq!(scanner.scan(&chat("x", clean)), Ok(()), "{clean}");
    }
    for leaky in [
        "Mai!",
        // `_` separates words, like `-` in channel names.
        "Mai_x",
        "line\\nmai",
        "マイMai",
        "[\"mai\"]",
        "x200000000000000002",
    ] {
        assert!(scanner.scan(&chat("x", leaky)).is_err(), "{leaky}");
    }
}

#[test]
fn only_pseudonymizing_codecs_get_an_active_scanner() {
    let roster = [member("200000000000000001", "Alice")];
    let plain = open_session(&Passthrough, &route(false), &roster).unwrap();
    assert!(!plain.scanner().is_active());
    assert_eq!(plain.scanner().scan(&chat("Alice", "Alice")), Ok(()));

    let tagged = open_session(&TaggingCodec, &route(true), &roster).unwrap();
    assert!(tagged.scanner().is_active());
    let found = tagged
        .scanner()
        .scan(&chat("x", "alice 200000000000000001"))
        .unwrap_err();
    assert_eq!(
        (found.kinds, found.count),
        (vec![LeakKind::Name, LeakKind::Id], 2)
    );
}

/// Words of multi-word names are scanned too: an unencoded first name in a
/// channel name is refused; the tailored code-owned list still exempts a
/// word only where code text uses it.
#[test]
fn words_of_multi_word_names_are_scanned() {
    let roster = [member("200000000000000031", "Jonas lau")];
    let identity = open_session(&codec(), &route(false), &roster).unwrap();
    for leaky in ["#hbaldguy-jonas-cryz", "hstar_jonas", "ask LAU"] {
        let found = identity.scanner().scan(&chat("x", leaky)).unwrap_err();
        assert_eq!(found.kinds, vec![LeakKind::Name], "{leaky}");
    }
    let tailored = codec().with_scan_exemptions(&ScanExemptions::default().with_texts(["lau"]));
    let identity = open_session(&tailored, &route(false), &roster).unwrap();
    assert_eq!(identity.scanner().scan(&chat("x", "lau")), Ok(()));
    assert!(identity.scanner().scan(&chat("x", "jonas")).is_err());
}
