//! `D-MIXED-PEOPLE`: a question naming other roster members together with
//! the asker ("what do Bramble and I have on wednesday?") is answered only
//! after the asker and every named person are covered by a `get_schedule`
//! read; code reads whoever the model left out, in its last read's scope,
//! and the model answers again inside the round cap. Self-only,
//! third-person-only and unrecognised-mention questions keep their reads.
//! Wholly invented fixtures (the V02 world's names and runs); the fake
//! provider replays the calls a live model made.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use kanade::chat::answer::{AnswerDeps, Generation, Question, answer};
use kanade::chat::pilot::ChatPilot;
use kanade::chat::sanitize::{schedule_defaults, schedule_people};
use kanade::chat::tools::ToolContext;
use kanade::domain::members::MemberProfile;
use kanade::infrastructure::llm::governor::DEFAULT_TOOL_ROUNDS;
use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, FakeAction, FakeProvider, FinishReason, Message, ToolCall,
};
use serde_json::{Value, json};

use crate::looping::{Ports, said, settings};
use crate::model::{Scripted, capabilities, client};
use crate::support::load;
use crate::world::World;

const BOT: &str = "5000";
const ROLE: &str = "5001";
const CHANNEL: &str = "700";
const MODEL: &str = "synthetic-chat";
const ASTER: &str = "101";
const BRAMBLE: &str = "102";
const COBALT: &str = "103";
const QUESTION: &str = "<@5000> what do Bramble and I have on wednesday?";

fn run(id: &str, bosses: &[&str], at: &str, channel: &str, party: &[&str]) -> Value {
    json!({
        "id": format!("{id}-0000-4000-8000-000000000000"),
        "bosses": bosses,
        "at": at,
        "channel_id": channel,
        "fixed_run_id": null,
        "participants": party,
        "status": "planned",
        "week_start": "2026-10-08T00:00:00+08:00",
    })
}

/// The loop vector's settings and catalog over the V02 world: Tue 13 Oct
/// 15:30, boss weeks from Thursday.
fn input() -> Value {
    let mut input = load("loop.json")["cases"][0]["input"].clone();
    input["clock"] = json!("2026-10-13T15:30:00+08:00");
    input["timezone"] = json!("Asia/Singapore");
    input["reset_weekday"] = json!(3);
    input["reset_time"] = json!("00:00");
    let member = |id: &str, name: &str| json!({"user_id": id, "display_name": name, "nickname": null, "has_role": true});
    input["world"] = json!({
        "members": [
            member(ASTER, "Aster"),
            member(BRAMBLE, "Bramble"),
            member(COBALT, "Cobalt"),
            member("104", "Dune"),
            member("105", "Fennel"),
        ],
        "fixed": [],
        "rsvps": [],
        "runs": [
            run("a1000002", &["HCarling"], "2026-10-13T21:30:00+08:00", "900", &[ASTER, BRAMBLE]),
            run("a1000003", &["HMaleficStar"], "2026-10-14T21:30:00+08:00", "900", &[ASTER, COBALT, "104"]),
            run("b2000005", &["NBaldrix"], "2026-10-14T22:00:00+08:00", "901", &["104", "105"]),
            run("a1000004", &["XKalos"], "2026-10-14T23:00:00+08:00", "900", &[BRAMBLE, COBALT]),
        ],
    });
    input
}

fn roster(world: &World) -> Vec<MemberProfile> {
    world
        .guild_view()
        .members
        .iter()
        .map(|member| MemberProfile {
            member: member.clone(),
            ..MemberProfile::default()
        })
        .collect()
}

/// The driver's trusted tool context (`driver/run.rs` `tool_context`).
fn tool_context(world: &World, asker: &str, content: &str) -> ToolContext {
    let mut ctx = world.context(&json!({"author_id": asker, "channel_id": CHANNEL}));
    let defaults = schedule_defaults(content, Some(BOT), Some(ROLE));
    ctx.force_all_channels = defaults.force_all_channels;
    ctx.force_channel_scope = defaults.force_channel_scope;
    ctx.force_group_schedule = defaults.force_group_schedule;
    ctx.self_schedule_requested = defaults.self_schedule_requested;
    ctx.upcoming_only = defaults.upcoming_only;
    ctx.next_only = defaults.next_only;
    ctx.schedule_people = schedule_people(content, Some(BOT), Some(ROLE), asker, &roster(world));
    ctx
}

static CALL_IDS: AtomicUsize = AtomicUsize::new(0);

fn wants(calls: &[(&str, Value)]) -> FakeAction {
    FakeAction::Response(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: MODEL.into(),
        content: None,
        tool_calls: calls
            .iter()
            .map(|(name, arguments)| ToolCall {
                // Unique across rounds: the runner rejects a reused id.
                id: format!("call-{}", CALL_IDS.fetch_add(1, Ordering::Relaxed)),
                name: (*name).into(),
                arguments: arguments.to_string(),
            })
            .collect(),
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    })
}

fn words(text: &str) -> FakeAction {
    FakeAction::Response(said(MODEL, text))
}

struct Asked {
    ctx: ToolContext,
    requests: Vec<ChatRequest>,
    generation: Generation,
}

async fn ask(asker: &str, content: &str, rounds: u8, actions: Vec<FakeAction>) -> Asked {
    let input = input();
    let mut world = World::new(&input).await;
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new(actions),
        caps: capabilities(&input["caps"]),
    });
    let (_governor, client) = client(Some(MODEL), provider.clone());
    let ctx = tool_context(&world, asker, content);
    let ports = Ports::default();
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ctx,
            conversation: vec![
                Message::System {
                    content: "SYSTEM".into(),
                },
                Message::User {
                    content: format!("Aster: {content}"),
                },
            ],
            reminder: "Stay in voice.".into(),
            offer: ChatPilot::route(content, None, ctx.read_only),
            settings: settings(&input, rounds),
            profanity: None,
        };
        let deps = AnswerDeps {
            client: &client,
            route: None,
        };
        answer(&deps, question, &guild, &mut proposer, &ports).await
    };
    Asked {
        ctx,
        requests: provider.fake.requests(),
        generation,
    }
}

/// `(participant argument, ok, output)` of every `get_schedule` call.
fn reads(generation: &Generation) -> Vec<(String, bool, String)> {
    generation
        .outcomes
        .iter()
        .filter(|o| o.outcome.name == "get_schedule")
        .map(|o| {
            (
                o.outcome
                    .arguments
                    .get("participant")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                o.outcome.ok,
                o.outcome.output.clone(),
            )
        })
        .collect()
}

fn tool_results(request: &ChatRequest) -> Vec<&str> {
    request
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::Tool { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect()
}

const DRAFT: &str =
    "Papa, you have Hard MaleficStar on Wed 14 Oct at 21:30. Bramble isn't listed on this run.";
const BOTH: &str = "You have [a1000003] at 21:30, and Bramble has [a1000004] at 23:00.";
/// [`BOTH`] grounded on each person's listing, asker first.
const GROUNDED_BOTH: &str = "You have **Hard MaleficStar** at 21:30, and Bramble has **Extreme Kalos** at 23:00.\n\n**Your 1 run Wed 14 Oct · All channels**\n\n`[a1000003]` **Hard MaleficStar**\n*Wed 14 Oct · 21:30* · `planned` · `0/3 yes` · <#900>\n\n**Bramble's 1 run Wed 14 Oct · All channels**\n\n`[a1000004]` **Extreme Kalos**\n*Wed 14 Oct · 23:00* · `planned` · `0/2 yes` · <#900>";

fn wednesday(participant: &str) -> Value {
    json!({"participant": participant, "week": "auto", "day": "wednesday"})
}

/// C08 as it ran live (5/5): only the asker's Wednesday read, then an
/// answer that leaves Bramble out. Code reads Bramble in the same scope and
/// the model answers again.
#[tokio::test]
async fn a_mixed_question_reads_every_named_person() {
    let asked = ask(
        ASTER,
        QUESTION,
        DEFAULT_TOOL_ROUNDS,
        vec![
            wants(&[("get_schedule", wednesday("Aster"))]),
            words(DRAFT),
            words(BOTH),
        ],
    )
    .await;
    assert_eq!(asked.ctx.schedule_people, [BRAMBLE]);
    assert!(!asked.ctx.self_schedule_requested, "never self-only");
    let reads = reads(&asked.generation);
    assert_eq!(reads.len(), 2, "{reads:?}");
    let (who, ok, aster) = &reads[0];
    assert_eq!((who.as_str(), *ok), ("Aster", true));
    assert!(aster.starts_with("**Your 1 run Wed 14 Oct"), "{aster}");
    let (who, ok, bramble) = &reads[1];
    assert_eq!((who.as_str(), *ok), ("<@102>", true));
    assert!(
        bramble.starts_with("**Bramble's 1 run Wed 14 Oct"),
        "{bramble}"
    );
    assert!(bramble.contains("[a1000004]"), "{bramble}");
    assert!(
        !bramble.contains("[a1000003]"),
        "Aster's run is not Bramble's"
    );
    // The model answered again over both reads, in words, inside the cap.
    assert_eq!(asked.requests.len(), 3);
    assert_eq!(asked.generation.rounds, 3);
    assert!(asked.requests[2].tools.is_empty(), "an answer in words");
    let results = tool_results(&asked.requests[2]);
    assert_eq!(results.len(), 2);
    assert!(results[1].starts_with("**Bramble's 1 run"), "{results:?}");
    // Grounded against both people's runs, each under the model's words.
    assert_eq!(asked.generation.reply, GROUNDED_BOTH);
    assert!(asked.generation.failure.is_none());
}

/// The next round is the last (no tools): whoever is unread is read before
/// it is sent, so its one answer covers everyone and no request goes past
/// the cap.
#[tokio::test]
async fn a_mixed_question_is_read_in_full_before_the_last_round() {
    let asked = ask(
        ASTER,
        QUESTION,
        2,
        vec![wants(&[("get_schedule", wednesday("me"))]), words(BOTH)],
    )
    .await;
    assert_eq!(asked.requests.len(), 2);
    assert!(asked.requests[1].tools.is_empty());
    let results = tool_results(&asked.requests[1]);
    assert_eq!(results.len(), 2);
    assert!(results[1].starts_with("**Bramble's 1 run"), "{results:?}");
    let reads = reads(&asked.generation);
    assert_eq!(reads.len(), 2, "{reads:?}");
    assert_eq!(reads[1].0, "<@102>");
    let reply = &asked.generation.reply;
    assert!(reply.contains("Bramble has"), "{reply}");
    assert!(reply.contains("`[a1000004]` **Extreme Kalos**"), "{reply}");
}

/// A read of another day does not cover the asker: after a Tuesday self
/// read and a Wednesday read of Bramble, code reads the asker's Wednesday.
#[tokio::test]
async fn an_earlier_read_of_another_day_does_not_cover_the_asker() {
    let tuesday = json!({"participant": "me", "week": "auto", "day": "tuesday"});
    let asked = ask(
        ASTER,
        QUESTION,
        DEFAULT_TOOL_ROUNDS,
        vec![
            wants(&[("get_schedule", tuesday)]),
            wants(&[("get_schedule", wednesday("Bramble"))]),
            words("You have Carling tonight, and Bramble has [a1000004] at 23:00."),
            words(BOTH),
        ],
    )
    .await;
    let reads = reads(&asked.generation);
    assert_eq!(reads.len(), 3, "{reads:?}");
    let (who, ok, aster) = &reads[2];
    assert_eq!((who.as_str(), *ok), ("me", true));
    assert!(aster.starts_with("**Your 1 run Wed 14 Oct"), "{aster}");
    assert_eq!(
        asked.generation.outcomes[2].outcome.arguments,
        *wednesday("me").as_object().expect("object")
    );
    // The draft is answered again, in words, inside the cap.
    assert_eq!(asked.requests.len(), 4);
    assert!(asked.requests[3].tools.is_empty());
    assert_eq!(asked.generation.reply, GROUNDED_BOTH);
}

/// A model that read both people, or the group, is answered as it wrote,
/// grounded on each person's listing (not only the latest read).
#[tokio::test]
async fn a_mixed_question_read_in_full_needs_no_extra_round() {
    let both = ask(
        ASTER,
        QUESTION,
        DEFAULT_TOOL_ROUNDS,
        vec![
            wants(&[
                ("get_schedule", wednesday("me")),
                ("get_schedule", wednesday("Bramble")),
            ]),
            words(BOTH),
        ],
    )
    .await;
    assert_eq!(both.requests.len(), 2);
    assert_eq!(reads(&both.generation).len(), 2);
    assert_eq!(both.generation.reply, GROUNDED_BOTH);

    let group = |answer: &'static str| {
        ask(
            ASTER,
            QUESTION,
            DEFAULT_TOOL_ROUNDS,
            vec![
                wants(&[("get_schedule", json!({"week": "auto", "day": "wednesday"}))]),
                words(answer),
            ],
        )
    };
    let named = group(BOTH).await;
    assert_eq!(named.requests.len(), 2);
    assert_eq!(reads(&named.generation).len(), 1);
    assert_eq!(named.generation.reply, GROUNDED_BOTH);
    // A reply naming no run (as live: "one run each:") gets both people's
    // listings, each under its own heading, and nobody else's runs.
    let unnamed = group("Papa~ Wednesday has one run each:").await;
    let reply = &unnamed.generation.reply;
    let listings = GROUNDED_BOTH.split_once("\n\n").expect("listing").1;
    assert_eq!(
        *reply,
        format!("Papa~ Wednesday has one run each:\n\n{listings}")
    );
    assert!(!reply.contains("[b2000005]"), "nobody asked about: {reply}");
}

/// A run two of them share is under each person's heading.
#[tokio::test]
async fn a_shared_run_is_under_each_persons_heading() {
    let tuesday =
        |participant: &str| json!({"participant": participant, "week": "auto", "day": "tuesday"});
    let asked = ask(
        ASTER,
        "<@5000> what do Bramble and I have on tuesday?",
        DEFAULT_TOOL_ROUNDS,
        vec![
            wants(&[("get_schedule", tuesday("me"))]),
            words("We're both on [a1000002] at 21:30 tonight."),
            words("We're both on [a1000002] at 21:30 tonight."),
        ],
    )
    .await;
    let reads = reads(&asked.generation);
    assert_eq!(reads.len(), 2, "{reads:?}");
    assert_eq!(reads[1].0, "<@102>");
    let record =
        "`[a1000002]` **Hard Carling**\n*Tue 13 Oct · 21:30* · `planned` · `0/2 yes` · <#900>";
    assert_eq!(
        asked.generation.reply,
        format!(
            "We're both on **Hard Carling** at 21:30 tonight.\n\n**Your 1 run Tue 13 Oct · All channels**\n\n{record}\n\n**Bramble's 1 run Tue 13 Oct · All channels**\n\n{record}"
        )
    );
}

/// A clarifying answer with no schedule read is not forced into reads.
#[tokio::test]
async fn a_mixed_question_answered_without_a_read_is_left_alone() {
    let asked = ask(
        ASTER,
        "<@5000> what do Bramble and I have?",
        DEFAULT_TOOL_ROUNDS,
        vec![words("Which week do you mean?")],
    )
    .await;
    assert_eq!(asked.ctx.schedule_people, [BRAMBLE]);
    assert_eq!(asked.requests.len(), 1);
    assert!(asked.generation.outcomes.is_empty());
}

/// C06: the trusted self-only recovery is untouched.
#[tokio::test]
async fn a_self_only_question_keeps_its_one_read() {
    let content = "<@5000> when is the next run for me?";
    let asked = ask(
        ASTER,
        content,
        DEFAULT_TOOL_ROUNDS,
        vec![
            wants(&[(
                "get_schedule",
                json!({"week": "auto", "participant": "Aster"}),
            )]),
            words("Tonight, Carling at 21:30."),
        ],
    )
    .await;
    assert!(asked.ctx.schedule_people.is_empty());
    assert!(asked.ctx.self_schedule_requested);
    assert_eq!(asked.requests.len(), 2);
    let reads = reads(&asked.generation);
    assert_eq!(reads.len(), 1);
    assert!(reads[0].2.starts_with("**Your next run"), "{reads:?}");
}

/// C07: a third-person-only question reads only that person.
#[tokio::test]
async fn a_third_person_question_keeps_its_one_read() {
    let content = "<@5000> what runs does Cobalt have this week?";
    let asked = ask(
        ASTER,
        content,
        DEFAULT_TOOL_ROUNDS,
        vec![
            wants(&[(
                "get_schedule",
                json!({"week": "this", "participant": "Cobalt"}),
            )]),
            words("Cobalt has two runs on Wednesday."),
        ],
    )
    .await;
    assert!(asked.ctx.schedule_people.is_empty());
    assert_eq!(asked.requests.len(), 2);
    let reads = reads(&asked.generation);
    assert_eq!(reads.len(), 1);
    assert!(reads[0].2.starts_with("**Cobalt's 2 runs"), "{reads:?}");
}

/// C09: an unrecognised mention is still refused and asked about; the
/// asker's runs are never substituted.
#[tokio::test]
async fn an_unrecognised_mention_is_still_asked_about() {
    let content = "<@5000> what's on for <@100000000000000999> this week?";
    let asked = ask(
        ASTER,
        content,
        DEFAULT_TOOL_ROUNDS,
        vec![
            wants(&[(
                "get_schedule",
                json!({"week": "this", "participant": "<@100000000000000999>"}),
            )]),
            words("That mention isn't on the roster. Who do you mean?"),
        ],
    )
    .await;
    assert!(asked.ctx.schedule_people.is_empty());
    assert_eq!(asked.requests.len(), 2);
    let reads = reads(&asked.generation);
    assert_eq!(reads.len(), 1);
    assert!(!reads[0].1, "refused: {reads:?}");
    assert!(asked.generation.reply.ends_with("Who do you mean?"));
}

#[tokio::test]
async fn only_a_question_naming_others_with_the_asker_is_mixed() {
    let world = World::new(&input()).await;
    let roster = roster(&world);
    let people = |text: &str| schedule_people(text, Some(BOT), Some(ROLE), ASTER, &roster);
    assert_eq!(people(QUESTION), [BRAMBLE]);
    assert_eq!(people("what do bramble and me have tmr"), [BRAMBLE]);
    assert_eq!(people("are <@103> and I on kalos?"), [COBALT]);
    assert_eq!(
        people("when do we and Cobalt and Bramble run?"),
        [COBALT, BRAMBLE]
    );
    assert_eq!(people("<@5000> what's on for me and Bramble"), [BRAMBLE]);
    for single in [
        "<@5000> when is the next run for me?",
        "show my runs this week",
        "what runs does Cobalt have this week?",
        "what's on for <@100000000000000999> this week?",
        "what do Aster and I have?",
        "what do Brambles and I have?",
        "can you put Bramble down as yes for tonight's carling?",
    ] {
        assert!(people(single).is_empty(), "{single}");
    }
}
