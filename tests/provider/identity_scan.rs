//! Boundary-scanner rules on their own: code-owned exemptions (wire
//! vocabulary, prompts, the boss table), passthrough and test-codec grants.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use kanade::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec};
use kanade::extract::prompt::SYSTEM_PROMPT;
use kanade::infrastructure::llm::{
    Capability, CapabilityFuture, ChatRequest, CompletionFuture, CompletionResponse, Effort,
    ExecutionLimits, FinishReason, LlmProvider, Message, ModelCapabilities, OutputSchema,
    OutputValidation, ProviderFailure, ProviderFailureKind, RetryPolicy, Sampling, ToolCallRequest,
    ToolDefinition,
    governor::XorShift,
    governor::{
        Governor, GovernorConfig, GovernorPolicy, GroupConfig, ModelClient, QuestionLimits, Role,
        RoleConfig, SessionFailure,
    },
    identity::{
        BotIdentity, CodeLexicon, CodecMode, DecodeError, IdentityCodec, IdentitySession, LeakKind,
        Member, NamePool, Passthrough, PseudonymCodec, PseudonymConfig, ScanExemptions, ScanName,
        ScanNeedles, TaggingCodec, open_session,
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

#[test]
fn masked_url_guard_scans_text_json_tool_fields_and_schemas() {
    const URL: &str = "https://example.invalid/host/path?query=one#fragment";
    let prefixed = format!("prefix{URL}");
    let identity = open_session(&codec(), &route(false), &[]).unwrap();
    for content in [
        URL,
        prefixed.as_str(),
        r#"{"link":"prefixhttps:\/\/example.invalid/host/path?query=one#fragment"}"#,
    ] {
        let found = identity.scanner().scan(&chat("x", content)).unwrap_err();
        assert_eq!(found.kinds, vec![LeakKind::Url], "{content}");
    }

    let mut request = chat("x", "safe");
    request.messages.push(Message::Assistant {
        content: None,
        tool_calls: vec![ToolCallRequest {
            id: "call_2".into(),
            name: "open_link".into(),
            arguments: json!({"link": prefixed}).to_string(),
        }],
    });
    request.tools.push(ToolDefinition {
        name: "open_link".into(),
        description: Some(prefixed.clone()),
        input_schema: json!({URL: prefixed}),
    });
    request.output_schema = Some(OutputSchema {
        name: "reply".into(),
        schema: json!({"description": prefixed}),
        strict: true,
        validation: OutputValidation::CallerValidates,
    });
    let found = identity.scanner().scan(&request).unwrap_err();
    assert_eq!(found.kinds, vec![LeakKind::Url]);
    assert!(
        found.count >= 5,
        "tool args, description, key, value and schema"
    );

    let mut capabilities = ModelCapabilities::minimal();
    capabilities.function_tools = true;
    let shaped = kanade::infrastructure::llm::shape_request(&request, &capabilities)
        .unwrap()
        .expect("schema instruction reshapes request");
    assert_eq!(
        identity.scanner().scan(&shaped).unwrap_err().kinds,
        vec![LeakKind::Url]
    );

    let scanner = identity.scanner();
    let echoed_url = format!("prefix{URL}");
    scanner.echo(&CompletionResponse {
        model: "Carling".into(),
        content: Some(echoed_url.clone()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    });
    let mut echoed = chat("x", "safe");
    echoed.messages.push(Message::Assistant {
        content: Some(echoed_url.clone()),
        tool_calls: Vec::new(),
    });
    assert_eq!(
        scanner.scan(&echoed).unwrap_err().kinds,
        vec![LeakKind::Url]
    );

    let plain = open_session(&Passthrough, &route(false), &[]).unwrap();
    assert_eq!(plain.scanner().scan(&chat("x", URL)), Ok(()));
    assert_eq!(plain.scanner().scan(&chat("x", &prefixed)), Ok(()));
}

#[test]
fn issued_link_tokens_do_not_collide_with_punctuation_only_names() {
    struct ZeroRandom;
    impl kanade::infrastructure::llm::governor::Random for ZeroRandom {
        fn next_u64(&self) -> u64 {
            0
        }
    }

    let codec = PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::builtin(),
        bot: BotIdentity::default(),
        extra_exclusions: Vec::new(),
        random: Arc::new(ZeroRandom),
    });
    let roster = [member("200000000000000099", "!!")];
    let mut identity = open_session(&codec, &route(false), &roster).unwrap();
    let token = identity.text("https://example.invalid/private");
    assert_eq!(identity.scanner().scan(&chat("x", &token)), Ok(()));
}

struct ReshapeCodec(Arc<AtomicUsize>);

impl IdentityCodec for ReshapeCodec {
    fn mode(&self) -> CodecMode {
        CodecMode::Pseudonymizing
    }

    fn open(&self, roster: &[Member]) -> Box<dyn IdentitySession> {
        Box::new(ReshapeSession {
            scans: Arc::clone(&self.0),
            inner: Passthrough.open(roster),
        })
    }
}

struct ReshapeSession {
    scans: Arc<AtomicUsize>,
    inner: Box<dyn IdentitySession>,
}

impl IdentitySession for ReshapeSession {
    fn author_label(&mut self, user_id: &str, name: &str) -> String {
        self.inner.author_label(user_id, name)
    }

    fn member_ref(&mut self, user_id: &str) -> String {
        self.inner.member_ref(user_id)
    }

    fn text(&mut self, text: &str) -> String {
        self.inner.text(text)
    }

    fn tool_result(&mut self, content: &str) -> String {
        self.inner.tool_result(content)
    }

    fn participant_enum(&self) -> Option<Vec<String>> {
        self.inner.participant_enum()
    }

    fn decode_ref(&self, value: &str) -> Result<String, DecodeError> {
        self.inner.decode_ref(value)
    }

    fn decode_json(&self, json: &str) -> Result<String, DecodeError> {
        self.inner.decode_json(json)
    }

    fn decode_reply(&self, text: &str) -> Result<String, DecodeError> {
        self.inner.decode_reply(text)
    }

    fn scan_needles(&self) -> Option<ScanNeedles> {
        let name = (self.scans.fetch_add(1, Ordering::SeqCst) > 0).then(|| ScanName {
            text: "Leakyname".into(),
            collides: false,
            token_clash: false,
        });
        Some(ScanNeedles {
            names: name.into_iter().collect(),
            ..ScanNeedles::default()
        })
    }

    fn masks(&self) -> bool {
        true
    }
}

#[derive(Default)]
struct RejectStructuredOutputOnce(AtomicUsize);

impl LlmProvider for RejectStructuredOutputOnce {
    fn complete(&self, _request: &ChatRequest) -> CompletionFuture<'_> {
        Box::pin(async {
            Err(ProviderFailure {
                kind: ProviderFailureKind::Permanent,
                reason_code: "unexpected-unshaped-call",
            })
        })
    }

    fn capabilities<'a>(
        &'a self,
        _model: &'a str,
        _deadline: tokio::time::Instant,
    ) -> CapabilityFuture<'a> {
        let mut capabilities = ModelCapabilities::minimal();
        capabilities.structured_output = true;
        Box::pin(async move { Some(capabilities) })
    }

    fn complete_with(
        &self,
        request: &ChatRequest,
        _capabilities: &ModelCapabilities,
    ) -> CompletionFuture<'_> {
        let call = self.0.fetch_add(1, Ordering::SeqCst);
        let model = request.model.clone();
        Box::pin(async move {
            if call == 0 {
                Err(ProviderFailure {
                    kind: ProviderFailureKind::CapabilityRejected(Capability::StructuredOutput),
                    reason_code: "structured-output",
                })
            } else {
                Ok(CompletionResponse {
                    model,
                    content: Some("{}".into()),
                    tool_calls: Vec::new(),
                    finish_reason: FinishReason::Stop,
                    usage: None,
                })
            }
        })
    }
}

#[tokio::test]
async fn runner_rescans_reshaped_attempts_before_admission() {
    let config = GovernorConfig {
        groups: vec![GroupConfig {
            name: "group".into(),
            backend: "local".into(),
            permits: 1,
            requests_per_min: 6_000,
            burst: Some(100),
            aliases: vec!["model".into()],
        }],
        roles: [(
            Role::Chat,
            RoleConfig {
                alias: "model".into(),
                external: false,
            },
        )]
        .into_iter()
        .collect(),
        policy: GovernorPolicy::default(),
    };
    let governor = Arc::new(Governor::new(&config, Arc::new(XorShift::new(41))).expect("governor"));
    let provider = Arc::new(RejectStructuredOutputOnce::default());
    let client = ModelClient::new(
        governor.clone(),
        provider.clone(),
        ExecutionLimits::default(),
        RetryPolicy {
            total_deadline: Duration::from_secs(20),
            max_attempts: 3,
            backoff: Duration::from_millis(1),
        },
    )
    .unwrap()
    .with_masking(true);
    let scans = Arc::new(AtomicUsize::new(0));
    let identity = open_session(
        &ReshapeCodec(Arc::clone(&scans)),
        &governor.route(Role::Chat).unwrap(),
        &[],
    )
    .unwrap();
    let route = governor.route(Role::Chat).unwrap();
    let mut session = client
        .open_question_on(
            &route,
            "reshape-test",
            false,
            QuestionLimits::new(Duration::from_secs(10)),
        )
        .await
        .expect("chat permit")
        .with_scanner(identity.scanner());
    let mut request = chat("x", "Leakyname");
    request.model = "model".into();
    request.output_schema = Some(OutputSchema {
        name: "reply".into(),
        schema: json!({"type": "object"}),
        strict: true,
        validation: OutputValidation::Runner,
    });

    let error = session.complete(&request).await.unwrap_err();
    assert!(matches!(
        error.failure,
        SessionFailure::IdentityLeakBlocked(ref blocked) if blocked.kinds == [LeakKind::Name]
    ));
    assert_eq!(provider.0.load(Ordering::SeqCst), 1);
    assert_eq!(scans.load(Ordering::SeqCst), 2);
    assert_eq!(
        session.requests_used(),
        1,
        "blocked reshape was not admitted"
    );
}
