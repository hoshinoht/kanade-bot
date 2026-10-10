//! A run completion prompt's buttons (user decisions 2026-10-09/10): only the
//! run's party or staff answers (no bossing role needed); the press claims
//! the ask first (one winner), then Done and Didn't happen settle the run
//! through the shared writer's guarded op as the presser, with no change
//! notice; Not yet opens the next ask half an hour later until the cutoff.
//! `/status` itself still announces (pinned in `runs.rs`).

use chrono::TimeDelta;
use serde_json::json;

use kanade::domain::completion::{PromptOutcome, RunPrompt, RunPromptStore};
use kanade::domain::history::{Actor, ChangeHistory, Surface};
use kanade::domain::notify::NoticeOutbox;
use kanade::domain::schedule::RunStatus;

use super::super::support::{ADMIN_ROLE, BOSSING_ROLE};
use super::{ALICE, BOB, DAN, R_KALOS, R_NEXT, Slash, now, opt, utc};

/// `R_KALOS`'s first ask (the run starts 14:00Z with 30 minutes of XKalos,
/// so it ends 14:30Z), posted, cut off `cutoff` from now.
async fn asked(slash: &Slash, cutoff: TimeDelta) -> RunPrompt {
    let due = now() - TimeDelta::minutes(5);
    let prompt = RunPrompt::open(R_KALOS.into(), 0, utc(9, 29, 14, 30), due, now() + cutoff);
    slash.store.create_run_prompt(prompt.clone()).await.unwrap();
    slash
        .store
        .set_run_prompt_message(R_KALOS.into(), 0, "301".into(), "9001".into())
        .await
        .unwrap();
    prompt
}

fn button(kind: &str, ask: u32) -> String {
    format!("run:{kind}:{R_KALOS}:{ask}")
}

async fn ask(slash: &Slash, ask: u32) -> RunPrompt {
    slash
        .store
        .run_prompt(R_KALOS.into(), ask)
        .await
        .unwrap()
        .expect("ask")
}

#[tokio::test]
async fn someone_off_the_run_is_refused_and_nothing_changes() {
    let slash = Slash::new().await;
    asked(&slash, TimeDelta::hours(8)).await;
    for kind in ["done", "missed", "later"] {
        assert_eq!(
            slash.press(DAN, &button(kind, 0)).await,
            "❌ Only the run's party or an admin can answer this."
        );
    }
    assert_eq!(slash.run_row(R_KALOS).await.status, RunStatus::Planned);
    assert!(ask(&slash, 0).await.is_open());
}

#[tokio::test]
async fn a_party_member_s_done_marks_the_run_done_without_a_notice() {
    let slash = Slash::new().await;
    asked(&slash, TimeDelta::hours(8)).await;
    let head = slash.store.history_head().await.unwrap().seq;
    assert_eq!(
        slash.press(BOB, &button("done", 0)).await,
        "🏁 Marked run `#11111111` done."
    );
    assert_eq!(slash.run_row(R_KALOS).await.status, RunStatus::Done);
    let record = slash.store.load_change(head + 1).await.unwrap().unwrap();
    assert_eq!(record.origin.actor, Actor::member(BOB.to_string()));
    assert_eq!(record.origin.surface, Surface::Discord);
    assert_eq!(
        record.origin.request_id.as_deref(),
        Some(format!("run-outcome:{R_KALOS}:0:done").as_str()),
        "History names the outcome"
    );
    assert!(record.notices.is_empty(), "no change notice");
    assert!(
        slash
            .store
            .pending_notices()
            .await
            .unwrap()
            .notices
            .is_empty(),
        "nothing waits in the outbox"
    );
    let closed = ask(&slash, 0).await;
    assert_eq!(
        (closed.outcome, closed.decided_by.as_deref()),
        (Some(PromptOutcome::Done), Some("1002"))
    );
    assert_eq!(
        slash.press(ALICE, &button("missed", 0)).await,
        "❌ That prompt was already answered."
    );
    assert_eq!(slash.run_row(R_KALOS).await.status, RunStatus::Done);
}

#[tokio::test]
async fn an_admin_off_the_run_records_that_it_did_not_happen() {
    let slash = Slash::new().await;
    asked(&slash, TimeDelta::hours(8)).await;
    let head = slash.store.history_head().await.unwrap().seq;
    assert_eq!(
        slash
            .press_as(DAN, &[BOSSING_ROLE, ADMIN_ROLE], &button("missed", 0))
            .await,
        "Marked run `#11111111` as didn't happen."
    );
    assert_eq!(slash.run_row(R_KALOS).await.status, RunStatus::Cancelled);
    let record = slash.store.load_change(head + 1).await.unwrap().unwrap();
    assert_eq!(record.origin.actor, Actor::member(DAN.to_string()));
    assert_eq!(
        record.origin.request_id.as_deref(),
        Some(format!("run-outcome:{R_KALOS}:0:didnt-happen").as_str())
    );
    assert!(record.notices.is_empty(), "no change notice");
    let closed = ask(&slash, 0).await;
    assert_eq!(
        (closed.outcome, closed.decided_by.as_deref()),
        (Some(PromptOutcome::DidntHappen), Some("1004"))
    );
}

#[tokio::test]
async fn not_yet_asks_again_half_an_hour_later_until_the_cutoff() {
    let slash = Slash::new().await;
    let first = asked(&slash, TimeDelta::hours(8)).await;
    // 04:00Z is 12:00 in Kuala Lumpur.
    assert_eq!(
        slash.press(ALICE, &button("later", 0)).await,
        "⏳ I'll ask again about run `#11111111` at 12:30."
    );
    let closed = ask(&slash, 0).await;
    assert_eq!(
        (closed.outcome, closed.decided_by.as_deref()),
        (Some(PromptOutcome::NotYet), Some("1001"))
    );
    let next = ask(&slash, 1).await;
    assert!(next.is_open());
    assert_eq!(next.due_at, now() + TimeDelta::minutes(30));
    assert_eq!(
        (next.ends_at, next.cutoff_at),
        (first.ends_at, first.cutoff_at)
    );
    assert_eq!(slash.run_row(R_KALOS).await.status, RunStatus::Planned);
    assert_eq!(
        slash.press(BOB, &button("done", 0)).await,
        "❌ That prompt was already answered.",
        "the old ask's buttons are spent"
    );
}

#[tokio::test]
async fn not_yet_inside_the_last_half_hour_leaves_the_prompt_to_the_cutoff() {
    let slash = Slash::new().await;
    asked(&slash, TimeDelta::minutes(20)).await;
    assert_eq!(
        slash.press(BOB, &button("later", 0)).await,
        "Okay. If nobody marks it, run `#11111111` will be marked done at 12:20."
    );
    let closed = ask(&slash, 0).await;
    assert_eq!(
        (closed.outcome, closed.decided_by.as_deref()),
        (Some(PromptOutcome::NotYet), Some("1002")),
        "the press is recorded without a next ask"
    );
    assert_eq!(
        slash.store.run_prompt(R_KALOS.into(), 1).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn an_admin_without_the_bossing_role_may_answer() {
    let slash = Slash::new().await;
    asked(&slash, TimeDelta::hours(8)).await;
    assert_eq!(
        slash.press_as(DAN, &[ADMIN_ROLE], &button("done", 0)).await,
        "🏁 Marked run `#11111111` done."
    );
    assert_eq!(slash.run_row(R_KALOS).await.status, RunStatus::Done);
}

#[tokio::test]
async fn a_second_press_of_either_kind_loses_and_writes_nothing() {
    let slash = Slash::new().await;
    asked(&slash, TimeDelta::hours(8)).await;
    slash.press(ALICE, &button("done", 0)).await;
    let head = slash.store.history_head().await.unwrap().seq;
    for kind in ["missed", "later", "done"] {
        assert_eq!(
            slash.press(BOB, &button(kind, 0)).await,
            "❌ That prompt was already answered."
        );
    }
    assert_eq!(slash.store.history_head().await.unwrap().seq, head);
    assert_eq!(slash.run_row(R_KALOS).await.status, RunStatus::Done);
}

#[tokio::test]
async fn not_yet_first_then_done_on_the_same_ask_loses() {
    let slash = Slash::new().await;
    asked(&slash, TimeDelta::hours(8)).await;
    slash.press(ALICE, &button("later", 0)).await;
    assert_eq!(
        slash.press(BOB, &button("done", 0)).await,
        "❌ That prompt was already answered."
    );
    assert_eq!(slash.run_row(R_KALOS).await.status, RunStatus::Planned);
}

#[tokio::test]
async fn a_press_after_a_cancel_elsewhere_changes_nothing() {
    let slash = Slash::new().await;
    asked(&slash, TimeDelta::hours(8)).await;
    slash
        .run(
            ALICE,
            "status",
            json!([opt("run_id", R_KALOS), opt("state", "cancelled")]),
        )
        .await;
    let reply = slash.press(BOB, &button("done", 0)).await;
    assert!(
        reply.starts_with("❌ Run `#11111111` is already"),
        "{reply}"
    );
    assert_eq!(slash.run_row(R_KALOS).await.status, RunStatus::Cancelled);
    assert!(ask(&slash, 0).await.is_open(), "the tick closes it");
}

#[tokio::test]
async fn a_press_after_a_move_elsewhere_changes_nothing() {
    let slash = Slash::new().await;
    asked(&slash, TimeDelta::hours(8)).await;
    slash
        .run(
            ALICE,
            "amend",
            json!([opt("run_id", R_KALOS), opt("to", "tue 23:00")]),
        )
        .await;
    assert_eq!(
        slash.press(BOB, &button("done", 0)).await,
        "❌ That run changed before your answer landed; nothing was recorded."
    );
    assert_eq!(slash.run_row(R_KALOS).await.status, RunStatus::Planned);
    assert!(ask(&slash, 0).await.is_open(), "the tick re-plans it");
}

#[tokio::test]
async fn a_forged_button_naming_another_run_is_inactive() {
    let slash = Slash::new().await;
    asked(&slash, TimeDelta::hours(8)).await;
    assert_eq!(
        slash.press(ALICE, &format!("run:done:{R_NEXT}:0")).await,
        "❌ This button is no longer active."
    );
    assert_eq!(slash.run_row(R_NEXT).await.status, RunStatus::Planned);
    assert!(ask(&slash, 0).await.is_open());
}

#[tokio::test]
async fn a_button_for_no_stored_ask_is_inactive() {
    let slash = Slash::new().await;
    asked(&slash, TimeDelta::hours(8)).await;
    assert_eq!(
        slash.press(ALICE, &button("done", 3)).await,
        "❌ This button is no longer active."
    );
    assert_eq!(slash.run_row(R_KALOS).await.status, RunStatus::Planned);
}
