//! Synthetic offline A/B evidence for extraction participants, evidence refs, and residuals.

use std::sync::Arc;

use kanade::domain::model_log::ExtractionOutcome;
use kanade::extract::pipeline::MessageEvent;
use kanade::extract::schema::{AttemptOutcome, ExtractionAttempts, Next};
use kanade::infrastructure::llm::governor::Random;
use kanade::infrastructure::llm::identity::{
    BotIdentity, CodeLexicon, IdentityCodec, Member, NamePool, Passthrough, PseudonymCodec,
    PseudonymConfig, ScanExemptions, TaggingCodec, find_request_leaks,
};
use kanade::infrastructure::llm::{ChatRequest, FakeAction};
use serde_json::{Value, json};

use crate::fakes::{World, after, local, message, nothing, reply};

struct FixedRandom;

impl Random for FixedRandom {
    fn next_u64(&self) -> u64 {
        1 << 63
    }
}

fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/privacy-evaluation.json"))
        .expect("synthetic privacy fixture")
}

fn roster(fixture: &Value) -> Vec<Member> {
    fixture["extraction_members"]
        .as_array()
        .expect("extraction members")
        .iter()
        .map(|raw| Member {
            user_id: raw["user_id"].as_str().expect("user id").to_owned(),
            display_name: raw["display_name"]
                .as_str()
                .expect("display name")
                .to_owned(),
            nickname: raw["nickname"].as_str().map(str::to_owned),
            aliases: raw["aliases"]
                .as_array()
                .expect("aliases")
                .iter()
                .map(|alias| alias.as_str().expect("alias").to_owned())
                .collect(),
        })
        .collect()
}

fn codec(bosses: &kanade::domain::catalog::BossTable) -> PseudonymCodec {
    let exemptions = ScanExemptions::default()
        .with_texts(kanade::extract::prompt::code_owned_texts(
            chrono_tz::Asia::Singapore,
            bosses,
        ))
        .with_value(&kanade::extract::prompt::code_owned_schema())
        .with_boss_table(bosses);
    PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::from_boss_table(bosses),
        bot: BotIdentity {
            user_id: Some("114200000000000001".into()),
            name: "Kanade".into(),
            aliases: Vec::new(),
        },
        extra_exclusions: Vec::new(),
        random: Arc::new(FixedRandom),
    })
    .with_scan_exemptions(&exemptions)
}

fn target_token(codec: &dyn IdentityCodec, roster: &[Member], target: &str) -> String {
    let mut session = codec.open(roster);
    session.author_label(target, "EOWYN ✨");
    session.member_ref(target)
}

fn response(participant: &str, message_ref: &str, summary: &str) -> FakeAction {
    reply(
        &json!({
            "amendments": [{
                "kind": "move",
                "bosses": ["HMaleficStar", "HFA"],
                "day_ref": "wed",
                "time_ref": "9:30pm",
                "participants": [participant],
                "confidence": 0.9,
                "evidence_message_ids": [message_ref]
            }],
            "summary": summary
        })
        .to_string(),
    )
}

struct ExtractRun {
    requests: Vec<ChatRequest>,
    logs: Vec<kanade::domain::model_log::ExtractionLog>,
    cards: Vec<kanade::extract::pipeline::Card>,
}

async fn run_extract(
    identity: Arc<dyn IdentityCodec>,
    members: &[Member],
    message_id: &str,
    author_id: &str,
    content: &str,
    action: FakeAction,
) -> ExtractRun {
    let world = World::with_members(vec![action], |_| {}, identity, members.to_vec()).await;
    let (events, _loop) = world.pipeline();
    events
        .send(MessageEvent::Posted(message(
            message_id,
            author_id,
            local(8, 30, 13, 1),
            content,
        )))
        .await
        .expect("synthetic message");
    after(91).await;
    ExtractRun {
        requests: world.provider.requests(),
        logs: world.logs().await,
        cards: world.outbox.cards.lock().unwrap().clone(),
    }
}

fn request_estimate(requests: &[ChatRequest]) -> (usize, usize) {
    let bytes = requests
        .iter()
        .map(|request| serde_json::to_vec(request).expect("request JSON").len())
        .sum::<usize>();
    (bytes, bytes.div_ceil(4))
}

fn participant_enum(request: &ChatRequest) -> Vec<String> {
    request
        .output_schema
        .as_ref()
        .expect("output schema")
        .schema["$defs"]["Amendment"]["properties"]["participants"]["items"]["enum"]
        .as_array()
        .expect("participant enum")
        .iter()
        .map(|value| value.as_str().expect("participant token").to_owned())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_frozen_extraction_parse_vector_is_equal_after_identity_decode() {
    let vectors = crate::support::load("parse.json");
    let case = vectors["cases"]
        .as_array()
        .expect("extract parse vectors")
        .iter()
        .find(|case| case["case_id"] == "accepted-and-coerced")
        .expect("frozen parse case");
    let fixture = fixture();
    let members = roster(&fixture);
    let probe = World::new(Vec::new()).await;
    let pseudo = codec(&probe.guild.bosses);
    let target_id = fixture["extraction"]["target_id"]
        .as_str()
        .expect("synthetic target id");
    let mut session = pseudo.open(&members);
    session.author_label(target_id, "EOWYN ✨");
    let token = session.member_ref(target_id);
    session.message_ref("1");

    let mut off_raw: Value = serde_json::from_str(
        case["input"]["steps"][0]["raw"]
            .as_str()
            .expect("frozen raw answer"),
    )
    .expect("frozen vector JSON");
    off_raw["amendments"][0]["participants"][0] = json!(target_id);
    let mut on_raw = off_raw.clone();
    on_raw["amendments"][0]["participants"][0] = json!(token);

    let decode =
        |raw: String, identity: &dyn kanade::infrastructure::llm::identity::IdentitySession| {
            let mut attempts = ExtractionAttempts::new(Vec::new());
            match attempts.record(
                AttemptOutcome::Reply {
                    content: Some(raw),
                    reasoning: None,
                },
                identity,
            ) {
                Next::Done(call) => call,
                Next::Retry => panic!("frozen valid answer unexpectedly retried"),
            }
        };
    let off_identity = Passthrough.open(&members);
    let off = decode(off_raw.to_string(), off_identity.as_ref());
    let on = decode(on_raw.to_string(), session.as_ref());
    assert_eq!(off.attempts, 1);
    assert_eq!(on.attempts, 1);
    assert_eq!(off.extraction, on.extraction);
    assert_eq!(
        on.extraction.expect("decoded vector answer").amendments[0].participants,
        [target_id]
    );
    println!(
        "PRIVACY_VECTOR {}",
        json!({
            "family": "extract",
            "case": case["case_id"],
            "response_parity": true,
            "participant_parity": true,
            "decode_failures": 0,
            "quarantines": 0
        })
    );
}

#[tokio::test(start_paused = true)]
async fn offline_ab_preserves_extraction_amendment_and_decoded_refs() {
    let fixture = fixture();
    let members = roster(&fixture);
    let probe = World::new(Vec::new()).await;
    let pseudo = Arc::new(codec(&probe.guild.bosses));
    let target_id = fixture["extraction"]["target_id"]
        .as_str()
        .expect("target id");
    let token = target_token(pseudo.as_ref(), &members, target_id);
    let message_id = fixture["extraction"]["message_id"]
        .as_str()
        .expect("message id");
    let content = fixture["extraction"]["content"]
        .as_str()
        .expect("message content");
    let summary = fixture["extraction"]["summary"].as_str().expect("summary");

    let off = run_extract(
        Arc::new(Passthrough),
        &members,
        message_id,
        target_id,
        content,
        response(target_id, message_id, summary),
    )
    .await;
    let on = run_extract(
        pseudo,
        &members,
        message_id,
        target_id,
        content,
        response(&token, "1", &format!("{token}'s group moved to p2.")),
    )
    .await;

    assert_eq!(off.requests.len(), 1);
    assert_eq!(on.requests.len(), 1);
    assert_eq!(off.logs.len(), 1);
    assert_eq!(on.logs.len(), 1);
    assert_eq!(off.logs[0].outcome, ExtractionOutcome::Proposed);
    assert_eq!(on.logs[0].outcome, ExtractionOutcome::Proposed);
    assert!(off.logs[0].error.is_none() && on.logs[0].error.is_none());
    assert_eq!(off.cards, on.cards);
    assert_eq!(on.cards.len(), 1);
    assert_eq!(on.cards[0].entries.len(), 1);
    let entry = &on.cards[0].entries[0];
    assert_eq!(entry.change.participants, [target_id]);
    assert_eq!(entry.evidence_message_ids, [message_id]);
    assert_eq!(entry.summary, summary);

    let issued = participant_enum(&on.requests[0]);
    assert!(issued.contains(&token));
    let hits = find_request_leaks(&on.requests[0], &members);
    let unexpected: Vec<_> = hits
        .iter()
        .filter(|hit| !hit.eq_ignore_ascii_case("HFA"))
        .collect();
    assert!(
        unexpected.is_empty(),
        "unexpected request leaks: {unexpected:?}"
    );

    let (off_bytes, off_tokens) = request_estimate(&off.requests);
    let (on_bytes, on_tokens) = request_estimate(&on.requests);
    println!(
        "PRIVACY_AB {}",
        json!({
            "family": "extract",
            "off_requests": off.requests.len(),
            "on_requests": on.requests.len(),
            "amendment_parity": true,
            "participant_parity": true,
            "evidence_ref_parity": true,
            "scanner_blocks": 0,
            "decode_failures": 0,
            "quarantines": 0,
            "off_request_bytes": off_bytes,
            "on_request_bytes": on_bytes,
            "off_request_token_proxy": off_tokens,
            "on_request_token_proxy": on_tokens,
            "request_token_proxy_delta": on_tokens as isize - off_tokens as isize,
            "estimate": "ceil(sum serialized ChatRequest bytes / 4); not provider tokenizer usage",
            "expected_code_owned_collision": fixture["boss_alias_collision"]
        })
    );
}

#[tokio::test(start_paused = true)]
async fn a_roster_name_url_is_redacted_before_masked_extraction() {
    let fixture = fixture();
    let members = roster(&fixture);
    let probe = World::new(Vec::new()).await;
    let pseudo = Arc::new(codec(&probe.guild.bosses));
    let message_id = fixture["extraction"]["message_id"]
        .as_str()
        .expect("message id");
    let target_id = fixture["extraction"]["target_id"]
        .as_str()
        .expect("target id");
    let content = fixture["extraction"]["name_in_url"]
        .as_str()
        .expect("synthetic URL input");
    let off = run_extract(
        Arc::new(Passthrough),
        &members,
        message_id,
        target_id,
        content,
        nothing(),
    )
    .await;
    let on = run_extract(pseudo, &members, message_id, target_id, content, nothing()).await;
    assert_eq!(off.requests.len(), 1);
    assert_eq!(on.requests.len(), 1);
    assert_eq!(off.logs[0].outcome, ExtractionOutcome::NoChange);
    assert_eq!(on.logs[0].outcome, ExtractionOutcome::NoChange);
    let sent = serde_json::to_string(&on.requests).expect("captured request");
    assert!(!sent.contains("https://"));
    assert!(!sent.contains("EOWYN"));
    assert!(sent.contains('⟦'));
    assert!(content.contains("https://example.invalid/users/EOWYN"));
}

#[tokio::test(start_paused = true)]
async fn non_roster_url_handles_are_redacted_from_extraction_requests() {
    let fixture = fixture();
    let members = roster(&fixture);
    let probe = World::new(Vec::new()).await;
    let pseudo = Arc::new(codec(&probe.guild.bosses));
    let message_id = fixture["extraction"]["message_id"]
        .as_str()
        .expect("message id");
    let target_id = fixture["extraction"]["target_id"]
        .as_str()
        .expect("target id");
    let content = fixture["extraction"]["non_roster_url"]
        .as_str()
        .expect("synthetic non-roster URL input");
    let off = run_extract(
        Arc::new(Passthrough),
        &members,
        message_id,
        target_id,
        content,
        nothing(),
    )
    .await;
    let on = run_extract(pseudo, &members, message_id, target_id, content, nothing()).await;
    let sent = serde_json::to_string(&on.requests).expect("captured requests");
    let plain = serde_json::to_string(&off.requests).expect("captured passthrough requests");
    assert_eq!(off.requests.len(), 1);
    assert_eq!(on.requests.len(), 1);
    assert_eq!(on.logs[0].outcome, ExtractionOutcome::NoChange);
    assert!(plain.contains("https://example.invalid/users/orbitquill42"));
    assert!(!sent.contains("https://"));
    assert!(!sent.contains("orbitquill42"));
    assert!(sent.contains('⟦'));
}

#[tokio::test(start_paused = true)]
async fn a_prefixed_extraction_url_is_masked_and_an_encoder_bypass_is_refused() {
    let fixture = fixture();
    let members = roster(&fixture);
    let probe = World::new(Vec::new()).await;
    let pseudo = Arc::new(codec(&probe.guild.bosses));
    let message_id = fixture["extraction"]["message_id"]
        .as_str()
        .expect("synthetic message id");
    let target_id = fixture["extraction"]["target_id"]
        .as_str()
        .expect("synthetic target id");
    let source = fixture["extraction"]["non_roster_url"]
        .as_str()
        .expect("synthetic URL input");
    let url = source
        .split_once("https://")
        .map(|(_, tail)| format!("https://{tail}"))
        .expect("URL");
    let content = format!("hfa wed 9pm? prefix{url}");
    let captured = run_extract(pseudo, &members, message_id, target_id, &content, nothing()).await;
    assert_eq!(captured.requests.len(), 1);
    assert_eq!(captured.logs[0].outcome, ExtractionOutcome::NoChange);
    let sent = serde_json::to_string(&captured.requests).expect("captured request");
    assert!(!sent.contains(&url));
    assert!(sent.contains('⟦'));

    let missed = run_extract(
        Arc::new(TaggingCodec),
        &members,
        message_id,
        target_id,
        &content,
        nothing(),
    )
    .await;
    assert!(missed.requests.is_empty());
    assert_eq!(missed.logs[0].outcome, ExtractionOutcome::IdentityLeak);
}
