//! SQLite-only journal behaviour: the card index and recovery across a real
//! close and reopen.

use kanade::bot::events::CardIndex;
use kanade::domain::notify::{
    AttemptState, Claim, DeliveryJournal, DeliveryTarget, EffectKind, IntentContent,
    NotificationIntent, Receipt,
};
use kanade::domain::scheduler::{ScheduleStore, Scope};
use kanade::infrastructure::store::SqliteStore;
use twilight_model::id::Id;

use crate::support::{TempDir, at, seed};

fn intent(channel: &str) -> NotificationIntent {
    NotificationIntent {
        effect: EffectKind::Reminder,
        effect_context: Vec::new(),
        channel_id: channel.into(),
        targets: vec![DeliveryTarget::Reminder("m-1".into())],
        mentions: Vec::new(),
        content: IntentContent::DayOf {
            run_ids: vec!["r-1".into()],
        },
        warnings: Vec::new(),
    }
}

/// Seeded `m-1` is already sent; put it back to unsent for claiming.
async fn unsent(store: &SqliteStore) {
    let state = store.load(&Scope::All).await.expect("load");
    let mut row = state.reminders[0].clone();
    row.sent_at = None;
    row.message_id = None;
    store
        .commit(
            state.revision,
            kanade::domain::schedule::ChangeSet {
                changes: vec![kanade::domain::schedule::Change::PutReminder(row)],
            },
            kanade::infrastructure::store::conformance::meta(),
        )
        .await
        .expect("commit");
}

#[tokio::test]
async fn card_index_finds_runs_by_the_bound_message_in_any_channel() {
    let dir = TempDir::new();
    let store = SqliteStore::open(&dir.config("cards"))
        .await
        .expect("opens");
    seed(&store).await;
    unsent(&store).await;
    assert!(
        store
            .runs_for_message(Id::new(4242))
            .await
            .expect("lookup")
            .is_empty()
    );
    let lease = store
        .begin_lease("instance", "delivery", at(30, 7))
        .await
        .expect("lease");
    // The run's home is 900; the card fell back to 777.
    let Claim::Fresh(attempt) = store
        .claim(&lease, &intent("777"), None, at(30, 8))
        .await
        .expect("claim")
    else {
        panic!("fresh claim expected");
    };
    let receipt = Receipt {
        channel_id: "777".into(),
        message_id: "5001".into(),
    };
    store
        .bind(&lease, &attempt, &receipt, None, at(30, 8))
        .await
        .expect("bind");
    assert_eq!(
        store.runs_for_message(Id::new(5001)).await.expect("lookup"),
        ["r-1"]
    );
    assert!(
        store
            .runs_for_message(Id::new(5002))
            .await
            .expect("lookup")
            .is_empty()
    );
    store.close().await.expect("close");
}

#[tokio::test]
async fn an_intent_left_by_a_crash_is_indeterminate_after_reopen() {
    let dir = TempDir::new();
    let config = dir.config("crash");
    let store = SqliteStore::open(&config).await.expect("opens");
    seed(&store).await;
    unsent(&store).await;
    let lease = store
        .begin_lease("instance-1", "delivery", at(30, 7))
        .await
        .expect("lease");
    let Claim::Fresh(attempt) = store
        .claim(&lease, &intent("900"), None, at(30, 8))
        .await
        .expect("claim")
    else {
        panic!("fresh claim expected");
    };
    // The process dies between claim and send.
    drop(store);

    let store = SqliteStore::open(&config).await.expect("reopens");
    let recovery = store.recover_on_start(at(30, 9)).await.expect("recovery");
    assert_eq!(recovery.indeterminate, std::slice::from_ref(&attempt));
    assert_eq!(recovery.orphaned_leases, 1);
    let record = store
        .load_attempt(&attempt)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(record.state, AttemptState::Indeterminate);
    let lease = store
        .begin_lease("instance-2", "delivery", at(30, 9))
        .await
        .expect("lease");
    assert_eq!(
        store.claim(&lease, &intent("900"), None, at(30, 9)).await,
        Ok(Claim::Held),
        "never resent"
    );
    store.close().await.expect("close");
}

#[tokio::test]
async fn a_cancelled_journal_write_leaves_the_next_one_working() {
    use kanade::domain::schedule::{Change, ChangeSet, Reminder};
    use std::time::Duration;

    let dir = TempDir::new();
    let store = SqliteStore::open(&dir.config("cancel-claim"))
        .await
        .expect("opens");
    seed(&store).await;
    let lease = store
        .begin_lease("instance", "delivery", at(30, 7))
        .await
        .expect("lease");
    let mut cancelled = 0;
    for (round, micros) in [0, 50, 100, 200, 400, 800, 1_600, 3_200, 6_400, 204_800]
        .into_iter()
        .enumerate()
    {
        // Many unsent targets make one claim a long transaction.
        let ids: Vec<String> = (0..300).map(|n| format!("c{round}-{n}")).collect();
        let changes = ids
            .iter()
            .map(|id| {
                Change::PutReminder(Reminder {
                    id: id.clone(),
                    run_id: "r-1".into(),
                    kind: id.clone(),
                    fire_at: at(30, 8),
                    sent_at: None,
                    message_id: None,
                })
            })
            .collect();
        let revision = store.load(&Scope::All).await.expect("load").revision;
        store
            .commit(
                revision,
                ChangeSet { changes },
                kanade::infrastructure::store::conformance::meta(),
            )
            .await
            .expect("seed reminders");
        let group = NotificationIntent {
            targets: ids.iter().cloned().map(DeliveryTarget::Reminder).collect(),
            ..intent("900")
        };
        let attempt = tokio::time::timeout(
            Duration::from_micros(micros),
            store.claim(&lease, &group, None, at(30, 8)),
        )
        .await;
        if attempt.is_err() {
            cancelled += 1;
        }
        // A cancelled claim landed whole or not at all; either way the next
        // journal write succeeds and the group ends up held exactly once.
        let next = store
            .claim(&lease, &group, None, at(30, 8))
            .await
            .expect("the next journal write succeeds");
        match (&attempt, &next) {
            (Ok(Ok(Claim::Fresh(_))), Claim::Held) | (Err(_), _) => {}
            other => panic!("round {round}: {other:?}"),
        }
        let view = store.load_view().await.expect("view");
        let held = ids
            .iter()
            .filter(|id| {
                view.targets()
                    .contains(&DeliveryTarget::Reminder((*id).clone()))
            })
            .count();
        assert_eq!(held, 300, "round {round}");
    }
    assert!(cancelled > 0, "at least one claim was cancelled");
    store.close().await.expect("close");
}
