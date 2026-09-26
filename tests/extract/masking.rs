//! Extraction with pseudonymization on (the production `PseudonymCodec`):
//! the request carries no roster name or snowflake (message ids become
//! per-request refs), the participants enum is exactly the issued tokens,
//! evidence refs decode back, and an identity-leak refusal or an empty roster
//! sends nothing.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use kanade::domain::model_log::ExtractionOutcome;
use kanade::extract::pipeline::MessageEvent;
use kanade::infrastructure::llm::governor::XorShift;
use kanade::infrastructure::llm::identity::{
    BotIdentity, CodeLexicon, CodecMode, DecodeError, IdentityCodec, IdentitySession, Member,
    NamePool, PseudonymCodec, PseudonymConfig, ScanExemptions, ScanNeedles, find_request_leaks,
};
use kanade::infrastructure::llm::{ChatRequest, Message};
use serde_json::{Value, json};

use crate::fakes::{MY, PRIYA, World, after, local, message, nothing, reply};

/// A Discord-sized message id: it must never reach the model.
const MESSAGE_ID: &str = "1142000000000009001";

fn codec(guild: &kanade::domain::catalog::BossTable) -> PseudonymCodec {
    PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::from_boss_table(guild),
        bot: BotIdentity {
            user_id: Some("114200000000000001".into()),
            name: "Kanade".into(),
            aliases: Vec::new(),
        },
        extra_exclusions: Vec::new(),
        random: Arc::new(XorShift::new(11)),
    })
}

async fn masked(actions: Vec<kanade::infrastructure::llm::FakeAction>) -> World {
    let probe = World::new(Vec::new()).await;
    let bosses = probe.guild.bosses.clone();
    let exemptions = ScanExemptions::default()
        .with_texts(kanade::extract::prompt::code_owned_texts(
            chrono_tz::Asia::Singapore,
            &bosses,
        ))
        .with_value(&kanade::extract::prompt::code_owned_schema())
        .with_boss_table(&bosses);
    let codec = codec(&bosses).with_scan_exemptions(&exemptions);
    World::with(actions, |_| {}, Arc::new(codec)).await
}

fn post(text: &str) -> MessageEvent {
    MessageEvent::Posted(message(MESSAGE_ID, MY, local(8, 30, 13, 1), text))
}

fn user_prompt(request: &ChatRequest) -> &str {
    match &request.messages[1] {
        Message::User { content } => content,
        _ => panic!("user prompt"),
    }
}

fn participants_enum(request: &ChatRequest) -> Vec<String> {
    let schema = &request.output_schema.as_ref().expect("schema").schema;
    schema["$defs"]["Amendment"]["properties"]["participants"]["items"]["enum"]
        .as_array()
        .expect("strict enum")
        .iter()
        .map(|value| value.as_str().expect("token").to_owned())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn masked_requests_carry_refs_and_tokens_and_evidence_decodes_back() {
    let answer = reply(
        r#"{"amendments": [{"kind": "move", "bosses": ["HMaleficStar", "HFA"],
            "day_ref": "wed", "time_ref": "9:30pm", "participants": [],
            "confidence": 0.9, "evidence_message_ids": ["1"]}],
          "summary": "moved"}"#,
    );
    let world = masked(vec![answer]).await;
    let (events, _loop) = world.pipeline();
    let text = format!("Priya and <@{PRIYA}> cannot mon, change to wed 9:30pm? see <#900>");
    events.send(post(&text)).await.expect("send");
    after(91).await;
    let sent = &world.provider.requests()[0];
    let leaks = find_request_leaks(sent, &world.guild.members);
    assert!(leaks.is_empty(), "{leaks:?}");
    let prompt = user_prompt(sent);
    assert!(prompt.contains("[1] ["), "{prompt}");
    assert!(!prompt.contains(MESSAGE_ID));
    let tokens = participants_enum(sent);
    assert!(!tokens.is_empty());
    for token in &tokens {
        assert!(
            prompt.contains(&format!("<@{token}>")),
            "{token} was issued"
        );
    }
    let cards = world.outbox.cards.lock().unwrap().clone();
    assert_eq!(cards[0].entries[0].evidence_message_ids, [MESSAGE_ID]);
    let log = &world.logs().await[0];
    assert_eq!(log.outcome, ExtractionOutcome::Proposed);
    assert_eq!(log.guardrail, json!({"pseudonymized": true}));
}

#[tokio::test(start_paused = true)]
async fn an_unknown_message_ref_is_a_malformed_answer() {
    let unknown = reply(
        r#"{"amendments": [{"kind": "move", "bosses": ["HFA"], "day_ref": "wed",
            "time_ref": "9pm", "participants": [], "confidence": 0.9,
            "evidence_message_ids": ["7"]}], "summary": "x"}"#,
    );
    let world = masked(vec![unknown, nothing()]).await;
    let (events, _loop) = world.pipeline();
    events.send(post("hfa wed 9pm?")).await.expect("send");
    after(91).await;
    assert_eq!(world.requests(), 2, "answered with the retry");
}

#[tokio::test(start_paused = true)]
async fn an_empty_roster_with_masking_sends_nothing() {
    let world = masked(vec![nothing()]).await;
    world.guild.vacant.store(true, Ordering::SeqCst);
    let (events, _loop) = world.pipeline();
    events.send(post("hfa wed 9pm?")).await.expect("send");
    after(91).await;
    assert_eq!(world.requests(), 0);
    let log = &world.logs().await[0];
    assert_eq!(log.outcome, ExtractionOutcome::Failed);
    assert!(!world.processed(MESSAGE_ID).await, "read again later");
}

/// Reports what it would mask but encodes nothing: a codec bug the
/// boundary scanner must catch.
struct Leaky(PseudonymCodec);

struct LeakySession(Box<dyn IdentitySession>);

impl IdentityCodec for Leaky {
    fn mode(&self) -> CodecMode {
        CodecMode::Pseudonymizing
    }

    fn open(&self, roster: &[Member]) -> Box<dyn IdentitySession> {
        Box::new(LeakySession(self.0.open(roster)))
    }
}

impl IdentitySession for LeakySession {
    fn author_label(&mut self, user_id: &str, name: &str) -> String {
        self.0.author_label(user_id, name);
        name.to_owned()
    }
    fn member_ref(&mut self, user_id: &str) -> String {
        self.0.member_ref(user_id)
    }
    fn text(&mut self, text: &str) -> String {
        text.to_owned()
    }
    fn tool_result(&mut self, content: &str) -> String {
        content.to_owned()
    }
    fn participant_enum(&self) -> Option<Vec<String>> {
        self.0.participant_enum()
    }
    fn decode_ref(&self, value: &str) -> Result<String, DecodeError> {
        self.0.decode_ref(value)
    }
    fn decode_json(&self, json: &str) -> Result<String, DecodeError> {
        self.0.decode_json(json)
    }
    fn decode_reply(&self, text: &str) -> Result<String, DecodeError> {
        self.0.decode_reply(text)
    }
    fn scan_needles(&self) -> Option<ScanNeedles> {
        self.0.scan_needles()
    }
    fn masks(&self) -> bool {
        true
    }
    fn message_ref(&mut self, message_id: &str) -> String {
        self.0.message_ref(message_id)
    }
}

#[tokio::test(start_paused = true)]
async fn an_identity_leak_is_refused_logged_and_its_messages_marked_read() {
    let probe = World::new(Vec::new()).await;
    let bosses = probe.guild.bosses.clone();
    let world = World::with(vec![nothing()], |_| {}, Arc::new(Leaky(codec(&bosses)))).await;
    let (events, _loop) = world.pipeline();
    events
        .send(post("Priya cannot mon, hfa wed 9pm?"))
        .await
        .expect("send");
    after(91).await;
    assert_eq!(world.requests(), 0, "nothing reached the provider");
    let log = &world.logs().await[0];
    assert_eq!(log.outcome, ExtractionOutcome::IdentityLeak);
    assert_eq!(log.request_count, 0);
    assert_eq!(log.message_ids, [MESSAGE_ID]);
    let blocked: &Value = &log.guardrail["identity_leak_blocked"];
    assert_eq!(blocked["role"], "extraction");
    assert!(
        blocked["kinds"]
            .as_array()
            .unwrap()
            .contains(&json!("name"))
    );
    let shown = format!("{log:?} {} {:?}", log.guardrail, log.error);
    assert!(
        !shown.contains("Priya") && !shown.contains("Mylene"),
        "{shown}"
    );
    assert!(
        world.processed(MESSAGE_ID).await,
        "marked read; rescan re-reads"
    );
}
