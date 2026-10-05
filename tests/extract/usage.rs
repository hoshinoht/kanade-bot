//! Token usage on the extraction log row (`docs/notes/extraction-orchestration.md`
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
        reasoning_content: None,
        reasoning_tokens: None,
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
async fn logged_request_ids_are_exactly_the_ids_sent_across_retries() {
    use kanade::infrastructure::llm::ModelCapabilities;
    // A transient failure retried, a bad answer, then the answer retry.
    let world = World::new(vec![
        FakeAction::Transient,
        answer("not json", None),
        answer(NOTHING, None),
    ])
    .await;
    *world.model.caps.lock().unwrap() = Some(ModelCapabilities::minimal());
    one_call(&world).await;
    let rows = world.logs().await;
    let sent = world.model.request_ids.lock().unwrap().clone();
    let session = rows[0]
        .session_id
        .clone()
        .expect("a tagged request went out");
    assert!(session.starts_with("kanade-extraction-"), "{session}");
    assert_eq!(
        sent,
        (1..=3)
            .map(|n| format!("{session}-{n}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(rows[0].request_ids, sent);
    assert_eq!(rows[0].request_count, 3);
}

#[tokio::test(start_paused = true)]
async fn untagged_calls_log_no_correlation() {
    let world = World::new(vec![answer(NOTHING, None)]).await;
    one_call(&world).await;
    let rows = world.logs().await;
    assert!(world.model.request_ids.lock().unwrap().is_empty());
    assert_eq!((&rows[0].session_id, rows[0].request_ids.len()), (&None, 0));
}

#[tokio::test(start_paused = true)]
async fn reasoning_attempts_are_retained_summed_and_not_sent_back() {
    use kanade::infrastructure::llm::{ModelCapabilities, wire_body};
    let with_reasoning = |content: &str, text: &str, tokens: Option<u64>| {
        let FakeAction::Response(mut response) = answer(content, None) else {
            panic!("response")
        };
        response.reasoning_content = Some(text.into());
        response.reasoning_tokens = tokens;
        FakeAction::Response(response)
    };
    let world = World::new(vec![
        with_reasoning("not json", "First reasoning.", Some(7)),
        with_reasoning(NOTHING, "Retry reasoning.", Some(9)),
    ])
    .await;
    one_call(&world).await;
    let rows = world.logs().await;
    assert_eq!(
        rows[0].reasoning_content.as_deref(),
        Some("First reasoning.\n\nRetry reasoning.")
    );
    assert_eq!(rows[0].reasoning_tokens, Some(16));
    assert_eq!(
        rows[0].prompt_tokens, None,
        "reasoning count is independent of the usage pair"
    );
    let requests = world.provider.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        let body = wire_body(request, &ModelCapabilities::minimal()).expect("body");
        assert!(!body.to_string().contains("First reasoning"));
        assert!(!body.to_string().contains("reasoning_content"));
        assert!(!body.to_string().contains("reasoning_tokens"));
    }
}

#[tokio::test(start_paused = true)]
async fn absent_reasoning_stays_null_and_text_only_stays_unknown() {
    let world = World::new(vec![answer(NOTHING, None)]).await;
    one_call(&world).await;
    let rows = world.logs().await;
    assert_eq!(
        (rows[0].reasoning_content.as_ref(), rows[0].reasoning_tokens),
        (None, None)
    );
    let FakeAction::Response(mut response) = answer(NOTHING, None) else {
        panic!("response")
    };
    response.reasoning_content = Some("Text only.".into());
    let world = World::new(vec![FakeAction::Response(response)]).await;
    one_call(&world).await;
    let rows = world.logs().await;
    assert_eq!(rows[0].reasoning_content.as_deref(), Some("Text only."));
    assert_eq!(rows[0].reasoning_tokens, None);
}

#[tokio::test(start_paused = true)]
async fn a_truncated_attempt_is_not_appended_again_and_token_sums_stay_storable() {
    use kanade::domain::model_log::{REASONING_CAP, REASONING_TRUNCATED, capped_reasoning};

    let text = "奏".repeat(REASONING_CAP);
    let FakeAction::Response(mut first) = answer("not json", None) else {
        panic!("response")
    };
    first.reasoning_content = Some(text.clone());
    first.reasoning_tokens = Some(i64::MAX as u64);
    let FakeAction::Response(mut second) = answer(NOTHING, None) else {
        panic!("response")
    };
    second.reasoning_content = Some("Later reasoning must not replace the marker.".into());
    second.reasoning_tokens = Some(1);
    let world = World::new(vec![
        FakeAction::Response(first),
        FakeAction::Response(second),
    ])
    .await;
    one_call(&world).await;
    let rows = world.logs().await;
    assert_eq!(rows[0].reasoning_content, capped_reasoning(&text));
    assert_eq!(rows[0].reasoning_tokens, Some(i64::MAX as u64));
    assert_eq!(
        rows[0]
            .reasoning_content
            .as_deref()
            .expect("text")
            .matches(REASONING_TRUNCATED)
            .count(),
        1
    );
    assert_eq!(world.provider.requests().len(), 2);
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
