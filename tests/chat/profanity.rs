//! The chat profanity guardrail on the reply side: a listed word in the
//! finished reply spends the reserved clean retry once; a clean retry is
//! delivered, anything else sends the deflection line. Invented text only.
//! The live nudge-rewrite list is covered in `tests/extract/nudge.rs`.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use kanade::chat::answer::{
    AnswerDeps, Generation, ProfanityGuard, Question, answer, chat_outcome, interaction,
};
use kanade::chat::nudge::{WordFilter, builtin_words};
use kanade::chat::tools::bundles::ToolOffer;
use kanade::domain::model_log::ChatOutcome;
use kanade::domain::settings::{DEFAULT_DEFLECTION_LINE, Profanity};
use kanade::infrastructure::llm::{ChatRequest, FakeAction, FakeProvider, Message};
use serde_json::json;

use crate::looping::{Ports, said, settings};
use crate::model::{Scripted, capabilities, client};
use crate::support::load;
use crate::wire::kanade;
use crate::world::World;

const MODEL: &str = "synthetic-chat";

fn words(text: &str) -> FakeAction {
    FakeAction::Response(said(MODEL, text))
}

struct Run {
    generation: Generation,
    requests: Vec<ChatRequest>,
}

async fn run(actions: Vec<FakeAction>, guard: &ProfanityGuard, clean_retry: bool) -> Run {
    run_in(actions, guard, clean_retry, |_| {}).await
}

/// Cards fail to post when `PORTS_FAIL` is set by the caller's edit.
const PORTS_FAIL: &str = "__ports_fail";

/// `edit` adjusts the vector input (its world) before the question.
async fn run_in(
    actions: Vec<FakeAction>,
    guard: &ProfanityGuard,
    clean_retry: bool,
    edit: impl FnOnce(&mut serde_json::Value),
) -> Run {
    let mut input = load("loop.json")["cases"][0]["input"].clone();
    edit(&mut input);
    let ports = Ports {
        fail: input.get(PORTS_FAIL).is_some(),
        ..Ports::default()
    };
    let mut world = World::new(&input).await;
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new(actions),
        caps: capabilities(&input["caps"]),
    });
    let (_governor, client) = client(Some(MODEL), provider.clone());
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "900"}));
    let deps = AnswerDeps {
        client: &client,
        route: None,
    };
    let mut tuned = settings(&input, 8);
    tuned.clean_retry = clean_retry;
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ctx,
            conversation: vec![
                Message::System {
                    content: "SYSTEM".into(),
                },
                Message::User {
                    content: "Invented Mira: what's on tonight?".into(),
                },
            ],
            profanity: Some(guard),
            reminder: kanade().voice_reminder(),
            offer: ToolOffer::full_set(false),
            settings: tuned,
        };
        answer(&deps, question, &guild, &mut proposer, &ports).await
    };
    Run {
        generation,
        requests: provider.fake.requests(),
    }
}

fn logged(generation: &Generation) -> serde_json::Value {
    let at = Utc.with_ymd_and_hms(2026, 9, 9, 4, 0, 0).unwrap();
    let ctx = kanade::chat::tools::ToolContext::new("11", "900", "1001", at);
    let row = interaction(
        "c-1".into(),
        at,
        &ctx,
        "what's on tonight?",
        generation,
        MODEL,
        None,
        5,
    );
    assert_eq!(row.outcome, ChatOutcome::Profanity);
    row.guardrail["profanity"].clone()
}

#[tokio::test(start_paused = true)]
async fn a_listed_reply_is_retried_clean_and_the_clean_answer_is_delivered() {
    let guard = ProfanityGuard::default();
    let run = run(
        vec![words("Lotus is at 9, shit."), words("Lotus is at 9.")],
        &guard,
        true,
    )
    .await;
    assert_eq!(run.requests.len(), 2);
    let clean = &run.requests[1];
    assert!(clean.tools.is_empty(), "the reserved clean retry");
    assert_eq!(clean.messages.len(), 3, "system, question, reminder");
    assert_eq!(run.generation.reply, "Lotus is at 9.");
    assert!(run.generation.clean_retry, "the storm guard counts it");
    assert_eq!(chat_outcome(&run.generation), ChatOutcome::Profanity);
    assert_eq!(
        logged(&run.generation),
        json!({"side": "reply", "word": "shit", "sent": null})
    );
}

#[tokio::test(start_paused = true)]
async fn a_retry_that_hits_again_sends_the_deflection_line() {
    let guard = ProfanityGuard::default();
    let run = run(
        vec![words("Lotus is at 9, shit."), words("Fucking 9.")],
        &guard,
        true,
    )
    .await;
    assert_eq!(run.requests.len(), 2);
    assert_eq!(run.generation.reply, DEFAULT_DEFLECTION_LINE);
    assert_eq!(run.generation.failure, None);
    assert_eq!(
        logged(&run.generation),
        json!({"side": "reply", "word": "shit", "sent": DEFAULT_DEFLECTION_LINE})
    );
}

#[tokio::test(start_paused = true)]
async fn without_the_reserved_retry_the_line_is_sent_at_once() {
    let guard = ProfanityGuard::new(&Profanity {
        extra_words: vec!["frick".into()],
        deflection_line: "Language, please!".into(),
        ..Profanity::default()
    });
    // The storm or per-member guard withheld the retry: no second request.
    let run = run(vec![words("Frickin 9 tonight.")], &guard, false).await;
    assert_eq!(run.requests.len(), 1);
    assert_eq!(run.generation.reply, "Language, please!");
    assert!(!run.generation.clean_retry);
    assert_eq!(
        logged(&run.generation),
        json!({"side": "reply", "word": "frick", "sent": "Language, please!"})
    );
}

#[tokio::test(start_paused = true)]
async fn a_retry_already_spent_on_a_malformed_answer_leaves_only_the_line() {
    let guard = ProfanityGuard::default();
    let run = run(vec![words(""), words("Shit, 9.")], &guard, true).await;
    assert_eq!(run.requests.len(), 2, "no third request");
    assert_eq!(run.generation.reply, DEFAULT_DEFLECTION_LINE);
    assert_eq!(chat_outcome(&run.generation), ChatOutcome::Profanity);
}

#[tokio::test(start_paused = true)]
async fn unchecked_replies_and_allowed_words_pass() {
    let off = ProfanityGuard::new(&Profanity {
        check_replies: false,
        ..Profanity::default()
    });
    let run = run(vec![words("Lotus is at 9, shit.")], &off, true).await;
    assert_eq!(run.generation.reply, "Lotus is at 9, shit.");
    assert_eq!(chat_outcome(&run.generation), ChatOutcome::Answered);
    let allowed = ProfanityGuard::new(&Profanity {
        allowed_words: vec!["babi".into()],
        ..Profanity::default()
    });
    let run = run_allowed(&allowed).await;
    assert_eq!(run.requests.len(), 1);
    assert_eq!(chat_outcome(&run.generation), ChatOutcome::Answered);
}

async fn run_allowed(guard: &ProfanityGuard) -> Run {
    run(vec![words("Babi Lotus at 9.")], guard, true).await
}

/// Boss names and aliases from the tracked catalog and event knowledge, and
/// the bot's own name, never read as listed words (substring entries could).
#[test]
fn boss_names_aliases_and_the_bot_name_pass_the_built_in_list() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog = kanade::infrastructure::files::load_catalog(&root.join("boss/bosses.yaml"))
        .expect("tracked catalog");
    let knowledge = kanade::infrastructure::files::load_knowledge_dir(&root.join("boss/knowledge"))
        .expect("tracked knowledge");
    let mut names = vec!["Kanade".to_owned(), "OtonoseKanade".to_owned()];
    for boss in catalog.bosses() {
        names.push(boss.short().to_owned());
        names.push(boss.full().to_owned());
        names.extend(boss.aliases().iter().cloned());
    }
    for event in &knowledge.events {
        names.push(event.key.clone());
        names.push(event.name.clone());
        names.extend(event.aliases.iter().cloned());
    }
    assert!(names.len() > 50, "the catalog was read: {}", names.len());
    let words = WordFilter::builtin();
    let hits: Vec<(String, String)> = names
        .iter()
        .filter_map(|name| Some((name.clone(), words.denied(name)?.as_str().to_owned())))
        .collect();
    assert!(hits.is_empty(), "{hits:?}");
    // Common member names pass the chat matcher with no roster at all.
    let chat = ProfanityGuard::default();
    names.extend(
        [
            "Bob",
            "Bobby",
            "Bobbie",
            "Taiga",
            "Taiwo",
            "Dickens",
            "Cumberbatch",
            "Hoey",
        ]
        .map(str::to_owned),
    );
    let hits: Vec<(String, String)> = names
        .iter()
        .filter_map(|name| Some((name.clone(), chat.reply_hit(name)?)))
        .collect();
    assert!(hits.is_empty(), "{hits:?}");
    assert!(builtin_words().contains(&"babi"));
}

/// Grounded records are store data, not the model's words: a listed word
/// that only the appended record carries (here the admin extra word
/// `planned`, the run's status) never triggers, so no clean retry is spent
/// and the storm guard counts nothing. A party member named Bob is on it too.
#[tokio::test(start_paused = true)]
async fn grounded_records_from_store_data_are_not_checked() {
    let guard = ProfanityGuard::new(&Profanity {
        extra_words: vec!["planned".into()],
        ..Profanity::default()
    })
    .with_names(["Bob".to_owned()]);
    let said = "Bob, your Malefic Star run a1a1a1a1 is tonight.";
    let run = run_in(vec![wants_schedule(), words(said)], &guard, true, |input| {
        input["world"]["members"][0]["display_name"] = json!("Bob")
    })
    .await;
    assert!(
        run.generation.reply.contains("`planned`"),
        "the record was appended: {}",
        run.generation.reply
    );
    assert_eq!(run.generation.profanity, None);
    assert_eq!(run.requests.len(), 2, "no clean retry");
    assert!(!run.generation.clean_retry, "nothing for the storm guard");
    assert_eq!(chat_outcome(&run.generation), ChatOutcome::Answered);
    // The same word in the model's own sentence is checked.
    let run = run_in(
        vec![wants_schedule(), words("It is planned."), words("Tonight.")],
        &guard,
        true,
        |_| {},
    )
    .await;
    assert_eq!(run.requests.len(), 3, "the model's word spends the retry");
    assert_eq!(chat_outcome(&run.generation), ChatOutcome::Profanity);
}

fn wants_schedule() -> FakeAction {
    FakeAction::Response(kanade::infrastructure::llm::CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: MODEL.into(),
        content: None,
        tool_calls: vec![kanade::infrastructure::llm::ToolCall {
            id: "s1".into(),
            name: "get_schedule".into(),
            arguments: json!({"scope": "all"}).to_string(),
        }],
        finish_reason: kanade::infrastructure::llm::FinishReason::ToolCalls,
        usage: None,
    })
}

/// An unposted write replaces the model's reply with fixed status text, so
/// its words are never shown: no check, no retry, no profanity row.
#[tokio::test(start_paused = true)]
async fn a_replaced_write_reply_is_not_checked() {
    let propose = FakeAction::Response(kanade::infrastructure::llm::CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: MODEL.into(),
        content: None,
        tool_calls: vec![kanade::infrastructure::llm::ToolCall {
            id: "m1".into(),
            name: "propose_move".into(),
            arguments: json!({"run_query": "hstar", "to_when": "thu 22:00"}).to_string(),
        }],
        finish_reason: kanade::infrastructure::llm::FinishReason::ToolCalls,
        usage: None,
    });
    let run = run_in(
        vec![propose, words("Card's up, shit yeah!")],
        &ProfanityGuard::default(),
        true,
        |input| input[PORTS_FAIL] = json!(true),
    )
    .await;
    assert!(
        run.generation
            .reply
            .starts_with("The requested card was not posted."),
        "{}",
        run.generation.reply
    );
    assert_eq!(run.generation.profanity, None);
    assert_eq!(run.requests.len(), 2, "no clean retry");
    assert_eq!(chat_outcome(&run.generation), ChatOutcome::Refused);
}
