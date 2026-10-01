//! Token usage on the extraction log row (`docs/v5/extraction-orchestration.md`
//! "Token usage"): the reported pair sums the attempts whose reply carried
//! usage and the estimate covers those same attempts; without any reported
//! usage the estimate covers every sent attempt; nothing sent, nothing logged.

use std::time::Duration;

use kanade::extract::pipeline::MessageEvent;
use kanade::extract::prompt::estimate_messages;
use kanade::infrastructure::llm::{CompletionResponse, FakeAction, FinishReason, Usage};

use crate::fakes::{ALIAS, MY, World, after, local, message};

const NOTHING: &str = r#"{"amendments": [], "summary": "no schedule change"}"#;

fn answer(content: &str, usage: Option<(u32, u32)>) -> FakeAction {
    FakeAction::Response(CompletionResponse {
        model: ALIAS.into(),
        content: Some(content.to_owned()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: usage.map(|(prompt_tokens, completion_tokens)| Usage {
            prompt_tokens,
            completion_tokens,
        }),
    })
}

fn slow(action: FakeAction) -> FakeAction {
    FakeAction::Delayed {
        delay: Duration::from_secs(10),
        action: Box::new(action),
    }
}

/// The estimates of the requests the provider received, in order.
fn estimates(world: &World) -> Vec<u64> {
    world
        .provider
        .requests()
        .iter()
        .map(|request| u64::try_from(estimate_messages(&request.messages)).expect("fits"))
        .collect()
}

async fn one_call(world: &World) -> (Option<u64>, Option<u64>, Option<u64>) {
    let (events, _loop) = world.pipeline();
    let post = message("101", MY, local(8, 30, 13, 1), "hstar wed 9pm?");
    events.send(MessageEvent::Posted(post)).await.expect("send");
    after(91).await;
    usage_of(world).await
}

async fn usage_of(world: &World) -> (Option<u64>, Option<u64>, Option<u64>) {
    let logs = world.logs().await;
    assert_eq!(logs.len(), 1, "one call, one row");
    (
        logs[0].prompt_tokens,
        logs[0].completion_tokens,
        logs[0].prompt_estimate,
    )
}

#[tokio::test(start_paused = true)]
async fn one_reported_reply_logs_its_pair_and_estimate() {
    let world = World::new(vec![answer(NOTHING, Some((1200, 80)))]).await;
    let usage = one_call(&world).await;
    let sent = estimates(&world);
    assert_eq!(sent.len(), 1);
    assert!(sent[0] > 0);
    assert_eq!(usage, (Some(1200), Some(80), Some(sent[0])));
}

#[tokio::test(start_paused = true)]
async fn an_answer_retry_sums_both_reported_attempts() {
    let world = World::new(vec![
        answer("not json", Some((1000, 5))),
        answer(NOTHING, Some((1100, 40))),
    ])
    .await;
    let usage = one_call(&world).await;
    let sent = estimates(&world);
    assert_eq!(sent.len(), 2);
    assert_eq!(usage, (Some(2100), Some(45), Some(sent[0] + sent[1])));
}

#[tokio::test(start_paused = true)]
async fn the_estimate_covers_only_the_attempts_that_reported() {
    let world = World::new(vec![
        answer("not json", Some((1000, 5))),
        answer(NOTHING, None),
    ])
    .await;
    let usage = one_call(&world).await;
    let sent = estimates(&world);
    assert_eq!(sent.len(), 2);
    assert_eq!(
        usage,
        (Some(1000), Some(5), Some(sent[0])),
        "the unreported retry adds to neither the pair nor the estimate"
    );
}

#[tokio::test(start_paused = true)]
async fn without_reported_usage_the_estimate_covers_every_sent_attempt() {
    let world = World::new(vec![answer("not json", None), answer(NOTHING, None)]).await;
    let usage = one_call(&world).await;
    let sent = estimates(&world);
    assert_eq!(sent.len(), 2);
    assert_eq!(usage, (None, None, Some(sent[0] + sent[1])));
}

#[tokio::test(start_paused = true)]
async fn a_failed_sent_attempt_keeps_only_the_estimate() {
    let world = World::new(vec![FakeAction::Permanent]).await;
    let usage = one_call(&world).await;
    let sent = estimates(&world);
    assert_eq!(sent.len(), 1);
    assert_eq!(usage, (None, None, Some(sent[0])));
}

#[tokio::test(start_paused = true)]
async fn a_call_turned_away_before_sending_logs_no_usage() {
    use kanade::infrastructure::llm::governor::Role;

    let world = World::ungrouped(vec![answer(NOTHING, Some((1, 1)))]).await;
    assert!(world.client.governor().set_external(Role::Extraction, true));
    let usage = one_call(&world).await;
    assert_eq!(world.requests(), 0);
    assert_eq!(usage, (None, None, None));
}

#[tokio::test(start_paused = true)]
async fn a_call_cut_in_flight_keeps_the_estimate_of_what_was_sent() {
    let world = World::new(vec![slow(answer(NOTHING, Some((1, 1))))]).await;
    let (events, _loop) = world.pipeline();
    let post = message("101", MY, local(8, 30, 13, 1), "hstar wed 9pm?");
    events.send(MessageEvent::Posted(post)).await.expect("send");
    after(91).await;
    assert_eq!(world.requests(), 1, "the reply is still on its way");
    world.extractor.interrupt_calls();
    after(1).await;
    let sent = estimates(&world);
    assert_eq!(usage_of(&world).await, (None, None, Some(sent[0])));
}

#[tokio::test(start_paused = true)]
async fn a_retry_cut_in_flight_keeps_the_reported_first_attempt() {
    let world = World::new(vec![
        answer("not json", Some((1000, 5))),
        slow(answer(NOTHING, Some((1, 1)))),
    ])
    .await;
    let (events, _loop) = world.pipeline();
    let post = message("101", MY, local(8, 30, 13, 1), "hstar wed 9pm?");
    events.send(MessageEvent::Posted(post)).await.expect("send");
    after(91).await;
    assert_eq!(world.requests(), 2, "the retry is on its way");
    world.extractor.interrupt_calls();
    after(1).await;
    let sent = estimates(&world);
    assert_eq!(usage_of(&world).await, (Some(1000), Some(5), Some(sent[0])));
}
