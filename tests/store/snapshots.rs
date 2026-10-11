//! Composed admin reads (the History page, the Inbox count) answer from one
//! read transaction: a commit landing between their statements is in none
//! of them. The store's test pause stops each read after its first
//! statement while the test commits.

use std::sync::Arc;

use kanade::domain::drafts::{DraftKind, DraftStore, NewDraft, Submission};
use kanade::domain::history::{Actor, ChangeFilter, ChangeHistory};
use kanade::domain::requests::DEFAULT_LIMITS;
use kanade::domain::schedule::Change;
use kanade::infrastructure::store::SqliteStore;
use kanade::infrastructure::store::snapshots::{HistoryViewQuery, SnapshotReads};

use crate::support::{TempDir, commit, fixed, seed};

fn everything() -> HistoryViewQuery {
    HistoryViewQuery {
        filter: ChangeFilter::All,
        before: None,
        limit: 100,
        settings: None,
    }
}

async fn close(store: Arc<SqliteStore>) {
    Arc::into_inner(store)
        .expect("last handle")
        .close()
        .await
        .expect("close");
}

#[tokio::test]
async fn a_history_page_never_mixes_in_a_commit_made_mid_read() {
    let dir = TempDir::new();
    let store = Arc::new(
        SqliteStore::open(&dir.config("snapshot-history"))
            .await
            .expect("opens"),
    );
    seed(&*store).await;
    let pause = store.pause_next_snapshot_read();
    let reader = {
        let store = Arc::clone(&store);
        tokio::spawn(async move { store.history_view(everything()).await })
    };
    pause.reached.await.expect("paused after the head");
    commit(&*store, vec![Change::PutFixedRun(fixed("f-mid"))]).await;
    pause.resume.send(()).expect("resume");
    let view = reader.await.expect("joined").expect("read");

    // Head, total and page all stop before the commit made mid-read.
    assert_eq!(view.records.first().map(|r| r.seq), Some(view.head.seq));
    assert_eq!(
        view.total, view.head.seq,
        "seqs are contiguous from genesis"
    );
    assert_eq!(view.records.len() as u64, view.total);
    let after = store.history_view(everything()).await.expect("read");
    assert_eq!(after.head.seq, view.head.seq + 1, "the commit landed");
    assert_eq!(after.head, store.history_head().await.expect("head"));
    assert_eq!(after.total, view.total + 1);
    close(store).await;
}

#[tokio::test]
async fn the_inbox_count_never_mixes_in_a_request_made_mid_read() {
    let dir = TempDir::new();
    let store = Arc::new(
        SqliteStore::open(&dir.config("snapshot-inbox"))
            .await
            .expect("opens"),
    );
    let base = store.history_head().await.expect("head");
    let pause = store.pause_next_snapshot_read();
    let reader = {
        let store = Arc::clone(&store);
        tokio::spawn(async move { store.inbox_count().await })
    };
    pause.reached.await.expect("paused after the proposals");
    store
        .create_draft(NewDraft {
            id: "q-1".into(),
            kind: DraftKind::Request,
            title: "join".into(),
            author: Actor::member("1"),
            base,
            base_revision: 0,
            request_type: Some("join".into()),
            subject: Some("run:r-1".into()),
            at: chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 9, 29, 4, 0, 0).unwrap(),
            request: None,
            submit: Some(Submission {
                ops: Vec::new(),
                expires_week: None,
                limits: DEFAULT_LIMITS,
            }),
        })
        .await
        .expect("request submitted");
    pause.resume.send(()).expect("resume");
    assert_eq!(reader.await.expect("joined").expect("count"), 0);
    assert_eq!(store.inbox_count().await.expect("count"), 1, "it landed");
    close(store).await;
}
