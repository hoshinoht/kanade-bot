//! A4 and A5 writes leave their notices in the store's outbox, written with
//! the change itself; an idempotent retry adds none.

use kanade::domain::history::ChangeHistory;
use kanade::domain::notify::{NoticeOutbox, change_source};
use kanade::domain::schedule::{Notice, NoticeChange};
use kanade::infrastructure::store::SqliteStore;
use serde_json::json;

use crate::reads::Reads;

/// The outbox notices change `seq` wrote, in order.
pub(crate) async fn written_by(store: &SqliteStore, seq: u64) -> Vec<Notice> {
    let source = change_source(seq);
    let rows: Vec<_> = store
        .outbox_notices()
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.source == source)
        .collect();
    let ordinals: Vec<i64> = rows.iter().map(|row| row.ordinal).collect();
    assert_eq!(ordinals, (0..rows.len() as i64).collect::<Vec<_>>());
    rows.into_iter().map(|row| row.notice).collect()
}

/// Every outbox row belongs to the change whose record names its kind.
async fn kinds_match_the_record(store: &SqliteStore, seq: u64) -> Vec<Notice> {
    let written = written_by(store, seq).await;
    let record = store.load_change(seq).await.unwrap().unwrap();
    let kinds: Vec<String> = written.iter().map(Notice::effect_kind).collect();
    let recorded: Vec<String> = record
        .notices
        .into_iter()
        .filter(|kind| kind != "notice.edit.override")
        .collect();
    assert_eq!(kinds, recorded, "change {seq}");
    written
}

#[tokio::test]
async fn a4_writes_and_rollbacks_enqueue_their_notices_once() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    let key = [("Idempotency-Key", "outbox-move")];
    let body = json!({"day": 6, "time": "21:00", "version": v});
    let moved = reads
        .call("POST", "/api/admin/runs/r-kalos/move", body.clone(), &key)
        .await;
    assert_eq!(moved.status, 200, "{}", moved.text());
    let notices = kinds_match_the_record(&reads.store, v + 1).await;
    assert!(
        matches!(&notices[..], [Notice { change: NoticeChange::RunMoved { run_id, .. }, .. }] if run_id == "r-kalos"),
        "{notices:?}"
    );
    let total = reads.store.outbox_notices().await.unwrap().len();
    let again = reads
        .call("POST", "/api/admin/runs/r-kalos/move", body, &key)
        .await;
    assert_eq!(again.status, 200, "{}", again.text());
    assert_eq!(
        reads.store.outbox_notices().await.unwrap().len(),
        total,
        "a replayed move enqueues nothing"
    );

    let edit = json!({
        "weekday": 1, "time": "22:00", "bosses": "xkalos", "participants": ["1001", "1002"],
        "channel_id": "kalos-four", "note": "bring pots", "version": reads.version().await,
    });
    let edited = reads
        .call("PATCH", "/api/admin/fixed/f-kalos", edit, &[])
        .await;
    assert_eq!(edited.status, 200, "{}", edited.text());
    let fixed = kinds_match_the_record(&reads.store, reads.version().await).await;
    assert!(
        fixed
            .iter()
            .any(|notice| matches!(notice.change, NoticeChange::FixedChanged { .. })),
        "{fixed:?}"
    );

    let revert = reads
        .call(
            "POST",
            "/api/admin/history/revert",
            json!({"seqs": [v + 1], "force": true}),
            &[("Idempotency-Key", "outbox-revert")],
        )
        .await;
    assert_eq!(revert.status, 200, "{}", revert.text());
    let rollback = kinds_match_the_record(&reads.store, reads.version().await).await;
    assert!(
        !rollback.is_empty()
            && rollback
                .iter()
                .all(|notice| matches!(notice.change, NoticeChange::Rollback { .. })),
        "{rollback:?}"
    );
    assert!(
        reads
            .store
            .pending_notices()
            .await
            .unwrap()
            .iter()
            .all(|row| row.drained_at.is_none())
    );
}
