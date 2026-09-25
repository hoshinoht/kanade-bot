//! Rescan jobs over `rescan_jobs`: one at a time, channels in order, backfill
//! before reading, v4 widening, cancellation between bursts, and no duplicate
//! proposals when handled messages are read again.

use std::sync::Arc;

use kanade::domain::model_log::{ExtractionOutcome, ModelLogStore, RescanStatus};
use kanade::extract::rescan::{RescanError, RescanRequest, Rescans};
use kanade::infrastructure::llm::FakeAction;
use serde_json::Value;

use crate::fakes::{
    CHANNEL, Core, FakeHistory, MY, OTHER, STRANGER, World, after, local, message, nothing, reply,
};

type Jobs = Rescans<
    kanade::infrastructure::store::MemoryScheduleStore,
    kanade::infrastructure::llm::FakeProvider,
    crate::fakes::Scheduler,
    crate::fakes::Recorder,
    FakeHistory,
>;

fn moved(evidence: &str) -> FakeAction {
    reply(&format!(
        r#"{{"amendments": [{{"kind": "move", "bosses": ["HMaleficStar", "HFA"],
            "day_ref": "wed", "time_ref": "9:30pm", "participants": ["{MY}"],
            "confidence": 0.9, "evidence_message_ids": ["{evidence}"]}}]}}"#
    ))
}

fn request(channels: &[&str], window: &str) -> RescanRequest {
    RescanRequest {
        channels: channels.iter().map(|c| (*c).to_owned()).collect(),
        window: window.into(),
        source: "portal".into(),
        automated: false,
        requested_by: Some(MY.into()),
    }
}

fn jobs(extractor: &Arc<Core>, history: FakeHistory) -> Arc<Jobs> {
    Arc::new(Rescans::new(extractor.clone(), Arc::new(history)))
}

fn start(jobs: &Arc<Jobs>) -> tokio::task::JoinHandle<()> {
    let worker = jobs.clone();
    tokio::spawn(async move { worker.run().await })
}

fn history(
    channel: &str,
    messages: Vec<kanade::extract::pipeline::IncomingMessage>,
) -> FakeHistory {
    let history = FakeHistory::default();
    let mut messages = messages;
    for message in &mut messages {
        message.channel_id = channel.to_owned();
    }
    history
        .messages
        .lock()
        .unwrap()
        .insert(channel.to_owned(), messages);
    history
}

async fn status(world: &World, id: &str) -> RescanStatus {
    world
        .store
        .load_rescan_job(id)
        .await
        .expect("load")
        .expect("job")
        .status
}

#[tokio::test(start_paused = true)]
async fn a_job_backfills_then_reads_each_channel_in_turn() {
    let world = World::new(vec![moved("101"), nothing()]).await;
    let source = history(
        CHANNEL,
        vec![message(
            "101",
            MY,
            local(8, 30, 13, 1),
            "mon cannot, change to wed 9:30pm?",
        )],
    );
    source.messages.lock().unwrap().insert(
        OTHER.into(),
        vec![kanade::extract::pipeline::IncomingMessage {
            channel_id: OTHER.into(),
            ..message("201", MY, local(8, 30, 13, 2), "hstar wed?")
        }],
    );
    let jobs = jobs(&world.extractor, source);
    let job = jobs
        .submit(request(&[CHANNEL, OTHER], "week"))
        .await
        .expect("queued");
    let id = job.job.id.clone();
    assert_eq!(job.job.status, RescanStatus::Queued);
    assert_eq!(status(&world, &id).await, RescanStatus::Queued);

    let _worker = start(&jobs);
    after(1).await;
    assert_eq!(status(&world, &id).await, RescanStatus::Running);
    let view = jobs.get(&id).await.expect("get").expect("job");
    assert_eq!(
        view.current.as_deref(),
        Some(OTHER),
        "first channel done, pacing"
    );
    assert_eq!(world.requests(), 1);
    after(20).await;
    assert_eq!(
        world.requests(),
        2,
        "the second channel's call waited its turn"
    );

    let stored = world
        .store
        .load_rescan_job(&id)
        .await
        .expect("load")
        .expect("job");
    assert_eq!(stored.status, RescanStatus::Done);
    assert!(stored.started_at.is_some() && stored.finished_at.is_some());
    let results = stored.results.as_array().expect("results");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["channel_id"], CHANNEL);
    assert_eq!(results[0]["backfilled"], 1);
    assert_eq!(results[0]["proposals"], 1);
    assert_eq!(results[1]["proposals"], 0);
    assert_eq!(world.live_proposals().await.len(), 1);
    assert_eq!(world.logs().await.len(), 2);
    assert_eq!(world.outbox.cards.lock().unwrap().len(), 1);
    assert_eq!(
        jobs.get(&id).await.expect("get").expect("job").current,
        None
    );
}

#[tokio::test(start_paused = true)]
async fn a_repeat_request_attaches_and_a_new_window_replaces_the_queued_job() {
    let world = World::new(Vec::new()).await;
    let jobs = jobs(&world.extractor, FakeHistory::default());
    let first = jobs
        .submit(request(&[CHANNEL, OTHER], "week"))
        .await
        .expect("queued");
    let again = jobs
        .submit(request(&[CHANNEL], "week"))
        .await
        .expect("attached");
    assert_eq!(again.job.id, first.job.id, "a narrower request attaches");
    let wider = jobs
        .submit(request(&[CHANNEL], "two_weeks"))
        .await
        .expect("queued");
    assert_ne!(wider.job.id, first.job.id);
    let replaced = world
        .store
        .load_rescan_job(&first.job.id)
        .await
        .expect("load")
        .expect("job");
    assert_eq!(replaced.status, RescanStatus::Cancelled);
    assert_eq!(
        replaced.error.as_deref(),
        Some("replaced by a newer request")
    );
    assert_eq!(jobs.queued(), 1);

    assert!(jobs.cancel(&wider.job.id).await.expect("cancel"));
    assert_eq!(status(&world, &wider.job.id).await, RescanStatus::Cancelled);
    assert!(!jobs.cancel(&wider.job.id).await.expect("cancel"));
    assert!(!jobs.cancel("unknown").await.expect("cancel"));
    assert_eq!(
        jobs.submit(request(&[], "week")).await,
        Err(RescanError::NoChannels)
    );
    assert!(matches!(
        jobs.submit(request(&[CHANNEL], "month")).await,
        Err(RescanError::Window(_))
    ));
}

#[tokio::test(start_paused = true)]
async fn cancelling_a_running_job_stops_before_the_next_burst() {
    let world = World::new(vec![nothing(), nothing()]).await;
    let source = history(
        CHANNEL,
        vec![
            message("1", MY, local(8, 28, 20, 0), "hstar wed 9pm?"),
            message("2", MY, local(8, 29, 20, 0), "carling tue 10pm?"),
        ],
    );
    let jobs = jobs(&world.extractor, source);
    let id = jobs
        .submit(request(&[CHANNEL], "week"))
        .await
        .expect("queued")
        .job
        .id;
    let _worker = start(&jobs);
    after(1).await;
    assert_eq!(
        world.requests(),
        1,
        "one conversation per day; the second waits"
    );
    assert!(jobs.cancel(&id).await.expect("cancel"));
    after(30).await;
    assert_eq!(world.requests(), 1);
    let stored = world
        .store
        .load_rescan_job(&id)
        .await
        .expect("load")
        .expect("job");
    assert_eq!(stored.status, RescanStatus::Cancelled);
    assert_eq!(stored.results[0]["cancelled"], Value::Bool(true));
    assert_eq!(stored.results[0]["bursts"], 2);
    assert_eq!(world.logs().await.len(), 1, "what was read is still logged");
}

#[tokio::test(start_paused = true)]
async fn reading_handled_messages_again_does_not_duplicate_proposals() {
    let world = World::new(vec![moved("101"), moved("101"), moved("101")]).await;
    let source = history(
        CHANNEL,
        vec![message(
            "101",
            MY,
            local(8, 30, 13, 1),
            "mon cannot, change to wed 9:30pm?",
        )],
    );
    let jobs = jobs(&world.extractor, source);
    let _worker = start(&jobs);

    jobs.submit(request(&[CHANNEL], "week"))
        .await
        .expect("queued");
    after(1).await;
    let first = world.live_proposals().await;
    assert_eq!(first.len(), 1);

    jobs.submit(request(&[CHANNEL], "since_reset"))
        .await
        .expect("queued");
    after(1).await;
    let second = world.live_proposals().await;
    assert_eq!(second.len(), 1, "the re-read replaces, never duplicates");
    assert_ne!(second[0].draft.id, first[0].draft.id);

    world
        .scheduler
        .approve(&second[0].draft.id)
        .await
        .expect("approved");
    let view = jobs
        .submit(request(&[CHANNEL], "week"))
        .await
        .expect("queued");
    after(20).await;
    assert_eq!(
        world.requests(),
        3,
        "a rescan re-reads processed messages (v4)"
    );
    assert!(world.live_proposals().await.is_empty(), "already scheduled");
    let stored = world
        .store
        .load_rescan_job(&view.job.id)
        .await
        .expect("load")
        .expect("job");
    assert_eq!(stored.results[0]["proposals"], 0);
    assert_eq!(stored.results[0]["dropped"], 1);
    let last = world.logs().await.pop().expect("log");
    assert_eq!(last.outcome, ExtractionOutcome::NoChange);
}

#[tokio::test(start_paused = true)]
async fn an_empty_week_widens_once_and_since_reset_does_not() {
    let world = World::new(vec![nothing()]).await;
    let source = history(
        CHANNEL,
        vec![message("1", MY, local(8, 25, 20, 0), "hstar wed 9pm?")],
    );
    let jobs = jobs(&world.extractor, source);
    let _worker = start(&jobs);

    let narrow = jobs
        .submit(request(&[CHANNEL], "since_reset"))
        .await
        .expect("queued");
    after(5).await;
    let stored = world
        .store
        .load_rescan_job(&narrow.job.id)
        .await
        .expect("load")
        .expect("job");
    assert_eq!(stored.results[0]["widened"], false);
    assert_eq!(stored.results[0]["gated"], 0);
    assert_eq!(world.requests(), 0);

    let week = jobs
        .submit(request(&[CHANNEL], "week"))
        .await
        .expect("queued");
    after(5).await;
    let stored = world
        .store
        .load_rescan_job(&week.job.id)
        .await
        .expect("load")
        .expect("job");
    assert_eq!(stored.results[0]["widened"], true);
    assert_eq!(stored.results[0]["gated"], 1);
    assert_eq!(world.requests(), 1);
}

#[tokio::test(start_paused = true)]
async fn outsiders_are_not_read_and_a_model_failure_is_reported_not_raised() {
    let world = World::new(vec![FakeAction::Permanent]).await;
    let source = history(
        CHANNEL,
        vec![
            message("1", STRANGER, local(8, 30, 12, 0), "hstar wed 9pm?"),
            message("2", MY, local(8, 30, 13, 0), "carling tue 10pm?"),
        ],
    );
    let jobs = jobs(&world.extractor, source);
    let _worker = start(&jobs);
    let id = jobs
        .submit(request(&[CHANNEL], "week"))
        .await
        .expect("queued")
        .job
        .id;
    after(5).await;
    let stored = world
        .store
        .load_rescan_job(&id)
        .await
        .expect("load")
        .expect("job");
    assert_eq!(stored.status, RescanStatus::Done);
    assert_eq!(stored.results[0]["stored"], 1, "only role holders count");
    assert_eq!(stored.results[0]["gated"], 1);
    assert!(
        !stored.results[0]["errors"]
            .as_array()
            .expect("errors")
            .is_empty()
    );
    let logs = world.logs().await;
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].outcome, ExtractionOutcome::Failed);
    assert_eq!(logs[0].message_ids, ["2"]);
}

#[tokio::test(start_paused = true)]
async fn closing_cancels_queued_jobs_and_refuses_new_ones() {
    let world = World::new(Vec::new()).await;
    let jobs = jobs(&world.extractor, FakeHistory::default());
    let first = jobs
        .submit(request(&[CHANNEL], "week"))
        .await
        .expect("queued");
    let second = jobs
        .submit(request(&[OTHER], "week"))
        .await
        .expect("queued");
    jobs.close().await;
    for id in [&first.job.id, &second.job.id] {
        assert_eq!(status(&world, id).await, RescanStatus::Cancelled);
    }
    jobs.run().await;
    assert_eq!(
        jobs.submit(request(&[CHANNEL], "week")).await,
        Err(RescanError::Closed)
    );
}
