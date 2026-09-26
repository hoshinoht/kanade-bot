//! Chat with pseudonymization on (the production `PseudonymCodec` behind a
//! masking client): no roster name, alias or id reaches the model in any
//! round or the clean retry, code-owned text stays literal, the Model view
//! is captured, and a leak after a posted card keeps the card and fails.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use kanade::chat::answer::{
    AnswerDeps, AnswerFailure, Generation, Question, answer, chat_outcome, interaction,
};
use kanade::chat::pilot::failure_reply;
use kanade::chat::prompts::{clock_header, code_owned_texts, focus_line};
use kanade::chat::sanitize::FAILURE_REPLY;
use kanade::chat::tools::ToolName;
use kanade::chat::tools::bundles::ToolOffer;
use kanade::domain::model_log::ChatOutcome;
use kanade::infrastructure::llm::governor::{Charge, XorShift};
use kanade::infrastructure::llm::identity::{
    BotIdentity, CodeLexicon, CodecMode, DecodeError, IdentityCodec, IdentitySession, LeakKind,
    Member, NamePool, Passthrough, PseudonymCodec, PseudonymConfig, ScanExemptions, ScanNeedles,
    find_request_leaks,
};
use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, FakeAction, FakeProvider, FinishReason, Message, ToolCall,
};
use serde_json::{Value, json};

use crate::looping::{Ports, roster, said, settings};
use crate::model::{Scripted, capabilities, client};
use crate::support::load;
use crate::wire::kanade;
use crate::world::World;

const MODEL: &str = "synthetic-chat";

fn codec() -> PseudonymCodec {
    let tools = ToolName::V4
        .into_iter()
        .chain([ToolName::RequestTools])
        .fold(ScanExemptions::default(), |words, tool| {
            words.with_value(&tool.schema())
        });
    PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::builtin(),
        bot: BotIdentity {
            user_id: Some("7".into()),
            name: "Kanade".into(),
            aliases: Vec::new(),
        },
        extra_exclusions: Vec::new(),
        random: Arc::new(XorShift::new(5)),
    })
    .with_scan_exemptions(
        &ScanExemptions::default()
            .with_texts(code_owned_texts("UTC"))
            .extend(&tools),
    )
}

fn member(user_id: &str, name: &str, aliases: &[&str]) -> Member {
    Member {
        user_id: user_id.into(),
        display_name: name.into(),
        nickname: None,
        aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
    }
}

/// The vector world's roster plus members named like English words.
fn masked_roster(world: &World) -> Vec<Member> {
    let mut members = roster(world);
    members[0].aliases = vec!["alvy".into()];
    members.push(member("114200000000000051", "Will", &[]));
    members.push(member("114200000000000052", "Sun", &[]));
    members.push(member("114200000000000053", "Ken", &[]));
    members.push(member("114200000000000054", "Jonas lau", &[]));
    members
}

fn header() -> String {
    let now = Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).unwrap();
    clock_header(&now, chrono_tz::UTC, &now)
}

fn system() -> String {
    format!(
        "You are Kanade. Priya runs the guild; we will see.\n\n{}\n\n{}",
        header(),
        focus_line("HStar Thu 22:00 — kanon, Alvin tan")
    )
}

fn wants(content: Option<&str>, calls: &[(&str, &str, Value)]) -> FakeAction {
    FakeAction::Response(CompletionResponse {
        model: MODEL.into(),
        content: content.map(str::to_owned),
        tool_calls: calls
            .iter()
            .map(|(id, name, arguments)| ToolCall {
                id: (*id).into(),
                name: (*name).into(),
                arguments: arguments.to_string(),
            })
            .collect(),
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    })
}

struct Run {
    generation: Generation,
    requests: Vec<ChatRequest>,
    roster: Vec<Member>,
    world: World,
    ctx: kanade::chat::tools::ToolContext,
}

async fn ask(actions: Vec<FakeAction>, codec: &dyn IdentityCodec, ports: &Ports) -> Run {
    ask_with(actions, codec, ports, &[], None).await
}

async fn ask_with(
    actions: Vec<FakeAction>,
    codec: &dyn IdentityCodec,
    ports: &Ports,
    former: &[(String, String)],
    history: Option<Vec<Message>>,
) -> Run {
    ask_tuned(actions, codec, ports, former, history, |_| {}).await
}

/// `history` replaces the stored turns before the question; `tune` edits
/// the model's published capabilities.
async fn ask_tuned(
    actions: Vec<FakeAction>,
    codec: &dyn IdentityCodec,
    ports: &Ports,
    former: &[(String, String)],
    history: Option<Vec<Message>>,
    tune: impl FnOnce(&mut kanade::infrastructure::llm::ModelCapabilities),
) -> Run {
    let input = load("loop.json")["cases"][0]["input"].clone();
    let mut world = World::new(&input).await;
    let mut caps = capabilities(&input["caps"]);
    tune(&mut caps);
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new(actions),
        caps,
    });
    let (_governor, client) = client(Some(MODEL), provider.clone());
    let client = client.with_masking(true);
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "900"}));
    let roster = masked_roster(&world);
    let deps = AnswerDeps {
        client: &client,
        codec,
        roster: &roster,
        former,
        route: None,
    };
    let mut conversation = vec![Message::System { content: system() }];
    conversation.extend(history.unwrap_or_else(|| {
        vec![Message::Assistant {
            content: Some("Alvin tan, kanon is on it.".into()),
            tool_calls: Vec::new(),
        }]
    }));
    conversation.push(Message::User {
        content: "Alvin tan: will Sun and alvy join hstar with <@22>? Will says yes".into(),
    });
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ctx,
            conversation,
            reminder: kanade().voice_reminder(),
            offer: ToolOffer::full_set(false),
            settings: settings(&input, 8),
        };
        answer(&deps, question, &guild, &mut proposer, ports).await
    };
    Run {
        generation,
        requests: provider.fake.requests(),
        roster,
        world,
        ctx,
    }
}

/// No raw identity outside the model's own round turns (its words, e.g. a
/// plain "will", come back verbatim and are not member data).
fn leak_free(run: &Run) {
    for request in &run.requests {
        let mut request = request.clone();
        request.messages.retain(|message| {
            !matches!(message, Message::Assistant { tool_calls, .. } if !tool_calls.is_empty())
        });
        let leaks = find_request_leaks(&request, &run.roster);
        assert!(leaks.is_empty(), "{leaks:?}");
    }
}

#[tokio::test(start_paused = true)]
async fn masked_rounds_leak_nobody_keep_code_text_and_capture_the_model_view() {
    let run = ask(
        vec![
            // The model's own "will" comes back in round 2 unscanned, though
            // a member is called Will.
            wants(
                Some("I will check that."),
                &[("g1", "get_run", json!({"query": "hstar"}))],
            ),
            FakeAction::Response(said(MODEL, "Nothing more to add.")),
        ],
        &codec(),
        &Ports::default(),
    )
    .await;
    assert_eq!(run.requests.len(), 2);
    leak_free(&run);
    let Message::System { content } = &run.requests[0].messages[0] else {
        panic!("system prompt");
    };
    assert!(
        content.contains(&header()),
        "the clock header stays literal"
    );
    assert_eq!(run.generation.reply, "Nothing more to add.");
    assert!(run.generation.pseudonymized);

    let view = run.generation.model_view.as_ref().expect("masked view");
    assert_eq!(view.rounds.len(), 2);
    for (round, request) in view.rounds.iter().zip(&run.requests) {
        assert_eq!(
            round.request,
            serde_json::to_value(&request.messages).unwrap()
        );
    }
    assert_eq!(view.rounds[0].tool_calls[0]["name"], "get_run");
    assert_eq!(view.reply, "Nothing more to add.");
    let kanon = view
        .mapping
        .iter()
        .find(|name| name.user_id == "22")
        .expect("kanon was issued a token");
    assert_eq!(kanon.display_name.as_deref(), Some("kanon [AZUR]"));
    let sent = serde_json::to_string(&run.requests[0].messages).unwrap();
    assert!(sent.contains(&kanon.token));
    for private in ["kanon", "Alvin", "22"] {
        assert!(!format!("{view:?}").contains(private), "Debug shows sizes");
    }

    let row = interaction(
        "chat-m".into(),
        Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).unwrap(),
        &run.ctx,
        "q",
        &run.generation,
        MODEL,
        None,
        1,
    );
    assert_eq!(row.guardrail, json!({"pseudonymized": true}));
    drop(run.world);
}

#[tokio::test(start_paused = true)]
async fn the_clean_retry_is_masked_too() {
    let run = ask(
        vec![
            FakeAction::Malformed,
            FakeAction::Response(said(MODEL, "Here you go.")),
        ],
        &codec(),
        &Ports::default(),
    )
    .await;
    assert!(run.generation.clean_retry);
    assert_eq!(run.requests.len(), 2);
    leak_free(&run);
    let view = run.generation.model_view.as_ref().expect("view");
    assert!(view.rounds.iter().any(|round| round.clean));
}

#[tokio::test(start_paused = true)]
async fn a_leak_after_a_posted_card_keeps_the_card_and_fails_with_the_line() {
    let ports = Ports::default();
    // A codec that fails to mask tool results: the card's result names the
    // party raw, so the round after the posted card is refused.
    let run = ask(
        vec![
            wants(
                Some("I will check."),
                &[(
                    "m1",
                    "propose_move",
                    json!({"run_query": "hstar", "to_when": "thu 22:00"}),
                )],
            ),
            FakeAction::Response(said(MODEL, "Card's up!")),
        ],
        &RawResults(codec()),
        &ports,
    )
    .await;
    let generation = &run.generation;
    assert_eq!(run.requests.len(), 1, "the leaking round never went out");
    assert_eq!(generation.posted.len(), 1, "the card stays");
    assert_eq!(ports.posted.lock().unwrap().len(), 1);
    let blocked = generation.leak_blocked().expect("refused");
    assert!(blocked.kinds.contains(&LeakKind::Name), "{blocked:?}");
    let Some(AnswerFailure::Session(error)) = &generation.failure else {
        panic!("session failure");
    };
    assert_eq!(error.charge, Charge::Refunded);
    assert!(generation.reply.is_empty());
    assert_eq!(failure_reply(generation, &kanade()), FAILURE_REPLY);
    assert_eq!(chat_outcome(generation), ChatOutcome::Error);
    let row = interaction(
        "chat-l".into(),
        Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).unwrap(),
        &run.ctx,
        "q",
        generation,
        MODEL,
        None,
        1,
    );
    assert_eq!(row.guardrail["identity_leak_blocked"], blocked.payload());
    assert_eq!(row.guardrail["identity_leak_blocked"]["role"], "chat");
    assert!(!row.to_string_lossy().contains("Alvin"));
}

#[tokio::test(start_paused = true)]
async fn a_masking_client_refuses_a_passthrough_session() {
    let run = ask(
        vec![FakeAction::Response(said(MODEL, "hi"))],
        &Passthrough,
        &Ports::default(),
    )
    .await;
    assert!(run.requests.is_empty(), "fails closed: no scanner");
    let blocked = run.generation.leak_blocked().expect("unscannable");
    assert_eq!(blocked.kinds, [LeakKind::Unscannable]);
}

trait Lossy {
    fn to_string_lossy(&self) -> String;
}

impl Lossy for kanade::domain::model_log::ChatInteraction {
    fn to_string_lossy(&self) -> String {
        format!("{self:?} {} {:?}", self.guardrail, self.error)
    }
}

/// Encodes everything but tool results (a codec bug the scanner catches).
struct RawResults(PseudonymCodec);

struct RawSession(Box<dyn IdentitySession>);

impl IdentityCodec for RawResults {
    fn mode(&self) -> CodecMode {
        CodecMode::Pseudonymizing
    }

    fn open(&self, roster: &[Member]) -> Box<dyn IdentitySession> {
        Box::new(RawSession(self.0.open(roster)))
    }
}

impl IdentitySession for RawSession {
    fn author_label(&mut self, user_id: &str, name: &str) -> String {
        self.0.author_label(user_id, name)
    }
    fn member_ref(&mut self, user_id: &str) -> String {
        self.0.member_ref(user_id)
    }
    fn text(&mut self, text: &str) -> String {
        self.0.text(text)
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
}

/// Conversation history never carries tool calls, so model-written tool
/// arguments of earlier questions never need encoding.
#[test]
fn assembled_history_carries_no_tool_calls() {
    use kanade::chat::context::{ChatTurn, TurnRole, assemble};
    let turns = [
        ChatTurn::new(TurnRole::User, "Alvin tan: hi", Some("1".into())),
        ChatTurn::new(TurnRole::Assistant, "Hi Alvin tan.", Some("2".into())),
        ChatTurn::new(TurnRole::User, "Alvin tan: when?", None),
    ];
    let messages = assemble(&turns, "SYSTEM".into(), 32_000);
    assert!(
        messages
            .iter()
            .any(|m| matches!(m, Message::Assistant { .. }))
    );
    for message in &messages {
        if let Message::Assistant { tool_calls, .. } = message {
            assert!(tool_calls.is_empty());
        }
    }
}

/// Stored history (live turns, anchors, reply chains, the bot's decoded
/// replies) is text rendered with the names of its time. Names members no
/// longer carry are registered as former names, so they are masked and
/// scanned like current ones.
#[tokio::test(start_paused = true)]
async fn former_names_in_stored_history_are_masked() {
    let history = vec![
        // Alvin was "Oldnick" (renamed) with alias "zorblax" (removed).
        Message::User {
            content: "Oldnick: zorblax here, is Ghost Rider coming?".into(),
        },
        // The bot's decoded reply names Oldnick and a member who left.
        Message::Assistant {
            content: Some("Sure Oldnick, Ghost Rider is on hstar.".into()),
            tool_calls: Vec::new(),
        },
        // An anchored late reply's parent, as the reply chain renders it.
        Message::User {
            content: "Ghost Rider: count me in".into(),
        },
    ];
    let former = [
        ("11", "Oldnick"),
        ("11", "zorblax"),
        ("114200000000000077", "Ghost Rider"),
    ]
    .map(|(id, name)| (id.to_owned(), name.to_owned()));
    let run = ask_with(
        vec![FakeAction::Response(said(MODEL, "On it."))],
        &codec(),
        &Ports::default(),
        &former,
        Some(history.clone()),
    )
    .await;
    assert_eq!(run.requests.len(), 1, "masked and sent");
    let sent = serde_json::to_string(&run.requests[0].messages).unwrap();
    // Single words of a multi-word name stay a codec residual (bare `Ghost`).
    for old in ["Oldnick", "zorblax", "Ghost Rider"] {
        assert!(!sent.contains(old), "{old}: {sent}");
    }

    // Without the former names the old ones would go out: the scanner does
    // not know them either (this is the refused-before-fix shape).
    let unregistered = ask_with(
        vec![FakeAction::Response(said(MODEL, "On it."))],
        &codec(),
        &Ports::default(),
        &[],
        Some(history),
    )
    .await;
    let sent = serde_json::to_string(&unregistered.requests[0].messages).unwrap();
    assert!(sent.contains("Oldnick"), "the gap the registry closes");
}

/// A former name the codec failed to encode is still refused at the boundary.
#[tokio::test(start_paused = true)]
async fn an_unencoded_former_name_is_caught_by_the_scanner() {
    let former = [("11".to_owned(), "Oldnick".to_owned())];
    let run = ask_with(
        vec![FakeAction::Response(said(MODEL, "On it."))],
        &RawText(codec()),
        &Ports::default(),
        &former,
        Some(vec![Message::User {
            content: "Oldnick: hi".into(),
        }]),
    )
    .await;
    assert!(run.requests.is_empty());
    let blocked = run.generation.leak_blocked().expect("refused");
    assert!(blocked.kinds.contains(&LeakKind::Name));
}

/// Encodes nothing (a codec bug), but registers names like the real one.
struct RawText(PseudonymCodec);

struct RawTextSession(Box<dyn IdentitySession>);

impl IdentityCodec for RawText {
    fn mode(&self) -> CodecMode {
        CodecMode::Pseudonymizing
    }

    fn open(&self, roster: &[Member]) -> Box<dyn IdentitySession> {
        Box::new(RawTextSession(self.0.open(roster)))
    }
}

impl IdentitySession for RawTextSession {
    fn author_label(&mut self, user_id: &str, name: &str) -> String {
        self.0.author_label(user_id, name)
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
        None
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
    fn former_name(&mut self, user_id: &str, name: &str) {
        self.0.former_name(user_id, name);
    }
    fn masks(&self) -> bool {
        true
    }
}

/// Each logged round names what the governed session actually sent: the
/// alias, the effort after shaping (none when the model has no reasoning
/// control), the route and the clean flag; the row carries the error code.
#[tokio::test(start_paused = true)]
async fn logged_rounds_record_what_was_sent() {
    let run = ask_tuned(
        vec![
            FakeAction::Malformed,
            FakeAction::Response(said(MODEL, "Here.")),
        ],
        &codec(),
        &Ports::default(),
        &[],
        None,
        |caps| caps.reasoning_control = false,
    )
    .await;
    let rounds = &run.generation.model_rounds;
    let sent = rounds[0].sent.as_ref().expect("sent facts");
    assert_eq!((sent.alias.as_str(), sent.effort), (MODEL, None));
    let row = interaction(
        "chat-s".into(),
        Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).unwrap(),
        &run.ctx,
        "q",
        &run.generation,
        "configured-alias",
        Some(kanade::infrastructure::llm::Effort::High),
        1,
    );
    assert_eq!(
        row.rounds[0].model, MODEL,
        "the alias as sent, not the configured one"
    );
    assert_eq!(row.rounds[0].reasoning, None, "no effort went out");
    assert_eq!(row.rounds[0].route.as_deref(), Some("homelab"));
    assert!(row.rounds.last().unwrap().clean);
    assert_eq!(row.error_code, None);

    let failed = ask(
        vec![FakeAction::Malformed, FakeAction::Malformed],
        &codec(),
        &Ports::default(),
    )
    .await;
    let row = interaction(
        "chat-f".into(),
        Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).unwrap(),
        &failed.ctx,
        "q",
        &failed.generation,
        MODEL,
        None,
        1,
    );
    assert_eq!(row.error_code.as_deref(), Some("malformed"));
}

/// The Model view numbers rounds by their logged position, as the
/// transcript does: a clean retry after round 2 is round 3 in both.
#[tokio::test(start_paused = true)]
async fn model_view_rounds_join_the_logged_rounds_after_a_clean_retry() {
    let run = ask(
        vec![
            wants(None, &[("g1", "get_run", json!({"query": "hstar"}))]),
            // An empty answer spends the clean retry.
            FakeAction::Response(said(MODEL, "")),
            FakeAction::Response(said(MODEL, "Here.")),
        ],
        &codec(),
        &Ports::default(),
    )
    .await;
    assert!(run.generation.clean_retry);
    let row = interaction(
        "chat-n".into(),
        Utc.with_ymd_and_hms(2026, 9, 6, 12, 0, 0).unwrap(),
        &run.ctx,
        "q",
        &run.generation,
        MODEL,
        None,
        1,
    );
    let view = run.generation.model_view.as_ref().expect("view");
    assert_eq!(view.rounds.len(), row.rounds.len());
    let numbers: Vec<u32> = view.rounds.iter().map(|round| round.round).collect();
    assert_eq!(numbers, [1, 2, 3]);
    let clean: Vec<bool> = view.rounds.iter().map(|round| round.clean).collect();
    assert_eq!(
        clean,
        row.rounds
            .iter()
            .map(|round| round.clean)
            .collect::<Vec<_>>()
    );
    assert_eq!(clean, [false, false, true]);
}

/// Party channels named after members' first names (the live shape): the
/// first name of a multi-word display name is masked in every round.
#[tokio::test(start_paused = true)]
async fn first_names_inside_channel_names_are_masked() {
    let history = vec![Message::User {
        content: "Alvin tan: is jonas in #hbaldguy-jonas-cryz or hstar_jonas?".into(),
    }];
    let run = ask_with(
        vec![FakeAction::Response(said(MODEL, "Yes."))],
        &codec(),
        &Ports::default(),
        &[],
        Some(history),
    )
    .await;
    assert_eq!(run.requests.len(), 1);
    let sent = serde_json::to_string(&run.requests[0].messages).unwrap();
    assert!(!sent.to_lowercase().contains("jonas"), "{sent}");
    assert!(
        sent.contains("#hbaldguy-"),
        "the channel name keeps its other words"
    );
}
