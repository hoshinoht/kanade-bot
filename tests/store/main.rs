//! The SQLite store against the shared conformance suite, plus migrations,
//! ownership, backup/restore and the scheduler's revision retry. Every test
//! works in its own fresh temp directory.

mod backup;
mod drafts;
mod failures;
mod history;
mod journal;
mod migrate;
mod model_logs;
mod owner;
mod retry;
mod support;

use kanade::infrastructure::store::{
    MemoryScheduleStore, SqliteStore, attendance_conformance, card_conformance,
    cherry_pick_conformance, conformance, draft_conformance, history_conformance,
    journal_conformance, model_log_conformance, precondition_conformance, proposal_conformance,
    web_sessions_conformance,
};

#[tokio::test]
async fn memory_members_conform() {
    kanade::infrastructure::store::members_conformance::run_suite(async || {
        MemoryScheduleStore::new()
    })
    .await;
}

#[tokio::test]
async fn sqlite_members_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    kanade::infrastructure::store::members_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("members-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_web_sessions_conform() {
    web_sessions_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_web_sessions_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    web_sessions_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("sessions-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_model_logs_conform() {
    model_log_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_model_logs_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    model_log_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("logs-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_proposal_cards_conform() {
    card_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_proposal_cards_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    card_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("cards-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_proposals_conform() {
    proposal_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_proposals_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    proposal_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("proposals-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn sqlite_store_conforms() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("conform-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_journal_conforms() {
    journal_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_journal_conforms() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    journal_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("journal-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_drafts_conform() {
    draft_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_drafts_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    draft_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("drafts-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_attendance_conforms() {
    attendance_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_attendance_conforms() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    attendance_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("attendance-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_cherry_picks_conform() {
    cherry_pick_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_cherry_picks_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    cherry_pick_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("picks-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_preconditions_conform() {
    precondition_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_preconditions_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    precondition_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("preconditions-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_history_conforms() {
    history_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_history_conforms() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    history_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("history-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}
