//! Synthetic offline A/B evidence for pseudonymized chat requests and tool args.

use std::sync::Arc;

use kanade::chat::answer::{AnswerDeps, Generation, Question, answer};
use kanade::chat::prompts::{code_owned_texts, protected};
use kanade::chat::tools::ToolName;
use kanade::chat::tools::bundles::ToolOffer;
use kanade::infrastructure::llm::governor::Random;
use kanade::infrastructure::llm::identity::{
    BotIdentity, CodeLexicon, IdentityCodec, Member, NamePool, Passthrough, PseudonymCodec,
    PseudonymConfig, ScanExemptions, find_request_leaks,
};
use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, FakeAction, FakeProvider, FinishReason, Message, ToolCall,
};
use serde_json::{Value, json};

use crate::looping::{Ports, settings};
use crate::model::{Scripted, capabilities, client};
use crate::support::load;
use crate::world::World;

const MODEL: &str = "synthetic-chat";
const SYSTEM: &str = "Schedule assistant. Preserve the requested day, time, boss, and party.";
const REMINDER: &str = "Keep the result brief and precise.";

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
    fixture["chat_members"]
        .as_array()
        .expect("chat members")
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

fn codec() -> PseudonymCodec {
    codec_with_bot("114200000000000001")
}

fn codec_with_bot(bot_id: &str) -> PseudonymCodec {
    let tools = ToolName::V4
        .into_iter()
        .chain([ToolName::RequestTools])
        .fold(ScanExemptions::default(), |words, tool| {
            words.with_value(&tool.schema())
        });
    let exemptions = ScanExemptions::default()
        .with_texts(code_owned_texts("UTC"))
        .extend(&tools);
    PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::builtin().with_terms(["HFA"]),
        bot: BotIdentity {
            user_id: Some(bot_id.into()),
            name: "Kanade".into(),
            aliases: Vec::new(),
        },
        extra_exclusions: Vec::new(),
        random: Arc::new(FixedRandom),
    })
    .with_scan_exemptions(&exemptions)
}

fn vector_roster(input: &Value) -> Vec<Member> {
    // Remap short vector ids: "22" also appears as the frozen 22:00 time.
    input["world"]["members"]
        .as_array()
        .expect("vector world members")
        .iter()
        .map(|raw| Member {
            user_id: format!(
                "114299900000000{:03}",
                raw["user_id"]
                    .as_str()
                    .expect("user id")
                    .parse::<u16>()
                    .expect("numeric fixture id")
            ),
            display_name: raw["display_name"]
                .as_str()
                .expect("display name")
                .to_owned(),
            nickname: raw["nickname"].as_str().map(str::to_owned),
            aliases: Vec::new(),
        })
        .collect()
}

fn encoded_target(codec: &dyn IdentityCodec, roster: &[Member], question: &str) -> String {
    let mut session = codec.open(roster);
    let owned = protected();
    kanade::infrastructure::llm::identity::encode_protected(session.as_mut(), SYSTEM, &owned);
    session.text(question);
    kanade::infrastructure::llm::identity::encode_protected(session.as_mut(), REMINDER, &owned);
    session.member_ref("11")
}

fn tool_action(participant: &str) -> FakeAction {
    FakeAction::Response(CompletionResponse {
        model: MODEL.into(),
        content: Some("I have enough detail.".into()),
        tool_calls: vec![ToolCall {
            id: "privacy-add".into(),
            name: "propose_add".into(),
            arguments: json!({
                "boss": "HFA",
                "when": "wed 9pm",
                "participants": participant,
            })
            .to_string(),
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    })
}

fn answer_action(content: &str) -> FakeAction {
    FakeAction::Response(CompletionResponse {
        model: MODEL.into(),
        content: Some(content.into()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    })
}

struct ChatRun {
    generation: Generation,
    requests: Vec<ChatRequest>,
    cards: Vec<kanade::chat::tools::ProposalCard>,
}

async fn run_chat(
    fixture: &Value,
    roster: &[Member],
    codec: &dyn IdentityCodec,
    masking: bool,
    question: &str,
    actions: Vec<FakeAction>,
) -> ChatRun {
    let mut input = load("loop.json")["cases"][0]["input"].clone();
    for member in input["world"]["members"]
        .as_array_mut()
        .expect("vector world members")
    {
        if let Some(synthetic) = fixture["chat_members"]
            .as_array()
            .expect("synthetic members")
            .iter()
            .find(|synthetic| synthetic["user_id"] == member["user_id"])
        {
            member["display_name"] = synthetic["display_name"].clone();
            member["nickname"] = synthetic["nickname"].clone();
        }
    }
    let mut world = World::new(&input).await;
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new(actions),
        caps: capabilities(&input["caps"]),
    });
    let (_governor, client) = client(Some(MODEL), provider.clone());
    let client = if masking {
        client.with_masking(true)
    } else {
        client
    };
    let ctx_input = json!({"author_id": "11", "channel_id": "900"});
    let ctx = world.context(&ctx_input);
    let conversation = vec![
        Message::System {
            content: SYSTEM.into(),
        },
        Message::User {
            content: question.to_owned(),
        },
    ];
    let question = Question {
        ctx: &ctx,
        conversation,
        reminder: REMINDER.into(),
        offer: ToolOffer::full_set(false),
        settings: settings(&input, 4),
    };
    let deps = AnswerDeps {
        client: &client,
        codec,
        roster,
        former: &[],
        route: None,
    };
    let ports = Ports::default();
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        answer(&deps, question, &guild, &mut proposer, &ports).await
    };
    ChatRun {
        generation,
        requests: provider.fake.requests(),
        cards: ports.posted.into_inner().expect("posted cards"),
    }
}

async fn run_vector_chat(
    input: &Value,
    step: &Value,
    roster: &[Member],
    codec: &dyn IdentityCodec,
    masking: bool,
    actions: Vec<FakeAction>,
) -> ChatRun {
    let mut world = World::new(input).await;
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new(actions),
        caps: capabilities(&input["caps"]),
    });
    let (_governor, client) = client(Some(MODEL), provider.clone());
    let client = if masking {
        client.with_masking(true)
    } else {
        client
    };
    let ctx = world.context(step);
    let question = Question {
        ctx: &ctx,
        conversation: crate::wire::messages(&step["conversation"]),
        reminder: crate::wire::kanade().voice_reminder(),
        offer: ToolOffer::full_set(ctx.read_only),
        settings: settings(input, crate::looping::V4_TOOL_ROUNDS),
    };
    let deps = AnswerDeps {
        client: &client,
        codec,
        roster,
        former: &[],
        route: None,
    };
    let ports = Ports::default();
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        answer(&deps, question, &guild, &mut proposer, &ports).await
    };
    ChatRun {
        generation,
        requests: provider.fake.requests(),
        cards: ports.posted.into_inner().expect("posted cards"),
    }
}

fn request_estimate(requests: &[ChatRequest]) -> (usize, usize) {
    let bytes = requests
        .iter()
        .map(|request| serde_json::to_vec(request).expect("request JSON").len())
        .sum::<usize>();
    (bytes, bytes.div_ceil(4))
}

#[tokio::test(start_paused = true)]
async fn a_frozen_chat_vector_has_the_same_off_and_on_tool_effect() {
    let vectors = load("loop.json");
    let case = vectors["cases"]
        .as_array()
        .expect("chat vector cases")
        .iter()
        .find(|case| case["case_id"] == "posted-write-reserves-the-confirmation-round")
        .expect("frozen chat case");
    let input = &case["input"];
    let step = &input["steps"][0];
    let roster = vector_roster(input);
    let actions: Vec<_> = step["replies"]
        .as_array()
        .expect("frozen replies")
        .iter()
        .map(|reply| crate::model::action(reply, MODEL))
        .collect();
    let pseudo = codec_with_bot(input["bot_user"]["id"].as_str().expect("bot id"));
    let off = run_vector_chat(input, step, &roster, &Passthrough, false, actions.clone()).await;
    let on = run_vector_chat(input, step, &roster, &pseudo, true, actions).await;
    let expected = &case["expected"]["steps"][0]["value"];
    let expected_tools: Vec<String> = expected["tool_calls"]
        .as_array()
        .expect("vector tool calls")
        .iter()
        .map(|tool| tool.as_str().expect("tool name").to_owned())
        .collect();
    let expected_created: Vec<String> = expected["created"]
        .as_array()
        .expect("vector created ids")
        .iter()
        .map(|id| id.as_str().expect("created id").to_owned())
        .collect();

    assert!(off.generation.failure.is_none() && on.generation.failure.is_none());
    assert_eq!(off.requests.len(), 2);
    assert_eq!(on.requests.len(), 2);
    assert_eq!(off.generation.reply, expected["reply"]);
    assert_eq!(on.generation.reply, expected["reply"]);
    assert_eq!(off.generation.tool_calls, expected_tools);
    assert_eq!(on.generation.tool_calls, expected_tools);
    assert_eq!(off.generation.created, expected_created);
    assert_eq!(on.generation.created, expected_created);
    assert_eq!(off.cards, on.cards);
    assert!(on.generation.leak_blocked().is_none());
    for request in &on.requests {
        assert_eq!(find_request_leaks(request, &roster), Vec::<String>::new());
    }
    println!(
        "PRIVACY_VECTOR {}",
        json!({
            "family": "chat",
            "case": case["case_id"],
            "off_requests": off.requests.len(),
            "on_requests": on.requests.len(),
            "reply_parity": true,
            "tool_effect_parity": true,
            "scanner_blocks": 0,
            "decode_failures": 0,
            "quarantines": 0
        })
    );
}

#[tokio::test(start_paused = true)]
async fn offline_ab_preserves_chat_reply_and_decoded_tool_arguments() {
    let fixture = fixture();
    let roster = roster(&fixture);
    let question = fixture["chat"]["question"].as_str().expect("question");
    let codec = codec();
    let target_token = encoded_target(&codec, &roster, question);
    let expected_reply = fixture["chat"]["answer"].as_str().expect("answer");
    let off = run_chat(
        &fixture,
        &roster,
        &Passthrough,
        false,
        question,
        vec![tool_action("<@11>"), answer_action(expected_reply)],
    )
    .await;
    let on = run_chat(
        &fixture,
        &roster,
        &codec,
        true,
        question,
        vec![
            tool_action(&format!("<@{target_token}>")),
            answer_action(&format!("{target_token}'s signup is HFA p1.")),
        ],
    )
    .await;

    assert!(off.generation.failure.is_none());
    assert!(on.generation.failure.is_none());
    assert!(!off.generation.pseudonymized);
    assert!(on.generation.pseudonymized);
    assert!(!off.generation.clean_retry && !on.generation.clean_retry);
    assert_eq!(off.requests.len(), 2);
    assert_eq!(on.requests.len(), 2);
    assert_eq!(off.generation.reply, on.generation.reply);
    assert_eq!(off.generation.reply, expected_reply);
    assert_eq!(off.generation.tool_calls, on.generation.tool_calls);
    assert_eq!(off.cards, on.cards);
    assert_eq!(on.cards.len(), 1);
    assert_eq!(on.cards[0].participants, ["11"]);
    assert_eq!(on.generation.leak_blocked(), None);

    let raw_args = on.generation.model_view.as_ref().expect("masked view");
    let tool_args = raw_args.rounds[0].tool_calls[0]["arguments"]
        .as_str()
        .expect("captured tool arguments");
    let decoded_args: Value = serde_json::from_str(tool_args).expect("tool JSON");
    assert_eq!(decoded_args["participants"], format!("<@{target_token}>"));
    assert!(
        !tool_args.contains("<@11>"),
        "raw model arguments must use the issued token"
    );
    let hits = find_request_leaks(&on.requests[0], &roster);
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
            "family": "chat",
            "off_requests": off.requests.len(),
            "on_requests": on.requests.len(),
            "reply_parity": true,
            "tool_call_parity": true,
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
async fn a_member_name_in_a_url_is_an_expected_fail_closed_residual() {
    let fixture = fixture();
    let roster = roster(&fixture);
    let question = fixture["chat"]["name_in_url"]
        .as_str()
        .expect("URL residual input");
    let codec = codec();
    let off = run_chat(
        &fixture,
        &roster,
        &Passthrough,
        false,
        question,
        vec![answer_action("Received.")],
    )
    .await;
    let on = run_chat(
        &fixture,
        &roster,
        &codec,
        true,
        question,
        vec![answer_action("Received.")],
    )
    .await;
    assert_eq!(off.requests.len(), 1);
    assert!(
        on.requests.is_empty(),
        "leaking URL must not reach provider"
    );
    assert!(on.generation.leak_blocked().is_some());
    println!(
        "PRIVACY_RESIDUAL {}",
        json!({
            "family": "chat",
            "category": "member_name_in_url",
            "off_provider_sends": off.requests.len(),
            "on_provider_sends": on.requests.len(),
            "expected_scanner_blocks": 1,
            "acceptance_blocker": true,
            "required_followup": "redact identity-bearing URL paths; preserve the original link outside provider requests",
            "quarantines": 0
        })
    );
}

#[tokio::test(start_paused = true)]
async fn a_non_roster_url_handle_reaches_the_masked_chat_provider() {
    let fixture = fixture();
    let roster = roster(&fixture);
    let question = fixture["chat"]["non_roster_url"]
        .as_str()
        .expect("non-roster URL input");
    let codec = codec();
    let off = run_chat(
        &fixture,
        &roster,
        &Passthrough,
        false,
        question,
        vec![answer_action("Received.")],
    )
    .await;
    let on = run_chat(
        &fixture,
        &roster,
        &codec,
        true,
        question,
        vec![answer_action("Received.")],
    )
    .await;
    let sent = serde_json::to_string(&on.requests).expect("captured requests");
    assert_eq!(off.requests.len(), 1);
    assert_eq!(on.requests.len(), 1);
    assert!(on.generation.leak_blocked().is_none());
    assert!(sent.contains("orbitquill42"));
    println!(
        "PRIVACY_RESIDUAL {}",
        json!({
            "family": "chat",
            "category": "non_roster_identity_in_url",
            "off_provider_sends": off.requests.len(),
            "on_provider_sends": on.requests.len(),
            "handle_visible_in_request": true,
            "scanner_blocks": 0,
            "acceptance_blocker": true,
            "quarantines": 0
        })
    );
}
