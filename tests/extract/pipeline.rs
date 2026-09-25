//! The live pipeline end to end on paused tokio time and a pinned wall clock:
//! debounce, loop guards, the governed call with its answer retry, outcomes
//! in the extraction log, proposals through the scheduler, and the backlog.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use kanade::domain::drafts::{DraftStatus, ProposalStore};
use kanade::domain::model_log::{ExtractionOutcome, ExtractionRefusal, ModelLogStore};
use kanade::domain::schedule::RsvpState;
use kanade::extract::AmendmentKind;
use kanade::extract::pipeline::{
    AuthorKind, MessageEvent, check_reasoning_effort, extraction_outcome,
};
use kanade::infrastructure::llm::identity::{TaggingCodec, find_request_leaks};
use kanade::infrastructure::llm::{Effort, FakeAction, ModelCapabilities};

use crate::fakes::{
    ALIAS, CHANNEL, MY, OTHER, PRIYA, STRANGER, World, after, filtered, local, message, nothing,
    replayed, reply,
};

fn moved(time: &str, evidence: &str) -> FakeAction {
    reply(&format!(
        r#"{{"amendments": [{{"kind": "move", "bosses": ["HMaleficStar", "HFA"],
            "day_ref": "wed", "time_ref": "{time}", "participants": ["{MY}"],
            "confidence": 0.9, "evidence_message_ids": ["{evidence}"]}}],
          "summary": "proposed for wed"}}"#
    ))
}

fn post(text: &str) -> MessageEvent {
    MessageEvent::Posted(message("101", MY, local(8, 30, 13, 1), text))
}

const MOVE_TEXT: &str = "mon cannot, change to wed 9:30pm?";

#[tokio::test(start_paused = true)]
async fn a_burst_waits_for_the_debounce_then_proposes_once() {
    let world = World::new(vec![moved("9:30pm", "101")]).await;
    let (events, _loop) = world.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(89).await;
    assert_eq!(world.requests(), 0, "still inside the 90 s debounce");
    after(2).await;
    assert_eq!(world.requests(), 1);

    let live = world.live_proposals().await;
    assert_eq!(live.len(), 1);
    let proposal = &live[0].draft;
    assert!(
        proposal
            .subject
            .as_deref()
            .unwrap()
            .contains(&world.runs[0])
    );
    let cards = world.outbox.cards.lock().unwrap().clone();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].entries.len(), 1);
    assert_eq!(cards[0].entries[0].kind, AmendmentKind::Move);
    assert_eq!(cards[0].entries[0].proposal_id, proposal.id);

    let logs = world.logs().await;
    assert_eq!(logs.len(), 1);
    let log = &logs[0];
    assert_eq!(log.outcome, ExtractionOutcome::Proposed);
    assert_eq!(log.model, ALIAS);
    assert_eq!(log.reasoning, None);
    assert_eq!(log.request_count, 1);
    assert_eq!(log.message_ids, ["101"]);
    assert_eq!(log.member_ids, [MY]);
    assert_eq!(log.proposal_ids, std::slice::from_ref(&proposal.id));
    assert_eq!(log.channel_id.as_deref(), Some(CHANNEL));
    assert!(log.latency_ms.is_some());
    assert!(log.prompt.contains("NEW MESSAGES"));
    assert!(log.prompt.contains("[101]"));
    assert!(log.raw_response.contains("HMaleficStar"));
    assert_eq!(log.error, None);
    assert_eq!(live[0].info.source_id, log.id);
    assert!(world.processed("101").await, "the burst is not re-read");
}

#[tokio::test(start_paused = true)]
async fn an_edit_pushes_the_debounce_out_and_a_ping_flushes_at_once() {
    let world = World::new(vec![nothing(), nothing()]).await;
    let (events, _loop) = world.pipeline();
    events.send(post("hstar wed 9pm?")).await.expect("send");
    after(60).await;
    let edit = message("101", MY, local(8, 30, 13, 1), "hstar wed 10pm?");
    events.send(MessageEvent::Edited(edit)).await.expect("send");
    after(40).await;
    assert_eq!(world.requests(), 0, "the edit restarted the 90 s");
    after(51).await;
    assert_eq!(world.requests(), 1);
    let prompt = &world.logs().await[0].prompt;
    assert!(prompt.contains("hstar wed 10pm?"), "the edit is read");
    assert!(!prompt.contains("hstar wed 9pm?"));

    let ping = message("102", MY, local(8, 30, 13, 5), "@here hstar tonight 9pm");
    events.send(MessageEvent::Posted(ping)).await.expect("send");
    after(1).await;
    assert_eq!(
        world.requests(),
        2,
        "a ping with a boss or time does not wait"
    );
}

#[tokio::test(start_paused = true)]
async fn bot_webhook_and_own_messages_are_never_stored_or_read() {
    let world = World::new(vec![nothing()]).await;
    let (events, _loop) = world.pipeline();
    for (id, kind) in [
        ("1", AuthorKind::Bot),
        ("2", AuthorKind::Webhook),
        ("3", AuthorKind::Myself),
    ] {
        let mut looped = message(id, MY, local(8, 30, 13, 1), "@here hstar tonight 9pm");
        looped.author = kind;
        events
            .send(MessageEvent::Posted(looped))
            .await
            .expect("send");
    }
    after(200).await;
    assert_eq!(world.requests(), 0);
    let cached = world
        .store
        .channel_messages(CHANNEL, local(8, 1, 0, 0), false)
        .await
        .expect("messages");
    assert!(cached.is_empty());
}

#[tokio::test(start_paused = true)]
async fn outsiders_chat_answers_and_a_disabled_extractor_are_cached_but_not_read() {
    let world = World::new(vec![nothing()]).await;
    let (events, _loop) = world.pipeline();
    let stranger = message("1", STRANGER, local(8, 30, 13, 1), "hstar wed 9pm?");
    let mut answered = message("2", MY, local(8, 30, 13, 2), "hstar wed 9pm?");
    answered.handled_by_chat = true;
    events
        .send(MessageEvent::Posted(stranger))
        .await
        .expect("send");
    events
        .send(MessageEvent::Posted(answered))
        .await
        .expect("send");
    after(1).await;
    world.guild.enabled.store(false, Ordering::SeqCst);
    let paused = message("3", MY, local(8, 30, 13, 3), "@here hstar tonight 9pm");
    events
        .send(MessageEvent::Posted(paused))
        .await
        .expect("send");
    after(200).await;
    assert_eq!(world.requests(), 0);
    let cached = world
        .store
        .channel_messages(CHANNEL, local(8, 1, 0, 0), false)
        .await
        .expect("messages");
    assert_eq!(cached.len(), 3, "stored whatever happens to them (v4)");
}

#[tokio::test(start_paused = true)]
async fn a_malformed_reply_gets_one_answer_retry_in_the_same_session() {
    let world = World::new(vec![reply("not json"), moved("9:30pm", "101")]).await;
    let (events, _loop) = world.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(91).await;
    let logs = world.logs().await;
    assert_eq!(logs.len(), 1, "one burst, one row");
    assert_eq!(logs[0].request_count, 2);
    assert_eq!(logs[0].outcome, ExtractionOutcome::Proposed);
    assert_eq!(world.live_proposals().await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_model_failure_is_logged_and_changes_nothing() {
    let world = World::new(vec![FakeAction::Permanent]).await;
    let (events, _loop) = world.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(91).await;
    let logs = world.logs().await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].outcome, ExtractionOutcome::Failed);
    assert!(logs[0].error.is_some());
    assert!(
        logs[0].prompt.contains("NEW MESSAGES"),
        "the prompt is kept for tuning"
    );
    assert!(world.live_proposals().await.is_empty());
    assert!(!world.processed("101").await, "left for a later read");
}

#[tokio::test(start_paused = true)]
async fn an_unworkable_change_is_refused_up_front_and_logged_with_its_reason() {
    let fix = reply(&format!(
        r#"{{"amendments": [{{"kind": "fix", "bosses": ["HMaleficStar", "HFA"],
            "day_ref": "wed", "participants": ["{MY}"], "confidence": 0.9,
            "evidence_message_ids": ["101"]}}], "summary": "weekly wed"}}"#
    ));
    let world = World::new(vec![fix]).await;
    let (events, _loop) = world.pipeline();
    events
        .send(post("hstar every wed from now on"))
        .await
        .expect("send");
    after(91).await;
    let logs = world.logs().await;
    assert_eq!(logs[0].outcome, ExtractionOutcome::NoChange);
    assert_eq!(logs[0].error, None, "error is for failures only");
    assert_eq!(
        logs[0].refusals,
        [ExtractionRefusal {
            change: "fix".into(),
            code: "no_recurring_slot".into(),
            message: "no recurring day and time were agreed - use `/fixed add`".into(),
        }]
    );
    assert!(logs[0].proposal_ids.is_empty());
    assert!(world.live_proposals().await.is_empty());
    assert!(world.outbox.cards.lock().unwrap().is_empty(), "no card");
    assert!(world.processed("101").await);
}

#[tokio::test(start_paused = true)]
async fn a_chat_answer_goes_to_the_reaction_path_not_a_proposal() {
    let rsvp = reply(&format!(
        r#"{{"amendments": [{{"kind": "rsvp", "bosses": ["HMaleficStar", "HFA"],
            "participants": ["{PRIYA}"], "rsvp": "yes", "confidence": 0.9,
            "evidence_message_ids": ["101"]}}]}}"#
    ));
    let world = World::new(vec![rsvp]).await;
    let (events, _loop) = world.pipeline();
    let text = message("101", PRIYA, local(8, 30, 13, 1), "can, mon hstar ok");
    events.send(MessageEvent::Posted(text)).await.expect("send");
    after(91).await;
    let answers = world.outbox.answers.lock().unwrap().clone();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].run_id, world.runs[0]);
    assert_eq!(answers[0].user_ids, [PRIYA]);
    assert_eq!(answers[0].state, RsvpState::Yes);
    assert!(world.live_proposals().await.is_empty());
    assert_eq!(world.logs().await[0].outcome, ExtractionOutcome::NoChange);
}

#[tokio::test(start_paused = true)]
async fn nothing_found_is_no_change_and_the_messages_are_consumed() {
    let world = World::new(vec![nothing()]).await;
    let (events, _loop) = world.pipeline();
    events.send(post("cch7 hstar map")).await.expect("send");
    after(91).await;
    assert_eq!(world.logs().await[0].outcome, ExtractionOutcome::NoChange);
    assert!(world.outbox.cards.lock().unwrap().is_empty());
    assert!(world.processed("101").await);
}

fn added(time: &str, evidence: &str) -> FakeAction {
    reply(&format!(
        r#"{{"amendments": [{{"kind": "add", "bosses": ["NMaleficStar", "NCarling"],
            "day_ref": "tonight", "time_ref": "{time}", "participants": ["{MY}"],
            "confidence": 0.9, "evidence_message_ids": ["{evidence}"]}}]}}"#
    ))
}

#[tokio::test(start_paused = true)]
async fn a_second_proposal_for_a_new_run_is_keyed_on_its_bosses() {
    let world = World::new(vec![added("9pm", "101"), added("9:45pm", "102")]).await;
    let (events, _loop) = world.pipeline();
    events
        .send(post("we do nstar and ncarl tonight 9pm?"))
        .await
        .expect("send");
    after(91).await;
    let first = world.live_proposals().await[0].draft.id.clone();
    let later = message(
        "102",
        MY,
        local(8, 30, 13, 3),
        "nstar ncarl amend to 9:45pm",
    );
    events
        .send(MessageEvent::Posted(later))
        .await
        .expect("send");
    after(91).await;
    let live = world.live_proposals().await;
    assert_eq!(live.len(), 1);
    assert_ne!(live[0].draft.id, first);
}

#[tokio::test(start_paused = true)]
async fn a_proposal_about_a_different_run_is_left_alone() {
    let other = reply(&format!(
        r#"{{"amendments": [{{"kind": "move", "bosses": ["HCarling", "XKalos"],
            "day_ref": "wed", "time_ref": "11pm", "participants": ["{MY}"],
            "confidence": 0.9, "evidence_message_ids": ["101"]}}]}}"#
    ));
    let world = World::new(vec![other, moved("9:30pm", "102")]).await;
    let (events, _loop) = world.pipeline();
    events
        .send(post("carling tue run to wed 11pm?"))
        .await
        .expect("send");
    after(91).await;
    let later = message("102", MY, local(8, 30, 13, 3), MOVE_TEXT);
    events
        .send(MessageEvent::Posted(later))
        .await
        .expect("send");
    after(91).await;
    assert_eq!(world.live_proposals().await.len(), 2);
    let cards = world.outbox.cards.lock().unwrap().clone();
    assert!(cards.iter().all(|card| card.superseded.is_empty()));
}

#[tokio::test(start_paused = true)]
async fn a_burst_too_big_for_one_prompt_is_read_in_pieces() {
    let world = World::with(
        vec![nothing(), nothing(), nothing(), nothing()],
        |config| config.context_tokens = 2_048,
        Arc::new(kanade::infrastructure::llm::identity::Passthrough),
    )
    .await;
    let (events, _loop) = world.pipeline();
    for n in 0..4 {
        let text = format!("hstar wed 9pm? {}", "long planning chatter ".repeat(150));
        let at = local(8, 30, 13, n);
        let long = message(&format!("10{n}"), MY, at, &text);
        events.send(MessageEvent::Posted(long)).await.expect("send");
    }
    after(91).await;
    let logs = world.logs().await;
    assert!(logs.len() > 1, "split to fit the context window");
    assert_eq!(logs.len(), world.requests(), "one row per model call");
    let read: usize = logs.iter().map(|log| log.message_ids.len()).sum();
    assert_eq!(read, 4);
}

#[tokio::test(start_paused = true)]
async fn a_second_burst_about_the_same_run_retires_the_first_proposal() {
    let world = World::new(vec![moved("9:30pm", "101"), moved("10pm", "102")]).await;
    let (events, _loop) = world.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(91).await;
    let first = world.live_proposals().await[0].draft.id.clone();
    let later = message(
        "102",
        MY,
        local(8, 30, 13, 3),
        "hstar actually make it 10pm",
    );
    events
        .send(MessageEvent::Posted(later))
        .await
        .expect("send");
    after(91).await;

    let live = world.live_proposals().await;
    assert_eq!(live.len(), 1);
    assert_ne!(live[0].draft.id, first);
    let all = world.store.list_proposals(false).await.expect("proposals");
    let retired = all.iter().find(|p| p.draft.id == first).expect("first");
    assert_eq!(retired.draft.status, DraftStatus::Discarded);
    assert_eq!(retired.draft.close_reason.as_deref(), Some("superseded"));
    let cards = world.outbox.cards.lock().unwrap().clone();
    assert_eq!(cards[1].superseded, [first], "its card says so");
}

#[tokio::test(start_paused = true)]
async fn two_changes_for_one_run_in_one_burst_both_stay_live() {
    let both = reply(&format!(
        r#"{{"amendments": [
            {{"kind": "move", "bosses": ["HMaleficStar", "HFA"], "day_ref": "wed",
              "time_ref": "9:30pm", "participants": ["{MY}"], "confidence": 0.9,
              "evidence_message_ids": ["101"]}},
            {{"kind": "sub", "bosses": ["HMaleficStar", "HFA"], "participants": ["{PRIYA}"],
              "confidence": 0.9, "evidence_message_ids": ["101"]}}]}}"#
    ));
    let world = World::new(vec![both]).await;
    let (events, _loop) = world.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(91).await;
    let live = world.live_proposals().await;
    assert_eq!(live.len(), 2, "the sub must not retire the move");
    let cards = world.outbox.cards.lock().unwrap().clone();
    assert_eq!(cards.len(), 1, "one card per burst");
    assert_eq!(cards[0].entries.len(), 2);
    assert_eq!(world.logs().await[0].proposal_ids.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn pseudonymized_prompts_leak_nobody_and_answers_decode_back() {
    let tagged = reply(
        r#"{"amendments": [{"kind": "move", "bosses": ["HMaleficStar", "HFA"],
            "day_ref": "wed", "time_ref": "9:30pm", "participants": ["ID1"],
            "confidence": 0.9, "evidence_message_ids": ["101"]}]}"#,
    );
    let world = World::with(vec![tagged], |_| {}, Arc::new(TaggingCodec)).await;
    let (events, _loop) = world.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(91).await;
    let sent = &world.provider.requests()[0];
    let leaks = find_request_leaks(sent, &world.guild.members);
    assert!(leaks.is_empty(), "{leaks:?}");
    let live = world.live_proposals().await;
    assert!(live[0].draft.subject.as_deref().unwrap().contains(MY));
}

#[tokio::test(start_paused = true)]
async fn a_turned_away_burst_is_logged_and_read_again_from_the_backlog() {
    let world = World::new(vec![
        FakeAction::AdmissionRefused(None),
        FakeAction::AdmissionRefused(None),
        moved("9:30pm", "101"),
    ])
    .await;
    let (events, _loop) = world.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(95).await;
    let logs = world.logs().await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].outcome, ExtractionOutcome::TurnedAway);
    assert!(!world.processed("101").await);
    after(30).await;
    let logs = world.logs().await;
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[1].outcome, ExtractionOutcome::Proposed);
    assert!(world.processed("101").await);
}

#[tokio::test(start_paused = true)]
async fn an_open_breaker_is_waited_out_not_spun_on() {
    let world = World::new(vec![FakeAction::BackendUnavailable, moved("9:30pm", "101")]).await;
    let (events, _loop) = world.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(91).await;
    // The breaker opened on the failure; the backlog tries once after its
    // interval, is refused without a request, then waits for the probe time.
    after(30).await;
    let outcomes: Vec<_> = world.logs().await.iter().map(|log| log.outcome).collect();
    assert_eq!(
        outcomes,
        [ExtractionOutcome::TurnedAway, ExtractionOutcome::TurnedAway]
    );
    assert_eq!(world.requests(), 1);
    after(120).await;
    let logs = world.logs().await;
    assert_eq!(logs.len(), 3, "no spinning while the breaker was open");
    assert_eq!(logs[1].request_count, 0);
    assert_eq!(logs[2].outcome, ExtractionOutcome::Proposed);
    assert_eq!(world.requests(), 2);
}

#[tokio::test(start_paused = true)]
async fn replayed_history_drains_at_a_fixed_rate_without_duplicates() {
    let world = World::new((0..5).map(|_| nothing()).collect()).await;
    let (events, _loop) = world.pipeline();
    let history: Vec<_> = (0..30)
        .map(|n| {
            let at = local(8, 29, 20, 0) + chrono::TimeDelta::minutes(n);
            replayed(&format!("r{n:02}"), MY, at, "hstar wed 9pm?")
        })
        .collect();
    for message in history.iter().chain(&history[..5]) {
        events
            .try_send(MessageEvent::Posted(message.clone()))
            .expect("room");
    }
    after(1).await;
    assert_eq!(world.requests(), 1, "12 messages per burst, one burst now");
    after(15).await;
    assert_eq!(world.requests(), 2);
    after(15).await;
    assert_eq!(world.requests(), 3);
    after(100).await;
    assert_eq!(world.requests(), 3, "30 messages, deduplicated, 3 bursts");
    let read: usize = world
        .logs()
        .await
        .iter()
        .map(|log| log.message_ids.len())
        .sum();
    assert_eq!(read, 30);
}

#[tokio::test(start_paused = true)]
async fn a_full_backlog_drops_its_oldest_messages_and_says_so() {
    let world = World::with(
        vec![nothing(); 3],
        |config| config.backlog_capacity = 3,
        Arc::new(kanade::infrastructure::llm::identity::Passthrough),
    )
    .await;
    let (events, _loop) = world.pipeline();
    for n in 0..5 {
        let at = local(8, 29, 20, n);
        let message = replayed(&format!("r{n}"), MY, at, "hstar wed 9pm?");
        events
            .try_send(MessageEvent::Posted(message))
            .expect("room");
    }
    after(1).await;
    let drops = world.outbox.drops.lock().unwrap().clone();
    let dropped: Vec<String> = drops.iter().flat_map(|d| d.message_ids.clone()).collect();
    assert_eq!(dropped, ["r0", "r1"]);
    assert!(drops.iter().all(|d| d.capacity == 3));
}

#[tokio::test(start_paused = true)]
async fn an_edit_during_an_in_flight_read_is_read_again() {
    let slow = FakeAction::Delayed {
        delay: Duration::from_secs(10),
        action: Box::new(moved("9pm", "101")),
    };
    let world = World::new(vec![slow, moved("10pm", "101")]).await;
    let (events, _loop) = world.pipeline();
    events
        .send(post("@here hstar wed 9pm"))
        .await
        .expect("send");
    after(5).await;
    let edit = message("101", MY, local(8, 30, 13, 1), "@here hstar wed 10pm");
    events.send(MessageEvent::Edited(edit)).await.expect("send");
    after(10).await;
    assert_eq!(world.requests(), 1);
    assert!(
        !world.processed("101").await,
        "the first read saw 9pm; the edit must be read again"
    );
    let first = world.live_proposals().await[0].draft.id.clone();
    after(90).await;
    assert_eq!(world.requests(), 2);
    let logs = world.logs().await;
    assert!(logs[1].prompt.contains("@here hstar wed 10pm"));
    let live = world.live_proposals().await;
    assert_eq!(live.len(), 1);
    assert_ne!(live[0].draft.id, first, "the correction replaced 9pm");
    let cards = world.outbox.cards.lock().unwrap().clone();
    assert_eq!(cards[1].entries[0].time_ref.as_deref(), Some("10pm"));
    assert_eq!(cards[1].superseded, [first]);
    assert!(world.processed("101").await);
}

#[tokio::test(start_paused = true)]
async fn a_misconfigured_route_fails_without_a_backlog_retry() {
    let world = World::ungrouped(vec![moved("9:30pm", "101")]).await;
    let (events, _loop) = world.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(91).await;
    after(120).await;
    let logs = world.logs().await;
    assert_eq!(logs.len(), 1, "never requeued");
    assert_eq!(logs[0].outcome, ExtractionOutcome::Failed);
    assert_eq!(world.requests(), 0);
    assert!(!world.processed("101").await);
}

#[tokio::test(start_paused = true)]
async fn a_content_filter_is_logged_as_content_blocked_without_a_retry() {
    let world = World::new(vec![filtered()]).await;
    let (events, _loop) = world.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(91).await;
    after(120).await;
    let logs = world.logs().await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].outcome, ExtractionOutcome::ContentBlocked);
    assert_eq!(world.requests(), 1, "no answer retry or requeue");
}

#[tokio::test(start_paused = true)]
async fn a_backlogged_message_edited_live_is_read_once_by_its_burst() {
    let world = World::new(vec![nothing(), nothing(), nothing()]).await;
    let (events, _loop) = world.pipeline();
    let first = replayed("r0", MY, local(8, 29, 20, 0), "hstar wed 9pm?");
    let mut other = replayed("r1", MY, local(8, 29, 20, 5), "hstar wed 9pm?");
    other.channel_id = OTHER.into();
    for message in [first, other] {
        events
            .try_send(MessageEvent::Posted(message))
            .expect("room");
    }
    after(1).await;
    assert_eq!(world.requests(), 1, "r1 waits for the next drain");
    let mut edit = message("r1", MY, local(8, 29, 20, 5), "hstar wed 10pm?");
    edit.channel_id = OTHER.into();
    events.send(MessageEvent::Edited(edit)).await.expect("send");
    after(20).await;
    assert_eq!(world.requests(), 1, "the live burst owns r1 now");
    after(80).await;
    assert_eq!(world.requests(), 2);
    let reads = world
        .logs()
        .await
        .iter()
        .filter(|log| log.message_ids.contains(&"r1".to_owned()))
        .count();
    assert_eq!(reads, 1);
}

#[test]
fn outcomes_are_derived_from_the_call_then_what_it_created() {
    use kanade::extract::pipeline::Failure;
    let away = Some(Failure::TurnedAway { retry_at: None });
    assert_eq!(
        extraction_outcome(away, 0, 0),
        ExtractionOutcome::TurnedAway
    );
    assert_eq!(
        extraction_outcome(Some(Failure::ContentBlocked), 0, 0),
        ExtractionOutcome::ContentBlocked
    );
    assert_eq!(extraction_outcome(None, 2, 0), ExtractionOutcome::Proposed);
}

#[test]
fn an_unpublished_extraction_effort_is_caught_at_startup() {
    let capabilities = ModelCapabilities {
        reasoning_control: true,
        reasoning_efforts: Some(vec![Effort::Low]),
        ..ModelCapabilities::minimal()
    };
    let refused = check_reasoning_effort("kanata/x", Some(Effort::High), &capabilities);
    assert_eq!(
        refused.map_err(|error| error.to_string()),
        Err("extraction reasoning high is not published by kanata/x".into())
    );
    assert!(check_reasoning_effort("kanata/x", Some(Effort::Low), &capabilities).is_ok());
    assert!(check_reasoning_effort("kanata/x", Some(Effort::Off), &capabilities).is_ok());
    assert!(check_reasoning_effort("kanata/x", None, &capabilities).is_ok());
    let decides = ModelCapabilities {
        reasoning_control: true,
        ..ModelCapabilities::minimal()
    };
    assert!(check_reasoning_effort("kanata/x", Some(Effort::High), &decides).is_ok());
}

#[tokio::test(start_paused = true)]
async fn an_external_route_is_refused_by_default_and_marked_when_allowed() {
    use kanade::infrastructure::llm::governor::Role;
    let refused = World::new(vec![moved("9:30pm", "101")]).await;
    refused
        .client
        .governor()
        .set_external(Role::Extraction, true);
    let (events, _loop) = refused.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(91).await;
    let logs = refused.logs().await;
    assert_eq!(logs[0].outcome, ExtractionOutcome::Failed);
    assert_eq!(logs[0].guardrail, serde_json::json!({}));
    assert_eq!(refused.requests(), 0);

    let world = World::new(vec![moved("9:30pm", "101")]).await;
    let governor = world.client.governor();
    governor.set_external(Role::Extraction, true);
    governor.allow_external_unmasked(true);
    let (events, _loop) = world.pipeline();
    events.send(post(MOVE_TEXT)).await.expect("send");
    after(91).await;
    let logs = world.logs().await;
    assert_eq!(logs[0].outcome, ExtractionOutcome::Proposed);
    assert_eq!(
        logs[0].guardrail,
        serde_json::json!({"external_unmasked": true})
    );
    assert_eq!(world.requests(), 1);
}
