//! Provider-boundary identity-leak scanner on governed sessions: a leaky
//! request is refused before any send, spends no request, rate token or retry,
//! is never retried, and retry/requeue attempts are scanned again.

use std::{sync::Arc, time::Duration};

use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, FakeAction, FakeProvider, FinishReason, Message, RetryPolicy,
    Usage,
    governor::{
        Charge, Governor, GovernorConfig, ModelClient, QuestionLimits, Role, SessionError,
        SessionFailure, XorShift,
    },
    identity::{
        BotIdentity, CodeLexicon, CodecMode, DecodeError, IdentityCodec, IdentityGrant,
        IdentityLeakBlocked, IdentitySession, LeakKind, Member, NamePool, Passthrough,
        PseudonymCodec, PseudonymConfig, ScanExemptions, open_session,
    },
};

use crate::support::{ALIAS, build, single, snap};

const BOT_ID: &str = "100000000000000001";
const ALICE_ID: &str = "200000000000000001";
const KEN_ID: &str = "200000000000000002";
const WILL_ID: &str = "200000000000000003";
const STRANGER_ID: &str = "200000000000000009";
const STRAY: &str = "123456789012345678";

fn member(user_id: &str, display_name: &str, aliases: &[&str]) -> Member {
    Member {
        user_id: user_id.into(),
        display_name: display_name.into(),
        nickname: None,
        aliases: aliases.iter().map(|&a| a.into()).collect(),
    }
}

fn roster() -> Vec<Member> {
    vec![
        member(BOT_ID, "Kanade", &[]),
        member(ALICE_ID, "Alice", &["Ally"]),
        member(KEN_ID, "Ken", &[]),
        member(WILL_ID, "Will", &[]),
    ]
}

fn codec() -> PseudonymCodec {
    PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::builtin(),
        bot: BotIdentity {
            user_id: Some(BOT_ID.into()),
            name: "Kanade".into(),
            aliases: Vec::new(),
        },
        extra_exclusions: Vec::new(),
        random: Arc::new(XorShift::new(7)),
    })
}

/// Code-owned prompt text that says "Will" (a boss) but never "Ken".
const CODE_TEXT: &str = "BOSSES\n  NWill, HWill  = Will\nYou will read the chat.";

fn exempting_codec() -> PseudonymCodec {
    codec().with_scan_exemptions(&ScanExemptions::default().with_texts([CODE_TEXT]))
}

fn request(system: &str, user: &str) -> ChatRequest {
    ChatRequest {
        model: ALIAS.into(),
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
        max_output_tokens: 16,
        reasoning: None,
        sampling: None,
    }
}

fn ok() -> FakeAction {
    FakeAction::Response(CompletionResponse {
        model: ALIAS.into(),
        content: Some("ok".into()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: Some(Usage {
            prompt_tokens: 1,
            completion_tokens: 1,
        }),
    })
}

fn config() -> GovernorConfig {
    let mut config = single(1, 6_000);
    config.groups[0].burst = Some(1_000);
    config.policy.retry_floor = 5;
    config
}

fn setup(
    actions: impl IntoIterator<Item = FakeAction>,
) -> (Arc<Governor>, Arc<FakeProvider>, ModelClient<FakeProvider>) {
    let governor = build(&config());
    let provider = Arc::new(FakeProvider::new(actions));
    let retry = RetryPolicy {
        total_deadline: Duration::from_secs(30),
        max_attempts: 3,
        backoff: Duration::from_millis(100),
    };
    let client = ModelClient::new(
        governor.clone(),
        provider.clone(),
        Default::default(),
        retry,
    )
    .expect("valid client");
    (governor, provider, client)
}

fn grant(governor: &Governor, codec: &dyn IdentityCodec, role: Role) -> IdentityGrant {
    let route = governor.route(role).expect("role routed");
    open_session(codec, &route, &roster()).expect("local route")
}

fn limits() -> QuestionLimits {
    QuestionLimits {
        tool_rounds: 4,
        timeout: Duration::from_secs(60),
    }
}

fn blocked(error: &SessionError) -> &IdentityLeakBlocked {
    assert_eq!(error.charge, Charge::Refunded);
    match &error.failure {
        SessionFailure::IdentityLeakBlocked(blocked) => blocked,
        other => panic!("expected an identity leak refusal, got {other:?}"),
    }
}

/// What the governor has spent: requests, retries, rate tokens, retry budget.
fn spent(governor: &Governor) -> (u64, u64, u32, u32) {
    let snap = snap(governor);
    (
        snap.counters.requests,
        snap.counters.retries,
        snap.rate.available,
        snap.retry.remaining,
    )
}

#[tokio::test(start_paused = true)]
async fn every_leak_kind_is_refused_before_any_send_and_spends_nothing() {
    let cases: [(&str, &str, LeakKind); 7] = [
        ("You are Kanade.", "Alice is late", LeakKind::Name),
        ("You are Kanade.", "is ally coming?", LeakKind::Name),
        ("You are Kanade.", "ALICE is late", LeakKind::Name),
        ("You are Kanade.", "ping 200000000000000001", LeakKind::Id),
        ("You are Kanade.", "Zed said so", LeakKind::Name),
        (
            "You are Kanade.",
            "see message 123456789012345678",
            LeakKind::Snowflake,
        ),
        (
            "You are Kanade, and Alice is your friend.",
            "hi",
            LeakKind::Name,
        ),
    ];
    for (system, user, kind) in cases {
        let (governor, provider, client) = setup([ok(), ok()]);
        let mut identity = grant(&governor, &codec(), Role::Chat);
        // A non-roster author becomes a needle once registered (D11).
        identity.author_label(STRANGER_ID, "Zed");
        let before = spent(&governor);
        let mut question = client
            .open_question("member", false, limits())
            .await
            .unwrap()
            .with_scanner(identity.scanner());
        let leaky = request(system, user);

        let error = question.complete(&leaky).await.unwrap_err();
        assert_eq!(
            blocked(&error),
            &IdentityLeakBlocked {
                role: Role::Chat,
                kinds: vec![kind],
                count: 1
            },
            "{user}"
        );
        let clean = question.clean_retry(&leaky).await.unwrap_err();
        assert!(matches!(
            clean.failure,
            SessionFailure::IdentityLeakBlocked(_)
        ));
        assert!(provider.requests().is_empty(), "{user}: nothing sent");
        assert_eq!(question.requests_used(), 0);
        assert_eq!(spent(&governor), before, "{user}: nothing spent");
    }
}

#[tokio::test(start_paused = true)]
async fn an_encoded_request_is_sent_and_the_bot_is_never_flagged() {
    let (governor, provider, client) = setup([ok()]);
    let mut identity = grant(&governor, &codec(), Role::Chat);
    let label = identity.author_label(STRANGER_ID, "Zed");
    let text = identity.text(&format!(
        "Alice, Ally and <@{KEN_ID}> ask <@{BOT_ID}> about {ALICE_ID}; Zed agrees"
    ));
    let user = format!("[{label}] {text}");
    let mut question = client
        .open_question("member", false, limits())
        .await
        .unwrap()
        .with_scanner(identity.scanner());
    let sent = request("You are Kanade.", &user);
    question.complete(&sent).await.unwrap();
    assert_eq!(provider.requests(), vec![sent]);
}

#[tokio::test(start_paused = true)]
async fn only_code_owned_words_are_exempt() {
    let (governor, provider, client) = setup([ok(), ok()]);
    let mut identity = grant(&governor, &exempting_codec(), Role::Chat);
    let text = identity.text("Will and Ken are in");
    let mut question = client
        .open_question("member", false, limits())
        .await
        .unwrap()
        .with_scanner(identity.scanner());
    // `Will` appears in code-owned text, so the scanner skips it.
    question.complete(&request(CODE_TEXT, &text)).await.unwrap();
    // `Ken` is a stopword but not in code text: always scanned.
    let error = question
        .complete(&request(CODE_TEXT, "ken is late"))
        .await
        .unwrap_err();
    assert_eq!(blocked(&error).kinds, vec![LeakKind::Name]);
    // Documented residual: an unmasked `Will` is not caught.
    question
        .complete(&request(CODE_TEXT, "Will is late"))
        .await
        .unwrap();
    assert_eq!(provider.requests().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_name_equal_to_an_issued_token_does_not_refuse_that_token() {
    let (governor, provider, client) = setup([ok()]);
    let mut identity = grant(&governor, &codec(), Role::Chat);
    let alice = identity.member_ref(ALICE_ID);
    // An author registered later with a name spelling Alice's token.
    let label = identity.author_label(STRANGER_ID, &alice);
    assert_ne!(label, alice);
    let text = identity.text(&format!("<@{ALICE_ID}> and {alice} are in"));
    assert!(
        identity
            .scan_needles()
            .unwrap()
            .names
            .iter()
            .any(|n| n.token_clash)
    );
    let mut question = client
        .open_question("member", false, limits())
        .await
        .unwrap()
        .with_scanner(identity.scanner());
    let sent = request("x", &format!("{alice}: {text}"));
    question.complete(&sent).await.unwrap();
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_requeued_attempt_is_scanned_again() {
    let (governor, provider, client) = setup([
        FakeAction::AdmissionRefused(Some(Duration::from_secs(1))),
        ok(),
    ]);
    let mut identity = grant(&governor, &codec(), Role::Chat);
    let mut question = client
        .open_question("member", false, limits())
        .await
        .unwrap()
        .with_scanner(identity.scanner());
    let sent = request("x", "Zed is in");
    // The author is registered while the session waits to requeue.
    let (result, ()) = tokio::join!(question.complete(&sent), async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        identity.author_label(STRANGER_ID, "Zed");
    });
    let error = result.unwrap_err();
    assert_eq!(blocked(&error).kinds, vec![LeakKind::Name]);
    assert_eq!(provider.requests().len(), 1, "the requeue never sent");
    assert_eq!(question.requests_used(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_transient_retry_is_scanned_again_and_spends_no_retry() {
    let (governor, provider, client) = setup([FakeAction::Transient, ok()]);
    let mut identity = grant(&governor, &codec(), Role::Extraction);
    let before = snap(&governor).retry.remaining;
    let mut session = client
        .open_extraction("channel", Duration::from_secs(5), Duration::from_secs(60))
        .await
        .unwrap()
        .with_scanner(identity.scanner());
    let sent = request("x", "Zed is in");
    let (result, ()) = tokio::join!(session.complete(&sent), async {
        tokio::time::sleep(Duration::from_millis(1)).await;
        identity.author_label(STRANGER_ID, "Zed");
    });
    let error = result.unwrap_err();
    assert_eq!(blocked(&error).role, Role::Extraction);
    assert_eq!(provider.requests().len(), 1);
    let snap = snap(&governor);
    assert_eq!((snap.counters.requests, snap.counters.retries), (1, 0));
    assert_eq!(snap.retry.remaining, before);
    let answer = session.answer_retry(&sent).await.unwrap_err();
    assert_eq!(answer.failure, SessionFailure::AnswerRetryUnavailable);
}

#[tokio::test(start_paused = true)]
async fn an_answer_retry_and_a_rewrite_are_scanned() {
    let (governor, provider, client) = setup([ok(), ok()]);
    let mut identity = grant(&governor, &codec(), Role::Extraction);
    let mut session = client
        .open_extraction("channel", Duration::from_secs(5), Duration::from_secs(60))
        .await
        .unwrap()
        .with_scanner(identity.scanner());
    let first = identity.text("Alice is in");
    session.complete(&request("x", &first)).await.unwrap();
    let error = session
        .answer_retry(&request("x", "Alice is in"))
        .await
        .unwrap_err();
    assert_eq!(blocked(&error).role, Role::Extraction);
    assert_eq!(provider.requests().len(), 1);
    drop(session);

    let route = governor.route(Role::Rewrite).unwrap();
    let rewrite = open_session(&codec(), &route, &[]).unwrap();
    let mut session = client
        .open_rewrite("nudge", Duration::from_secs(5))
        .unwrap()
        .with_scanner(rewrite.scanner());
    let error = session
        .complete(&request("persona", &format!("remind {STRAY}")))
        .await
        .unwrap_err();
    assert_eq!(
        blocked(&error),
        &IdentityLeakBlocked {
            role: Role::Rewrite,
            kinds: vec![LeakKind::Snowflake],
            count: 1
        }
    );
    assert_eq!(provider.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn passthrough_is_unscanned_and_sent_byte_identical() {
    let (governor, provider, client) = setup([ok()]);
    let identity = grant(&governor, &Passthrough, Role::Chat);
    assert!(!identity.scanner().is_active());
    let mut question = client
        .open_question("member", false, limits())
        .await
        .unwrap()
        .with_scanner(identity.scanner());
    let raw = request("You are Kanade.", &format!("Alice <@{ALICE_ID}> {STRAY}"));
    question.complete(&raw).await.unwrap();
    assert_eq!(provider.requests(), vec![raw]);
}

#[tokio::test(start_paused = true)]
async fn refusals_never_show_the_matched_text() {
    let (governor, _, client) = setup([]);
    let identity = grant(&governor, &codec(), Role::Chat);
    let mut question = client
        .open_question("member", false, limits())
        .await
        .unwrap()
        .with_scanner(identity.scanner());
    let error = question
        .complete(&request("x", &format!("Alice Ally {ALICE_ID} {STRAY}")))
        .await
        .unwrap_err();
    let blocked = blocked(&error).clone();
    assert_eq!(
        blocked.kinds,
        vec![LeakKind::Name, LeakKind::Id, LeakKind::Snowflake]
    );
    assert_eq!(blocked.count, 4);
    assert_eq!(
        blocked.payload(),
        serde_json::json!({"role": "chat", "kinds": ["name", "id", "snowflake"], "count": 4})
    );
    assert_eq!(IdentityLeakBlocked::EVENT, "identity_leak_blocked");
    let shown = [
        format!("{error:?}"),
        error.to_string(),
        format!("{blocked:?}"),
        blocked.payload().to_string(),
        format!("{identity:?}"),
        format!("{:?}", identity.scanner()),
        format!("{question:?}"),
    ];
    for text in shown {
        for raw in [
            "Alice", "alice", "Ally", ALICE_ID, STRAY, "2000000", "1234567",
        ] {
            assert!(!text.contains(raw), "{text} shows {raw}");
        }
    }
    assert!(error.to_string().starts_with("identity_leak_blocked"));
    assert!(!error.is_misconfiguration());
}

/// A pseudonymizing codec whose sessions report nothing to scan for.
struct Blind;

struct BlindSession;

impl IdentityCodec for Blind {
    fn mode(&self) -> CodecMode {
        CodecMode::Pseudonymizing
    }

    fn open(&self, _: &[Member]) -> Box<dyn IdentitySession> {
        Box::new(BlindSession)
    }
}

impl IdentitySession for BlindSession {
    fn author_label(&mut self, _: &str, _: &str) -> String {
        "P".into()
    }

    fn member_ref(&mut self, _: &str) -> String {
        "P".into()
    }

    fn text(&mut self, text: &str) -> String {
        text.into()
    }

    fn tool_result(&mut self, content: &str) -> String {
        content.into()
    }

    fn participant_enum(&self) -> Option<Vec<String>> {
        None
    }

    fn decode_ref(&self, value: &str) -> Result<String, DecodeError> {
        Ok(value.into())
    }

    fn decode_json(&self, json: &str) -> Result<String, DecodeError> {
        Ok(json.into())
    }

    fn decode_reply(&self, text: &str) -> Result<String, DecodeError> {
        Ok(text.into())
    }
}

#[tokio::test(start_paused = true)]
async fn a_pseudonymizing_session_without_needles_fails_closed() {
    let (governor, provider, client) = setup([ok()]);
    let identity = grant(&governor, &Blind, Role::Chat);
    let mut question = client
        .open_question("member", false, limits())
        .await
        .unwrap()
        .with_scanner(identity.scanner());
    let error = question.complete(&request("x", "hi")).await.unwrap_err();
    assert_eq!(blocked(&error).kinds, vec![LeakKind::Unscannable]);
    assert!(provider.requests().is_empty());
}
