//! Completion prompts every store must keep: one open ask per run, closed
//! exactly once, the next ask stored with the close or not at all, and only
//! posted closed asks waiting to be settled.

use chrono::{DateTime, TimeZone, Utc};

use crate::domain::completion::{PromptClose, PromptOutcome, RunPrompt, RunPromptStore};
use crate::domain::scheduler::StoreError;

pub async fn run_suite<S: RunPromptStore>(make: impl AsyncFn() -> S) {
    asks_round_trip_and_list_open_by_due_time(make().await).await;
    one_open_ask_per_run(make().await).await;
    an_ask_closes_once_with_its_next_ask(make().await).await;
    a_refused_next_ask_leaves_the_close_unapplied(make().await).await;
    only_posted_closed_asks_await_settling(make().await).await;
}

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 10, hour, 0, 0).unwrap()
}

fn ask(run: &str, ask: u32, due: u32) -> RunPrompt {
    RunPrompt::open(run.into(), ask, at(due - 1), at(due), at(23))
}

fn close(outcome: PromptOutcome, by: Option<&str>, next: Option<RunPrompt>) -> PromptClose {
    PromptClose {
        outcome,
        decided_by: by.map(str::to_owned),
        at: at(12),
        next,
    }
}

async fn asks_round_trip_and_list_open_by_due_time<S: RunPromptStore>(store: S) {
    store.create_run_prompt(ask("r-2", 0, 9)).await.unwrap();
    store.create_run_prompt(ask("r-1", 0, 10)).await.unwrap();
    store
        .set_run_prompt_message("r-2".into(), 0, "701".into(), "9001".into())
        .await
        .unwrap();
    let stored = store.run_prompt("r-2".into(), 0).await.unwrap().unwrap();
    assert_eq!(
        (stored.channel_id.as_deref(), stored.message_id.as_deref()),
        (Some("701"), Some("9001"))
    );
    assert_eq!(stored.due_at, at(9));
    assert_eq!(store.run_prompt("r-2".into(), 1).await.unwrap(), None);
    let open = store.open_run_prompts().await.unwrap();
    assert_eq!(
        open.iter().map(RunPrompt::key).collect::<Vec<_>>(),
        ["r-2:0", "r-1:0"],
        "run prompts: open asks earliest due first"
    );
    assert!(
        matches!(
            store
                .create_run_prompt(RunPrompt {
                    cutoff_at: at(9),
                    ..ask("r-3", 0, 9)
                })
                .await,
            Err(StoreError::Constraint(_))
        ),
        "run prompts: the cutoff follows the due time"
    );
}

async fn one_open_ask_per_run<S: RunPromptStore>(store: S) {
    store.create_run_prompt(ask("r-1", 0, 9)).await.unwrap();
    for duplicate in [ask("r-1", 0, 10), ask("r-1", 1, 10)] {
        assert!(
            matches!(
                store.create_run_prompt(duplicate).await,
                Err(StoreError::Constraint(_))
            ),
            "run prompts: one open ask per run and one row per ask"
        );
    }
    assert_eq!(store.open_run_prompts().await.unwrap().len(), 1);
}

async fn an_ask_closes_once_with_its_next_ask<S: RunPromptStore>(store: S) {
    store.create_run_prompt(ask("r-1", 0, 9)).await.unwrap();
    assert!(
        matches!(
            store
                .close_run_prompt("r-1".into(), 0, close(PromptOutcome::Done, None, None))
                .await,
            Err(StoreError::Constraint(_))
        ),
        "run prompts: a press names its presser"
    );
    let not_yet = close(PromptOutcome::NotYet, Some("1001"), Some(ask("r-1", 1, 13)));
    assert!(
        store
            .close_run_prompt("r-1".into(), 0, not_yet.clone())
            .await
            .unwrap()
    );
    assert!(
        !store
            .close_run_prompt("r-1".into(), 0, not_yet)
            .await
            .unwrap(),
        "run prompts: closed once"
    );
    let first = store.run_prompt("r-1".into(), 0).await.unwrap().unwrap();
    assert_eq!(
        (first.outcome, first.decided_by.as_deref(), first.decided_at),
        (Some(PromptOutcome::NotYet), Some("1001"), Some(at(12)))
    );
    let latest = store
        .latest_run_prompt("r-1".into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!((latest.key(), latest.is_open()), ("r-1:1".to_owned(), true));
    assert!(
        store
            .close_run_prompt("r-1".into(), 1, close(PromptOutcome::AutoDone, None, None))
            .await
            .unwrap()
    );
    assert!(store.open_run_prompts().await.unwrap().is_empty());
    assert_eq!(store.latest_run_prompt("r-9".into()).await.unwrap(), None);
}

async fn a_refused_next_ask_leaves_the_close_unapplied<S: RunPromptStore>(store: S) {
    store.create_run_prompt(ask("r-1", 0, 9)).await.unwrap();
    // Ask 0 again: that row exists, so the whole write is refused.
    let refused = close(PromptOutcome::Moved, None, Some(ask("r-1", 0, 14)));
    assert!(matches!(
        store.close_run_prompt("r-1".into(), 0, refused).await,
        Err(StoreError::Constraint(_))
    ));
    let other_run = close(PromptOutcome::Moved, None, Some(ask("r-2", 1, 14)));
    assert!(matches!(
        store.close_run_prompt("r-1".into(), 0, other_run).await,
        Err(StoreError::Constraint(_))
    ));
    assert!(
        store
            .run_prompt("r-1".into(), 0)
            .await
            .unwrap()
            .unwrap()
            .is_open(),
        "run prompts: a refused next ask leaves the ask open"
    );
}

async fn only_posted_closed_asks_await_settling<S: RunPromptStore>(store: S) {
    for run in ["r-1", "r-2", "r-3"] {
        store.create_run_prompt(ask(run, 0, 9)).await.unwrap();
    }
    for run in ["r-1", "r-3"] {
        store
            .set_run_prompt_message(run.into(), 0, "701".into(), "9001".into())
            .await
            .unwrap();
    }
    for run in ["r-1", "r-2"] {
        assert!(
            store
                .close_run_prompt(run.into(), 0, close(PromptOutcome::Closed, None, None))
                .await
                .unwrap()
        );
    }
    let unsettled = store.unsettled_run_prompts().await.unwrap();
    assert_eq!(
        unsettled.iter().map(RunPrompt::key).collect::<Vec<_>>(),
        ["r-1:0"],
        "run prompts: only a posted, closed ask awaits its edit"
    );
    store
        .settle_run_prompt_message("r-1".into(), 0)
        .await
        .unwrap();
    assert!(store.unsettled_run_prompts().await.unwrap().is_empty());
}
