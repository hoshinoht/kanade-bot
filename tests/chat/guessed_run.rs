//! D-GUESSED-RUN: unlike v4, a run-taking write tool refuses a run the
//! model chose itself (typically an id from its own read) when the asker's
//! words fit more than one run, with the same "Ask which one" listing a
//! vague `run_query` gets. Words that single the run out, a typed id, and a
//! follow-up answer to the bot's own question still produce the card.

use std::sync::Arc;
use std::time::Duration;

use kanade::chat::answer::{AnswerDeps, AnswerSettings, Generation, Question, answer};
use kanade::chat::tools::REFUSED;
use kanade::chat::tools::bundles::ToolOffer;
use kanade::domain::schedule::{Change, Run, RunSource, RunStatus};
use kanade::infrastructure::llm::{
    CompletionResponse, FakeAction, FakeProvider, FinishReason, Message, ModelCapabilities,
    ToolCall,
};
use serde_json::{Value, json};

use crate::common::utc;
use crate::looping::{Ports, said};
use crate::model::{Scripted, client};
use crate::world::World;

const MODEL: &str = "synthetic-guessed";
const ASTER: &str = "100000000000000101";
const BRAMBLE: &str = "100000000000000102";
const COBALT: &str = "100000000000000103";
const HOME: &str = "100000000000000201";

/// The V02 base world (invented): Tue 13 Oct 2026 15:30 SGT, three runs on
/// Wednesday (Hard Malefic Star 21:30 and Extreme Kalos 23:00 here, Normal
/// Baldrix 22:00 in the other channel).
async fn world() -> World {
    let run = |id: &str, bosses: &[&str], at: &str, channel: &str, party: &[&str], status: &str| {
        let week = if at < "2026-10-15" {
            "2026-10-08T00:00:00+08:00"
        } else {
            "2026-10-15T00:00:00+08:00"
        };
        json!({"id": id, "bosses": bosses, "at": format!("{at}:00+08:00"), "channel_id": channel,
               "participants": party, "status": status, "week_start": week, "fixed_run_id": null})
    };
    let other = "100000000000000202";
    let (aster, bramble, cobalt) = (ASTER, BRAMBLE, COBALT);
    let (dune, fennel) = ("100000000000000104", "100000000000000105");
    let member =
        |id: &str, name: &str| json!({"user_id": id, "display_name": name, "has_role": true});
    World::new(&json!({
        "clock": "2026-10-13T15:30:00+08:00", "timezone": "Asia/Singapore",
        "reset_weekday": 3, "reset_time": "00:00",
        "uuid_sequence": (1..40).map(|n| format!("e0e0e0e0-0000-4000-8000-{n:012}")).collect::<Vec<_>>(),
        "catalog": {"bosses": [
                {"short": "MaleficStar", "full": "Malefic Star", "difficulties": ["h"], "aliases": ["star", "mstar"]},
                {"short": "FA", "full": "First Adversary", "difficulties": ["h"], "aliases": ["fa"]},
                {"short": "Carling", "full": "Chief Carling", "difficulties": ["h"], "aliases": ["carl"]},
                {"short": "Kalos", "full": "Kalos the Guardian", "difficulties": ["x"], "aliases": ["kalos"]},
                {"short": "Baldrix", "full": "Baldrix", "difficulties": ["n"], "aliases": ["baldrix"]},
                {"short": "Limbo", "full": "Limbo", "difficulties": ["h"], "aliases": ["limbo"]},
                {"short": "Bellona", "full": "Bellona", "difficulties": ["h"], "aliases": ["bellona"]}],
            "difficulties": [{"prefix": "n", "label": "Normal"}, {"prefix": "h", "label": "Hard"},
                             {"prefix": "x", "label": "Extreme"}]},
        "channels": [{"id": HOME, "name": "boss-alpha"}, {"id": other, "name": "boss-beta"}],
        "settings": {"guild_id": "100000000000000900", "watched_channel_ids": format!("{HOME},{other}"),
                     "chat_pilot_channel_ids": format!("{HOME},{other}"), "chat_pilot_category_ids": ""},
        "bot_user": {"id": "100000000000000001", "name": "Kanade"}, "self_role_id": null, "guides": [],
        "world": {
            "members": [member(aster, "Aster"), member(bramble, "Bramble"), member(cobalt, "Cobalt"),
                        member(dune, "Dune"), member(fennel, "Fennel")],
            "fixed": [], "rsvps": [],
            "runs": [
                run("a1000001", &["HMaleficStar", "HFA"], "2026-10-12T21:30", HOME, &[aster, bramble, cobalt], "done"),
                run("a1000002", &["HCarling"], "2026-10-13T21:30", HOME, &[aster, bramble], "planned"),
                run("a1000003", &["HMaleficStar"], "2026-10-14T21:30", HOME, &[aster, cobalt, dune], "planned"),
                run("b2000005", &["NBaldrix"], "2026-10-14T22:00", other, &[dune, fennel], "planned"),
                run("a1000004", &["XKalos"], "2026-10-14T23:00", HOME, &[bramble, cobalt], "planned"),
                run("a1000006", &["HLimbo"], "2026-10-16T22:00", HOME, &[aster, fennel], "planned"),
                run("a1000008", &["HFA"], "2026-10-17T21:00", HOME, &[aster, cobalt], "cancelled"),
                run("b2000007", &["HBellona"], "2026-10-19T21:00", other, &[cobalt, fennel], "planned"),
            ]}
    }))
    .await
}

fn calls(calls: &[(&str, Value)]) -> FakeAction {
    FakeAction::Response(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: MODEL.into(),
        content: None,
        tool_calls: calls
            .iter()
            .map(|(name, arguments)| ToolCall {
                // Unique across rounds: a transcript never repeats an id.
                id: format!("call-{name}"),
                name: (*name).into(),
                arguments: arguments.to_string(),
            })
            .collect(),
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    })
}

fn user(text: &str) -> Message {
    Message::User {
        content: text.into(),
    }
}

fn bot(text: &str) -> Message {
    Message::Assistant {
        content: Some(text.into()),
        tool_calls: Vec::new(),
    }
}

/// One question as the driver hands it over: the conversation the model
/// sees (member turns read `Name: text`), the model's scripted tool calls,
/// then its words.
async fn ask(
    world: &mut World,
    asker: &str,
    turns: Vec<Message>,
    actions: Vec<FakeAction>,
) -> (Generation, Ports) {
    let mut caps = ModelCapabilities::minimal();
    caps.function_tools = true;
    let provider = Arc::new(Scripted {
        caps,
        fake: FakeProvider::new(actions),
    });
    let (_governor, client) = client(Some(MODEL), provider);
    let deps = AnswerDeps {
        client: &client,
        route: None,
    };
    let ctx = world.context(&json!({"author_id": asker, "channel_id": HOME}));
    let mut conversation = vec![Message::System {
        content: "SYSTEM".into(),
    }];
    conversation.extend(turns);
    let ports = Ports::default();
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ctx,
            conversation,
            profanity: None,
            reminder: "Keep the tool facts exact.".into(),
            offer: ToolOffer::full_set(false),
            settings: AnswerSettings {
                tool_rounds: 4,
                timeout: Duration::from_secs(60),
                reasoning: None,
                temperature: None,
                max_output_tokens: 1024,
                model_context_tokens: 32768,
                clean_retry: false,
            },
        };
        answer(&deps, question, &guild, &mut proposer, &ports).await
    };
    assert_eq!(generation.failure, None, "{:?}", generation.failure);
    (generation, ports)
}

fn write_outcome<'g>(
    generation: &'g Generation,
    tool: &str,
) -> &'g kanade::chat::tools::ToolOutcome {
    &generation
        .outcomes
        .iter()
        .find(|round| round.outcome.name == tool)
        .unwrap_or_else(|| panic!("no {tool} call"))
        .outcome
}

/// One card, about `run`.
fn carded(generation: &Generation, ports: &Ports, tool: &str, run: &str) {
    let outcome = write_outcome(generation, tool);
    assert!(outcome.ok, "{}", outcome.output);
    let posted = ports.posted.lock().unwrap();
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].run_id.as_deref(), Some(run));
    assert_eq!(generation.created.len(), 1);
}

/// No card, and the tool asked the model to ask which run.
fn refused(generation: &Generation, ports: &Ports, tool: &str, asked: &str, listed: &[&str]) {
    let outcome = write_outcome(generation, tool);
    assert_eq!(outcome.error, Some(REFUSED));
    assert!(
        outcome.output.starts_with(&format!(
            "`{asked}` matches more than one run. Ask which one:"
        )),
        "{}",
        outcome.output
    );
    for id in listed {
        assert!(
            outcome.output.contains(id),
            "{id} listed: {}",
            outcome.output
        );
    }
    assert!(generation.created.is_empty() && generation.posted.is_empty());
    assert!(ports.posted.lock().unwrap().is_empty());
}

#[tokio::test]
async fn c15_an_id_from_the_models_own_read_is_refused_when_the_day_has_several_runs() {
    let mut world = world().await;
    let (generation, ports) = ask(
        &mut world,
        COBALT,
        vec![user("Cobalt: move the wednesday run to 22:00")],
        vec![
            calls(&[("get_schedule", json!({"week": "auto", "day": "wednesday"}))]),
            calls(&[(
                "propose_move",
                json!({"run_query": "a1000003", "to_when": "wed 22:00"}),
            )]),
            FakeAction::Response(said(
                MODEL,
                "Which one -- the HStar at 21:30 or the Kalos at 23:00?",
            )),
        ],
    )
    .await;
    refused(
        &generation,
        &ports,
        "propose_move",
        "move the wednesday run to 22:00",
        &["a1000003", "b2000005", "a1000004"],
    );
}

#[tokio::test]
async fn a_description_the_model_made_up_is_refused_the_same_way() {
    let mut world = world().await;
    let (generation, ports) = ask(
        &mut world,
        COBALT,
        vec![user("Cobalt: move the wednesday run to 22:00")],
        vec![
            calls(&[(
                "propose_move",
                json!({"run_query": "kalos wednesday", "to_when": "wed 22:00"}),
            )]),
            FakeAction::Response(said(MODEL, "Which one?")),
        ],
    )
    .await;
    refused(
        &generation,
        &ports,
        "propose_move",
        "move the wednesday run to 22:00",
        &["a1000003", "a1000004"],
    );
}

#[tokio::test]
async fn c13_a_boss_and_day_that_single_out_the_run_keep_the_card() {
    let mut world = world().await;
    let (generation, ports) = ask(
        &mut world,
        BRAMBLE,
        vec![user("Bramble: can we push the kalos tomorrow to 23:30?")],
        vec![
            calls(&[("get_schedule", json!({"week": "auto", "day": "wednesday"}))]),
            calls(&[(
                "propose_move",
                json!({"run_query": "a1000004", "to_when": "tomorrow 23:30"}),
            )]),
            FakeAction::Response(said(MODEL, "Card's up -- it needs a ✅.")),
        ],
    )
    .await;
    carded(&generation, &ports, "propose_move", "a1000004");
}

#[tokio::test]
async fn c14_a_cancel_of_the_named_boss_on_its_day_keeps_the_card() {
    let mut world = world().await;
    let (generation, ports) = ask(
        &mut world,
        COBALT,
        vec![user(
            "Cobalt: cancel the hstar on wednesday, we can't make it",
        )],
        vec![
            calls(&[("propose_cancel", json!({"run_query": "a1000003"}))]),
            FakeAction::Response(said(MODEL, "Card's up -- it needs a ✅.")),
        ],
    )
    .await;
    carded(&generation, &ports, "propose_cancel", "a1000003");
}

#[tokio::test]
async fn an_id_the_asker_typed_is_taken_even_on_a_busy_day() {
    let mut world = world().await;
    let (generation, ports) = ask(
        &mut world,
        COBALT,
        vec![user("Cobalt: move the wednesday run #a1000003 to 22:00")],
        vec![
            calls(&[(
                "propose_move",
                json!({"run_query": "a1000003", "to_when": "wed 22:00"}),
            )]),
            FakeAction::Response(said(MODEL, "Card's up.")),
        ],
    )
    .await;
    carded(&generation, &ports, "propose_move", "a1000003");
}

#[tokio::test]
async fn a_different_run_than_the_one_the_asker_singled_out_is_refused() {
    let mut world = world().await;
    let (generation, ports) = ask(
        &mut world,
        COBALT,
        vec![user("Cobalt: cancel the HStar on Wednesday")],
        vec![
            calls(&[("propose_cancel", json!({"run_query": "a1000004"}))]),
            FakeAction::Response(said(MODEL, "Which one?")),
        ],
    )
    .await;
    refused(
        &generation,
        &ports,
        "propose_cancel",
        "cancel the HStar on Wednesday",
        &["a1000003", "a1000004"],
    );
}

#[tokio::test]
async fn words_that_fit_no_open_run_refuse_the_models_pick() {
    let mut world = world().await;
    let (generation, ports) = ask(
        &mut world,
        COBALT,
        vec![user("Cobalt: cancel the limbo on wednesday")],
        vec![
            calls(&[("propose_cancel", json!({"run_query": "a1000003"}))]),
            FakeAction::Response(said(MODEL, "There's no Limbo on Wednesday.")),
        ],
    )
    .await;
    let outcome = write_outcome(&generation, "propose_cancel");
    assert_eq!(outcome.error, Some(REFUSED));
    assert!(
        outcome
            .output
            .starts_with("No run matches `cancel the limbo on wednesday`."),
        "{}",
        outcome.output
    );
    assert!(generation.created.is_empty() && ports.posted.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_day_a_move_goes_to_does_not_describe_the_run() {
    let mut world = world().await;
    let (generation, ports) = ask(
        &mut world,
        COBALT,
        vec![user("Cobalt: move the hstar to friday 21:30")],
        vec![
            calls(&[(
                "propose_move",
                json!({"run_query": "a1000003", "to_when": "fri 21:30"}),
            )]),
            FakeAction::Response(said(MODEL, "Card's up.")),
        ],
    )
    .await;
    carded(&generation, &ports, "propose_move", "a1000003");
}

const WHICH: &str = "Which one -- HStar 21:30, Baldrix 22:00 in #boss-beta, or Kalos 23:00?";

#[tokio::test]
async fn the_answer_to_the_bots_which_one_question_produces_the_card() {
    let mut world = world().await;
    let (generation, ports) = ask(
        &mut world,
        COBALT,
        vec![
            user("Cobalt: move the wednesday run to 22:00"),
            bot(WHICH),
            user("Cobalt: the hstar one"),
        ],
        vec![
            calls(&[(
                "propose_move",
                json!({"run_query": "a1000003", "to_when": "wed 22:00"}),
            )]),
            FakeAction::Response(said(MODEL, "Card's up.")),
        ],
    )
    .await;
    carded(&generation, &ports, "propose_move", "a1000003");
}

/// A second Hard Malefic Star on Saturday: "the hstar one" alone fits two
/// runs, and only the question it answers ("the wednesday run") leaves one.
async fn two_hstars() -> World {
    let world = world().await;
    world
        .put(Change::PutRun(Run {
            id: "a1000009".into(),
            source: RunSource::Amend,
            fixed_run_id: None,
            channel_id: Some(HOME.into()),
            week_start: utc(&json!("2026-10-15T00:00:00+08:00")),
            datetime: utc(&json!("2026-10-17T21:30:00+08:00")),
            bosses: vec!["HMaleficStar".into()],
            participants: vec![ASTER.into(), COBALT.into()],
            status: RunStatus::Planned,
            attendance: Vec::new(),
            status_pin: None,
        }))
        .await;
    world
}

#[tokio::test]
async fn a_follow_up_is_read_with_the_question_it_answers() {
    let mut world = two_hstars().await;
    let (generation, ports) = ask(
        &mut world,
        COBALT,
        vec![
            user("Cobalt: move the wednesday run to 22:00"),
            bot(WHICH),
            user("Cobalt: the hstar one"),
        ],
        vec![
            calls(&[(
                "propose_move",
                json!({"run_query": "a1000003", "to_when": "wed 22:00"}),
            )]),
            FakeAction::Response(said(MODEL, "Card's up.")),
        ],
    )
    .await;
    carded(&generation, &ports, "propose_move", "a1000003");
}

#[tokio::test]
async fn somebody_elses_message_is_never_read_as_the_askers() {
    let mut world = two_hstars().await;
    let (generation, ports) = ask(
        &mut world,
        COBALT,
        vec![
            user("Aster: move the wednesday run to 22:00"),
            bot(WHICH),
            user("Cobalt: the hstar one"),
        ],
        vec![
            calls(&[(
                "propose_move",
                json!({"run_query": "a1000003", "to_when": "wed 22:00"}),
            )]),
            FakeAction::Response(said(MODEL, "Which hstar?")),
        ],
    )
    .await;
    refused(
        &generation,
        &ports,
        "propose_move",
        "the hstar one",
        &["a1000003", "a1000009"],
    );
}

#[tokio::test]
async fn an_rsvp_weighs_only_the_runs_the_asker_is_on() {
    for (asker, name, outcome) in [(ASTER, "Aster", true), (COBALT, "Cobalt", false)] {
        let mut world = world().await;
        let (generation, ports) = ask(
            &mut world,
            asker,
            vec![user(&format!("{name}: can't make it wednesday, sorry"))],
            vec![
                calls(&[(
                    "propose_rsvp",
                    json!({"run_query": "a1000003", "answer": "no"}),
                )]),
                FakeAction::Response(said(MODEL, "Noted.")),
            ],
        )
        .await;
        if outcome {
            carded(&generation, &ports, "propose_rsvp", "a1000003");
        } else {
            // Cobalt is on both Wednesday runs here.
            refused(
                &generation,
                &ports,
                "propose_rsvp",
                "can't make it wednesday, sorry",
                &["a1000003", "a1000004"],
            );
            assert!(
                !write_outcome(&generation, "propose_rsvp")
                    .output
                    .contains("b2000005")
            );
        }
    }
}
