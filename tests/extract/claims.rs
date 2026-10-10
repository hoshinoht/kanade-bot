//! One admission and one effect per message version across live bursts,
//! backlog drains and rescans sharing one extractor: a pass that finds a
//! row claimed defers it, the owner re-offers it if it lets go unread, and
//! effects follow an all-or-nothing processed mark. Every race runs over
//! ten fresh fixtures.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use kanade::domain::model_log::{ExtractionOutcome, ModelLogStore};
use kanade::extract::pipeline::{CALL_CANCELLED, MESSAGES_UNWRITABLE, MessageEvent};
use kanade::extract::rescan::{RescanRequest, Rescans};
use kanade::infrastructure::llm::FakeAction;
use serde_json::Value;

use crate::fakes::{
    CHANNEL, Core, FakeHistory, MY, OTHER, World, after, local, message, nothing, reply,
};

type Jobs = Rescans<
    kanade::infrastructure::store::MemoryScheduleStore,
    crate::fakes::Model,
    crate::fakes::Scheduler,
    crate::fakes::Recorder,
    FakeHistory,
>;

const RUNS: usize = 10;
const M: &str = "101";
/// Urgent: a ping with a boss flushes at once.
const URGENT: &str = "@here mon cannot, hstar wed 9:30pm?";
/// Waits for the 90 s debounce.
const QUIET: &str = "mon cannot, change to wed 9:30pm?";

fn moved(evidence: &str) -> FakeAction {
    reply(&format!(
        r#"{{"amendments": [{{"kind": "move", "bosses": ["HMaleficStar", "HFA"],
            "day_ref": "wed", "time_ref": "9:30pm", "participants": ["{MY}"],
            "confidence": 0.9, "evidence_message_ids": ["{evidence}"]}}],
          "summary": "proposed for wed"}}"#
    ))
}

fn added(evidence: &str) -> FakeAction {
    reply(&format!(
        r#"{{"amendments": [{{"kind": "add", "bosses": ["NMaleficStar", "NCarling"],
            "day_ref": "tonight", "time_ref": "9pm", "participants": ["{MY}"],
            "confidence": 0.9, "evidence_message_ids": ["{evidence}"]}}]}}"#
    ))
}

fn slow(seconds: u64, action: FakeAction) -> FakeAction {
    FakeAction::Delayed {
        delay: Duration::from_secs(seconds),
        action: Box::new(action),
    }
}

/// Model calls stay inside the fake client's 30 s deadline, so the live
/// burst comes due while a rescan call is still in flight.
fn quick_debounce(config: &mut kanade::extract::pipeline::PipelineConfig) {
    config.debounce = Duration::from_secs(5);
}

fn post(text: &str) -> MessageEvent {
    MessageEvent::Posted(message(M, MY, local(8, 30, 13, 1), text))
}

/// The startup rescan: unread messages only.
fn startup() -> RescanRequest {
    RescanRequest {
        channels: vec![CHANNEL.into()],
        window: "week".into(),
        source: "startup".into(),
        automated: false,
        requested_by: None,
        unprocessed_only: true,
    }
}

fn jobs(extractor: &Arc<Core>) -> Arc<Jobs> {
    jobs_over(extractor, FakeHistory::default())
}

fn jobs_over(extractor: &Arc<Core>, history: FakeHistory) -> Arc<Jobs> {
    Arc::new(Rescans::new(extractor.clone(), Arc::new(history)))
}

fn start(jobs: &Arc<Jobs>) -> tokio::task::JoinHandle<()> {
    let worker = jobs.clone();
    tokio::spawn(async move { worker.run().await })
}

/// The first channel result of a finished job.
async fn result(world: &World, id: &str) -> Value {
    let job = world
        .store
        .load_rescan_job(id)
        .await
        .expect("load")
        .expect("job");
    job.results[0].clone()
}

async fn reads_of(world: &World, id: &str) -> Vec<ExtractionOutcome> {
    world
        .logs()
        .await
        .iter()
        .filter(|log| log.message_ids.iter().any(|m| m == id))
        .map(|log| log.outcome)
        .collect()
}

/// (1) The live burst reads M first; the startup rescan defers it and M is
/// admitted, proposed and carded once.
#[tokio::test(start_paused = true)]
async fn a_live_read_in_flight_defers_the_startup_rescan() {
    for _ in 0..RUNS {
        let world = World::new(vec![moved(M)]).await;
        // The live call stays in flight until the rescan has run.
        let gate = world.model.close_gate();
        let (events, _loop) = world.pipeline();
        events.send(post(URGENT)).await.expect("send");
        after(1).await;
        assert_eq!(world.requests(), 1);

        let jobs = jobs(&world.extractor);
        let _worker = start(&jobs);
        let id = jobs.submit(startup()).await.expect("queued").job.id;
        after(5).await;
        let rescanned = result(&world, &id).await;
        assert_eq!(rescanned["deferred"], 1);
        assert_eq!(rescanned["calls"], 0);
        assert!(!world.processed(M).await);

        gate.add_permits(1);
        after(120).await;
        assert_eq!(world.requests(), 1, "one admission");
        assert_eq!(world.live_proposals().await.len(), 1);
        assert_eq!(world.outbox.cards.lock().unwrap().len(), 1);
        assert!(world.processed(M).await);
        assert_eq!(reads_of(&world, M).await, [ExtractionOutcome::Proposed]);
        assert_eq!(world.extractor.claims().held(), 0);
    }
}

/// (2) The rescan holds M until its channel commit; the debounced live burst
/// defers M, while an urgent message in another channel is read and carded.
#[tokio::test(start_paused = true)]
async fn a_rescan_holding_a_row_never_blocks_other_live_reads() {
    for _ in 0..RUNS {
        let actions = vec![moved(M), added("N"), nothing()];
        // A slow pace keeps the rescan between its two conversations.
        let world = World::with(actions, |config| {
            config.drain_interval = Duration::from_secs(300);
        })
        .await;
        let (events, _loop) = world.pipeline();
        let m = message(M, MY, local(8, 29, 20, 0), QUIET);
        events.send(MessageEvent::Posted(m)).await.expect("send");
        after(1).await;
        // A second conversation only the rescan backfills.
        let history = FakeHistory::default();
        history.messages.lock().unwrap().insert(
            CHANNEL.into(),
            vec![message("102", MY, local(8, 30, 13, 0), "hstar wed 9pm?")],
        );
        let jobs = jobs_over(&world.extractor, history);
        let _worker = start(&jobs);
        let id = jobs.submit(startup()).await.expect("queued").job.id;
        after(1).await;
        assert_eq!(world.requests(), 1, "the rescan read M's conversation");

        // M's live burst comes due while the rescan holds it.
        after(95).await;
        let mut n = message(
            "N",
            MY,
            local(8, 30, 13, 2),
            "@here nstar ncarl tonight 9pm",
        );
        n.channel_id = OTHER.into();
        events.send(MessageEvent::Posted(n)).await.expect("send");
        after(5).await;
        assert_eq!(world.requests(), 2, "N was read; M was not read again");
        let cards = world.outbox.cards.lock().unwrap().clone();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].channel_id, OTHER);
        assert!(!world.processed(M).await, "the rescan has not committed");

        after(300).await;
        assert_eq!(world.requests(), 3, "the rescan's second conversation");
        assert!(world.processed(M).await);
        assert_eq!(reads_of(&world, M).await, [ExtractionOutcome::Proposed]);
        assert_eq!(world.live_proposals().await.len(), 2);
        assert_eq!(world.outbox.cards.lock().unwrap().len(), 2);
        let rescanned = result(&world, &id).await;
        assert_eq!(rescanned["calls"], 2);
        after(600).await;
        assert_eq!(world.requests(), 3, "nothing is re-offered once read");
        assert_eq!(world.extractor.claims().held(), 0);
    }
}

/// (4) The rescan owns M and its call fails; the live burst that deferred M
/// gets it back and reads it once.
#[tokio::test(start_paused = true)]
async fn a_failed_owner_reoffers_the_row_it_held() {
    for _ in 0..RUNS {
        let actions = vec![slow(20, FakeAction::Permanent), moved(M)];
        let world = World::with(actions, quick_debounce).await;
        let (events, _loop) = world.pipeline();
        events.send(post(QUIET)).await.expect("send");
        after(1).await;
        let jobs = jobs(&world.extractor);
        let _worker = start(&jobs);
        jobs.submit(startup()).await.expect("queued");
        after(10).await;
        assert_eq!(world.requests(), 1, "the live burst deferred M");
        after(40).await;
        assert_eq!(world.requests(), 2, "re-offered after the failure");
        assert_eq!(
            reads_of(&world, M).await,
            [ExtractionOutcome::Failed, ExtractionOutcome::Proposed]
        );
        assert!(world.processed(M).await);
        assert_eq!(world.live_proposals().await.len(), 1);
        after(200).await;
        assert_eq!(world.requests(), 2);
        assert_eq!(world.extractor.claims().held(), 0);
    }
}

/// (5) The live owner panics inside the model call: its claim is released
/// on unwind and the deferred row is re-offered and read once.
#[tokio::test(start_paused = true)]
async fn a_panicking_owner_releases_and_reoffers() {
    for _ in 0..RUNS {
        let world = World::new(vec![nothing(), moved(M)]).await;
        world.model.panic_on(0);
        let (events, _loop) = world.pipeline();
        events.send(post(URGENT)).await.expect("send");
        after(1).await;
        let jobs = jobs(&world.extractor);
        let _worker = start(&jobs);
        let id = jobs.submit(startup()).await.expect("queued").job.id;
        after(5).await;
        assert_eq!(result(&world, &id).await["deferred"], 1);
        after(60).await;
        assert_eq!(world.extractor.claims().held(), 0);
        assert_eq!(world.requests(), 2, "read again after the panic");
        assert_eq!(reads_of(&world, M).await, [ExtractionOutcome::Proposed]);
        assert!(world.processed(M).await);
        assert_eq!(world.live_proposals().await.len(), 1);
    }
}

/// (6) The rescan task is aborted mid-call (shutdown), with and without
/// cutting the calls first: its claim is released and the row re-offered.
#[tokio::test(start_paused = true)]
async fn an_aborted_rescan_releases_and_reoffers() {
    for run in 0..RUNS {
        let cut = run % 2 == 1;
        let actions = vec![slow(20, moved(M)), moved(M)];
        let world = World::with(actions, quick_debounce).await;
        let (events, _loop) = world.pipeline();
        events.send(post(QUIET)).await.expect("send");
        after(1).await;
        let jobs = jobs(&world.extractor);
        let worker = start(&jobs);
        jobs.submit(startup()).await.expect("queued");
        after(10).await;
        assert_eq!(world.requests(), 1, "the live burst deferred M");
        assert_eq!(world.extractor.claims().held(), 1);
        if cut {
            world.extractor.cancel_calls();
        }
        worker.abort();
        after(30).await;
        assert_eq!(world.extractor.claims().held(), 0);
        if cut {
            // Re-offered, but shutdown cuts the new read too: left unread.
            let logs = world.logs().await;
            let last = logs.last().expect("the re-offered read");
            assert_eq!(last.message_ids, [M]);
            assert_eq!(last.error.as_deref(), Some(CALL_CANCELLED));
            assert!(!world.processed(M).await);
            assert!(world.live_proposals().await.is_empty());
        } else {
            assert_eq!(world.requests(), 2);
            assert_eq!(reads_of(&world, M).await, [ExtractionOutcome::Proposed]);
            assert!(world.processed(M).await);
            assert_eq!(world.live_proposals().await.len(), 1);
        }
    }
}

/// (7) A panic after the processed mark (posting the card): M stays read, so
/// neither the backlog nor a later startup rescan admits it again.
#[tokio::test(start_paused = true)]
async fn a_panic_after_marking_never_repeats_the_effects() {
    for _ in 0..RUNS {
        let world = World::new(vec![slow(10, moved(M))]).await;
        world.outbox.panic_cards.store(true, Ordering::SeqCst);
        let (events, _loop) = world.pipeline();
        events.send(post(URGENT)).await.expect("send");
        after(1).await;
        let jobs = jobs(&world.extractor);
        let _worker = start(&jobs);
        let deferred = jobs.submit(startup()).await.expect("queued").job.id;
        after(5).await;
        assert_eq!(result(&world, &deferred).await["deferred"], 1);
        after(20).await;
        assert!(world.processed(M).await);
        assert_eq!(world.live_proposals().await.len(), 1);
        assert_eq!(world.extractor.claims().held(), 0);

        let again = jobs.submit(startup()).await.expect("queued").job.id;
        after(200).await;
        assert_eq!(result(&world, &again).await["calls"], 0);
        assert_eq!(world.requests(), 1, "no second admission");
        assert_eq!(world.live_proposals().await.len(), 1);
        assert_eq!(world.outbox.cards.lock().unwrap().len(), 1);
    }
}

/// (8) The processed mark cannot be written: nothing is proposed, M stays
/// unread and is offered again.
#[tokio::test(start_paused = true)]
async fn an_unwritable_mark_applies_nothing_and_reoffers() {
    for _ in 0..RUNS {
        let world = World::new(vec![slow(10, moved(M)), slow(10, moved(M))]).await;
        world.store.fail_exact_marks(true);
        let (events, _loop) = world.pipeline();
        events.send(post(URGENT)).await.expect("send");
        after(11).await;
        assert!(world.live_proposals().await.is_empty());
        assert!(world.outbox.cards.lock().unwrap().is_empty());
        assert!(!world.processed(M).await);
        let logs = world.logs().await;
        assert_eq!(logs[0].outcome, ExtractionOutcome::Failed);
        assert_eq!(logs[0].error.as_deref(), Some(MESSAGES_UNWRITABLE));
        world.store.fail_exact_marks(false);
        after(30).await;
        assert_eq!(world.requests(), 2, "offered again");
        assert!(world.processed(M).await);
        assert_eq!(world.live_proposals().await.len(), 1);
        assert_eq!(world.outbox.cards.lock().unwrap().len(), 1);
        assert_eq!(world.extractor.claims().held(), 0);
    }
}

const A: &str = "101";
const B: &str = "102";

/// One answer about both messages: A moves the Monday run, B adds a run.
fn both_changes() -> FakeAction {
    reply(&format!(
        r#"{{"amendments": [
            {{"kind": "move", "bosses": ["HMaleficStar", "HFA"], "day_ref": "wed",
              "time_ref": "9:30pm", "participants": ["{MY}"], "confidence": 0.9,
              "evidence_message_ids": ["{A}"]}},
            {{"kind": "add", "bosses": ["NMaleficStar", "NCarling"], "day_ref": "tonight",
              "time_ref": "9pm", "participants": ["{MY}"], "confidence": 0.9,
              "evidence_message_ids": ["{B}"]}}]}}"#
    ))
}

fn added_at(evidence: &str, time: &str) -> FakeAction {
    reply(&format!(
        r#"{{"amendments": [{{"kind": "add", "bosses": ["NMaleficStar", "NCarling"],
            "day_ref": "tonight", "time_ref": "{time}", "participants": ["{MY}"],
            "confidence": 0.9, "evidence_message_ids": ["{evidence}"]}}]}}"#
    ))
}

/// A and B are read in one call; B changes while the call is in flight.
async fn a_and_b_in_flight(
    actions: Vec<FakeAction>,
) -> (World, tokio::sync::mpsc::Sender<MessageEvent>) {
    let world = World::new(actions).await;
    // Dropping the handle detaches the loop; it runs for the whole test.
    let (events, _) = world.pipeline();
    let a = message(A, MY, local(8, 30, 13, 1), QUIET);
    events.send(MessageEvent::Posted(a)).await.expect("send");
    // Urgent: flushes A and B together at once.
    let b = message(B, MY, local(8, 30, 13, 2), "@here nstar ncarl tonight 9pm");
    events.send(MessageEvent::Posted(b)).await.expect("send");
    after(1).await;
    assert_eq!(world.requests(), 1);
    (world, events)
}

/// A sibling of an edited message is re-read and applied once; the edit is
/// admitted once and the stale answer about B applies nothing.
#[tokio::test(start_paused = true)]
async fn an_unedited_sibling_of_a_stale_row_is_read_again() {
    for _ in 0..RUNS {
        let actions = vec![slow(10, both_changes()), moved(A), added_at(B, "9:45pm")];
        let (world, events) = a_and_b_in_flight(actions).await;
        let edit = message(
            B,
            MY,
            local(8, 30, 13, 2),
            "@here nstar ncarl tonight 9:45pm",
        );
        events.send(MessageEvent::Edited(edit)).await.expect("send");
        after(20).await;
        let logs = world.logs().await;
        assert_eq!(logs[0].guardrail["stale_version"], true);
        assert!(logs[0].proposal_ids.is_empty(), "no stale effect");
        assert_eq!(world.requests(), 2, "A offered again and read");
        assert!(world.processed(A).await);
        assert!(!world.processed(B).await, "B's edit waits for its burst");
        assert_eq!(world.live_proposals().await.len(), 1);

        after(100).await;
        assert_eq!(world.requests(), 3, "B's edit admitted once");
        assert!(world.processed(B).await);
        assert_eq!(reads_of(&world, A).await.len(), 2);
        assert_eq!(reads_of(&world, B).await.len(), 2);
        let cards = world.outbox.cards.lock().unwrap().clone();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[1].entries[0].time_ref.as_deref(), Some("9:45pm"));
        assert_eq!(world.live_proposals().await.len(), 2);
        after(300).await;
        assert_eq!(world.requests(), 3);
        assert_eq!(world.extractor.claims().held(), 0);
    }
}

/// B is deleted while the call is in flight: A is still re-read once.
#[tokio::test(start_paused = true)]
async fn a_sibling_of_a_deleted_row_is_read_again() {
    for _ in 0..RUNS {
        let actions = vec![slow(10, both_changes()), moved(A)];
        let (world, events) = a_and_b_in_flight(actions).await;
        events
            .send(MessageEvent::Deleted { id: B.into() })
            .await
            .expect("send");
        after(20).await;
        assert_eq!(world.logs().await[0].guardrail["stale_version"], true);
        assert_eq!(world.requests(), 2, "A offered again and read");
        assert!(world.processed(A).await);
        let live = world.live_proposals().await;
        assert_eq!(live.len(), 1, "only A's change, once");
        assert_eq!(world.outbox.cards.lock().unwrap().len(), 1);
        after(300).await;
        assert_eq!(world.requests(), 2);
        assert_eq!(world.extractor.claims().held(), 0);
    }
}
