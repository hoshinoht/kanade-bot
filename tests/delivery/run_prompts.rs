//! Run completion prompts on the tick (user decisions 2026-10-09/10), on one
//! fixture-scoped pinned clock (`scenarios::now`, Thu 10 Sep 20:00 Kuala
//! Lumpur). Without a catalog every boss lasts the default 30 minutes.

use chrono::{DateTime, TimeDelta, Utc};
use kanade::bot::transport::Call;
use kanade::domain::completion::{PromptClose, PromptOutcome, RunPrompt};
use kanade::domain::history::{Actor, ChangeHistory, Origin};
use kanade::domain::ids::RandomIds;
use kanade::domain::schedule::{NewRun, RunSource, RunStatus, StatusChange};
use kanade::domain::scheduler::Scope;
use twilight_model::channel::message::Component;

use crate::scenarios::{HOME, config, delivery, now, week, world};
use crate::support::{self, Store, on_both_stores};

/// A run that started an hour before `now()`: ends 30 minutes before it, so
/// its prompt is due at `now()`; its cutoff is local midnight, 16:00Z.
async fn run<S: Store>(store: &S, status: RunStatus, at: DateTime<Utc>) -> String {
    let mut ids = RandomIds;
    support::service(store, &mut ids, now() - TimeDelta::hours(2))
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(HOME.into()),
            week_start: week(),
            datetime: at,
            bosses: vec!["Kalos".into()],
            participants: vec!["1001".into(), "1002".into()],
            status,
            source: RunSource::Amend,
        })
        .await
        .expect("run")
}

fn started() -> DateTime<Utc> {
    now() - TimeDelta::hours(1)
}

fn cutoff() -> DateTime<Utc> {
    now() + TimeDelta::hours(4)
}

fn reset() -> DateTime<Utc> {
    week() + TimeDelta::days(7)
}

fn texts(components: &[Component]) -> Vec<String> {
    let mut out = Vec::new();
    for component in components {
        match component {
            Component::TextDisplay(text) => out.push(text.content.clone()),
            Component::Container(container) => out.extend(texts(&container.components)),
            _ => {}
        }
    }
    out
}

fn buttons(components: &[Component]) -> Vec<String> {
    let mut out = Vec::new();
    for component in components {
        match component {
            Component::Button(button) => out.extend(button.custom_id.clone()),
            Component::ActionRow(row) => out.extend(buttons(&row.components)),
            Component::Container(container) => out.extend(buttons(&container.components)),
            _ => {}
        }
    }
    out
}

/// Prompt posts, in order.
fn prompts(calls: &[Call]) -> Vec<Vec<String>> {
    calls
        .iter()
        .filter_map(|call| match call {
            Call::Create { message, .. }
                if texts(&message.components)
                    .iter()
                    .any(|text| text.contains("Did this run happen?")) =>
            {
                assert!(message.allowed_mentions.users.is_empty(), "pings nobody");
                Some(buttons(&message.components))
            }
            _ => None,
        })
        .collect()
}

/// Every post other than prompts (change notices among them).
fn other_posts(calls: &[Call]) -> usize {
    calls
        .iter()
        .filter(|call| matches!(call, Call::Create { .. }))
        .count()
        - prompts(calls).len()
}

/// The edits, as their text and remaining buttons.
fn edits(calls: &[Call]) -> Vec<(String, Vec<String>)> {
    calls
        .iter()
        .filter_map(|call| match call {
            Call::Edit { edit, .. } => {
                let components = edit.components.clone().unwrap_or_default();
                assert!(
                    edit.allowed_mentions.users.is_empty(),
                    "the edit pings nobody"
                );
                Some((texts(&components).join("\n"), buttons(&components)))
            }
            _ => None,
        })
        .collect()
}

async fn status<S: Store>(store: &S, id: &str) -> RunStatus {
    store
        .load(&Scope::All)
        .await
        .unwrap()
        .runs
        .into_iter()
        .find(|run| run.id == id)
        .unwrap()
        .status
}

async fn once_across_restart_and_replay<S: Store>(store: &S) {
    let world = world();
    let id = run(store, RunStatus::Planned, started()).await;
    let mut first = delivery(store, &world, &world.fake);
    let early = first
        .tick_at(now() - TimeDelta::minutes(1))
        .await
        .expect("tick");
    assert!(early.prompts.posted.is_empty(), "not before end + 30 min");
    let report = first.tick_at(now()).await.expect("tick");
    assert_eq!(report.prompts.posted, [format!("{id}:0")]);
    assert_eq!(
        prompts(&world.fake.calls()),
        [vec![
            format!("run:done:{id}:0"),
            format!("run:missed:{id}:0"),
            format!("run:later:{id}:0"),
        ]]
    );
    first.tick_at(now()).await.expect("replayed tick");
    let mut restarted = delivery(store, &world, &world.fake);
    restarted
        .tick_at(now() + TimeDelta::minutes(1))
        .await
        .expect("tick after a restart");
    assert_eq!(prompts(&world.fake.calls()).len(), 1, "posted exactly once");
    assert_eq!(status(store, &id).await, RunStatus::Planned);
}

#[tokio::test]
async fn a_prompt_posts_half_an_hour_after_the_end_once_across_restarts() {
    on_both_stores!(once_across_restart_and_replay);
}

async fn pressed_edits_without_a_notice<S: Store>(store: &S) {
    let world = world();
    let id = run(store, RunStatus::Planned, started()).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.tick_at(now()).await.expect("tick");
    let before = other_posts(&world.fake.calls());
    // What a Done press writes: the status, unannounced, as the presser.
    let mut ids = RandomIds;
    support::service(store, &mut ids, now())
        .as_origin(Origin::new(
            Actor::member("1002"),
            kanade::domain::history::Surface::Discord,
        ))
        .set_status(
            &id,
            StatusChange {
                status: RunStatus::Done,
                announce: false,
                via_portal: false,
            },
            &config().policy.reminders,
        )
        .await
        .expect("done");
    let close = PromptClose {
        outcome: PromptOutcome::Done,
        decided_by: Some("1002".into()),
        at: now(),
        next: None,
    };
    assert!(store.close_run_prompt(id.clone(), 0, close).await.unwrap());
    let report = delivery
        .tick_at(now() + TimeDelta::minutes(1))
        .await
        .expect("tick");
    assert_eq!(report.prompts.settled, [format!("{id}:0")]);
    let edits = edits(&world.fake.calls());
    let [(text, left)] = &edits[..] else {
        panic!("one edit: {edits:?}");
    };
    assert!(text.starts_with("🏁 Done · marked by <@1002>"), "{text}");
    assert!(left.is_empty(), "buttons removed");
    assert_eq!(other_posts(&world.fake.calls()), before, "no change notice");
}

#[tokio::test]
async fn a_pressed_prompt_edits_into_its_outcome_without_a_notice() {
    on_both_stores!(pressed_edits_without_a_notice);
}

async fn not_yet_asks_again<S: Store>(store: &S) {
    let world = world();
    let id = run(store, RunStatus::Planned, started()).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.tick_at(now()).await.expect("tick");
    let first = store.run_prompt(id.clone(), 0).await.unwrap().unwrap();
    let again = now() + TimeDelta::minutes(30);
    let close = PromptClose {
        outcome: PromptOutcome::NotYet,
        decided_by: Some("1001".into()),
        at: now(),
        next: Some(RunPrompt::open(
            id.clone(),
            1,
            first.ends_at,
            again,
            first.cutoff_at,
        )),
    };
    assert!(store.close_run_prompt(id.clone(), 0, close).await.unwrap());
    delivery
        .tick_at(again - TimeDelta::minutes(1))
        .await
        .expect("tick");
    assert_eq!(prompts(&world.fake.calls()).len(), 1, "not yet re-asked");
    let edits = edits(&world.fake.calls());
    assert!(
        edits[0]
            .0
            .starts_with("⏳ Not yet · marked by <@1001> · asking again at 20:30"),
        "{edits:?}"
    );
    let report = delivery.tick_at(again).await.expect("tick");
    assert_eq!(report.prompts.posted, [format!("{id}:1")]);
    assert_eq!(
        prompts(&world.fake.calls())[1][0],
        format!("run:done:{id}:1")
    );
}

#[tokio::test]
async fn not_yet_asks_again_half_an_hour_later() {
    on_both_stores!(not_yet_asks_again);
}

async fn auto_done_at_end_of_day<S: Store + ChangeHistory>(store: &S) {
    let world = world();
    let id = run(store, RunStatus::Planned, started()).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.tick_at(now()).await.expect("tick");
    let report = delivery
        .tick_at(cutoff() - TimeDelta::minutes(1))
        .await
        .expect("tick");
    assert!(report.done.is_empty());
    let head = store.history_head().await.unwrap().seq;
    let report = delivery.tick_at(cutoff()).await.expect("tick");
    assert_eq!(report.done, std::slice::from_ref(&id));
    assert_eq!(status(store, &id).await, RunStatus::Done);
    let record = store.load_change(head + 1).await.unwrap().unwrap();
    assert_eq!(record.origin.actor, Actor::system("delivery"));
    assert_eq!(
        record.origin.request_id,
        Some(format!(
            "run-outcome:{id}:auto-done:{}",
            cutoff().timestamp()
        )),
        "recorded as automatic"
    );
    assert!(record.notices.is_empty());
    let edits = edits(&world.fake.calls());
    assert!(
        edits[0].0.starts_with("🏁 Marked done automatically") && edits[0].1.is_empty(),
        "{edits:?}"
    );
}

#[tokio::test]
async fn an_unanswered_run_is_marked_done_at_the_end_of_the_day() {
    on_both_stores!(auto_done_at_end_of_day);
}

async fn skipped_prompt_done_at_reset<S: Store + ChangeHistory>(store: &S) {
    let world = world();
    // Tue 23:00 local: the prompt would fall on the Wed 00:00 reset.
    let id = run(store, RunStatus::Planned, reset() - TimeDelta::hours(1)).await;
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery
        .tick_at(reset() - TimeDelta::minutes(1))
        .await
        .expect("tick");
    assert!(report.done.is_empty());
    let report = delivery.tick_at(reset()).await.expect("tick");
    assert_eq!(report.done, std::slice::from_ref(&id));
    assert!(prompts(&world.fake.calls()).is_empty(), "never prompted");
    let head = store.history_head().await.unwrap();
    let record = store.load_change(head.seq).await.unwrap().unwrap();
    assert_eq!(record.origin.actor, Actor::system("delivery"));
}

#[tokio::test]
async fn a_prompt_due_at_the_reset_is_skipped_and_the_run_done_at_the_reset() {
    on_both_stores!(skipped_prompt_done_at_reset);
}

async fn own_time_done_at_reset<S: Store>(store: &S) {
    let world = world();
    let id = run(store, RunStatus::Otot, started()).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.tick_at(now()).await.expect("tick");
    let report = delivery.tick_at(cutoff()).await.expect("tick");
    assert!(report.done.is_empty(), "own time waits for the reset");
    let report = delivery.tick_at(reset()).await.expect("tick");
    assert_eq!(report.done, [id]);
    assert!(prompts(&world.fake.calls()).is_empty(), "no prompt");
}

#[tokio::test]
async fn own_time_runs_get_no_prompt_and_are_done_at_the_reset() {
    on_both_stores!(own_time_done_at_reset);
}

async fn terminal_runs_unprompted<S: Store>(store: &S) {
    let world = world();
    run(store, RunStatus::Cancelled, started()).await;
    run(store, RunStatus::Done, started()).await;
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.tick_at(now()).await.expect("tick");
    assert!(report.prompts.opened.is_empty());
    assert!(prompts(&world.fake.calls()).is_empty());
}

#[tokio::test]
async fn cancelled_and_done_runs_get_no_prompt() {
    on_both_stores!(terminal_runs_unprompted);
}

async fn moved_run_replans<S: Store>(store: &S) {
    let world = world();
    let id = run(store, RunStatus::Planned, started()).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.tick_at(now()).await.expect("tick");
    let moved_to = now() + TimeDelta::hours(1);
    let mut ids = RandomIds;
    support::service(store, &mut ids, now())
        .as_origin(Origin::for_tests())
        .amend_run(&id, moved_to, &config().policy)
        .await
        .expect("move");
    let report = delivery
        .tick_at(now() + TimeDelta::minutes(1))
        .await
        .expect("tick");
    assert_eq!(report.prompts.closed, [format!("{id}:0")]);
    assert_eq!(report.prompts.opened, [format!("{id}:1")]);
    let edits = edits(&world.fake.calls());
    assert!(edits[0].0.starts_with("This run moved"), "{edits:?}");
    // New end 21:30 local, prompt due 22:00 local.
    let due = moved_to + TimeDelta::minutes(60);
    delivery
        .tick_at(due - TimeDelta::minutes(1))
        .await
        .expect("tick");
    assert_eq!(prompts(&world.fake.calls()).len(), 1);
    let report = delivery.tick_at(due).await.expect("tick");
    assert_eq!(report.prompts.posted, [format!("{id}:1")]);
}

#[tokio::test]
async fn a_moved_run_re_plans_its_prompt() {
    on_both_stores!(moved_run_replans);
}

/// The tick's first ask of `id`, closed as a press by `by` (the claim landed).
async fn claimed<S: Store>(store: &S, id: &str, outcome: PromptOutcome, by: &str) {
    let close = PromptClose {
        outcome,
        decided_by: Some(by.into()),
        at: now(),
        next: None,
    };
    assert!(store.close_run_prompt(id.into(), 0, close).await.unwrap());
}

async fn lost_settle_is_finished<S: Store + ChangeHistory>(store: &S) {
    let world = world();
    let id = run(store, RunStatus::Planned, started()).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.tick_at(now()).await.expect("tick");
    claimed(store, &id, PromptOutcome::DidntHappen, "1002").await;
    let head = store.history_head().await.unwrap().seq;
    delivery
        .tick_at(now() + TimeDelta::minutes(1))
        .await
        .expect("tick");
    assert_eq!(status(store, &id).await, RunStatus::Cancelled);
    let record = store.load_change(head + 1).await.unwrap().unwrap();
    assert_eq!(record.origin.actor, Actor::member("1002"));
    assert_eq!(
        record.origin.request_id,
        Some(format!("run-outcome:{id}:0:didnt-happen"))
    );
    let edits = edits(&world.fake.calls());
    assert!(
        edits[0].0.starts_with("Didn't happen · marked by <@1002>"),
        "{edits:?}"
    );
}

#[tokio::test]
async fn a_claimed_press_whose_status_was_lost_is_finished_by_the_tick() {
    on_both_stores!(lost_settle_is_finished);
}

async fn lost_close_is_closed<S: Store>(store: &S) {
    let world = world();
    let id = run(store, RunStatus::Planned, started()).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.tick_at(now()).await.expect("tick");
    let mut ids = RandomIds;
    support::service(store, &mut ids, now())
        .as_origin(Origin::for_tests())
        .set_run_status(&id, RunStatus::Done)
        .await
        .expect("done");
    let report = delivery
        .tick_at(now() + TimeDelta::minutes(1))
        .await
        .expect("tick");
    assert_eq!(report.prompts.closed, [format!("{id}:0")]);
    let edits = edits(&world.fake.calls());
    assert_eq!(edits[0].1, Vec::<String>::new(), "buttons removed");
    assert!(edits[0].0.starts_with("🏁 Done"), "{edits:?}");
}

#[tokio::test]
async fn a_run_settled_while_its_close_was_lost_has_its_prompt_closed() {
    on_both_stores!(lost_close_is_closed);
}

async fn cutoff_then_press_guard<S: Store>(store: &S) {
    let world = world();
    let id = run(store, RunStatus::Planned, started()).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.tick_at(now()).await.expect("tick");
    delivery.tick_at(cutoff()).await.expect("cutoff");
    assert_eq!(status(store, &id).await, RunStatus::Done);
    // A Didn't happen press that read the run before the cutoff landed.
    let snapshot = store.load(&Scope::All).await.unwrap();
    let seen = snapshot.runs.iter().find(|run| run.id == id).unwrap();
    let mut ids = RandomIds;
    let settled = support::service(store, &mut ids, cutoff())
        .as_origin(Origin::new(
            Actor::member("1001"),
            kanade::domain::history::Surface::Discord,
        ))
        .settle_run(
            kanade::domain::schedule::SettleRun {
                run_id: id.clone(),
                status: RunStatus::Cancelled,
                datetime: seen.datetime,
                user: "1001".into(),
                staff: false,
            },
            &config().policy.reminders,
        )
        .await
        .expect("settle");
    assert!(!settled, "the cutoff won");
    assert_eq!(status(store, &id).await, RunStatus::Done);
}

#[tokio::test]
async fn a_press_landing_after_the_cutoff_leaves_the_run_done() {
    on_both_stores!(cutoff_then_press_guard);
}

async fn bound_post_is_recovered<S: Store>(store: &S) {
    use kanade::domain::notify::{Claim, EffectKind, IntentContent, NotificationIntent, Receipt};
    let world = world();
    let id = run(store, RunStatus::Planned, started()).await;
    // A previous process posted and bound ask 0, then died before recording it.
    let lease = store
        .begin_lease(support::INSTANCE, "earlier", now())
        .await
        .unwrap();
    let intent = NotificationIntent {
        effect: EffectKind::Notice(kanade::bot::delivery::RUN_PROMPT_EFFECT.into()),
        effect_context: vec![id.clone(), "0".into()],
        channel_id: HOME.into(),
        targets: Vec::new(),
        mentions: Vec::new(),
        content: IntentContent::Plain,
        warnings: Vec::new(),
    };
    let source = format!("run-prompt:{id}:0");
    let Claim::Fresh(attempt) = store
        .claim_source(&lease, &intent, &source, 0, now())
        .await
        .unwrap()
    else {
        panic!("fresh claim");
    };
    let receipt = Receipt {
        channel_id: HOME.into(),
        message_id: "777".into(),
    };
    store
        .bind(&lease, &attempt, &receipt, None, now())
        .await
        .unwrap();
    store.end_lease(&lease, now()).await.unwrap();
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.tick_at(now()).await.expect("tick");
    assert!(report.prompts.posted.is_empty());
    assert!(
        prompts(&world.fake.calls()).is_empty(),
        "never posted again"
    );
    let ask = store.run_prompt(id.clone(), 0).await.unwrap().unwrap();
    assert_eq!(
        (ask.channel_id.as_deref(), ask.message_id.as_deref()),
        (Some(HOME), Some("777"))
    );
}

#[tokio::test]
async fn a_post_bound_before_it_was_recorded_is_recovered_from_the_journal() {
    on_both_stores!(bound_post_is_recovered);
}

async fn reopened_run_is_asked_again<S: Store>(store: &S) {
    let world = world();
    let id = run(store, RunStatus::Planned, started()).await;
    let mut delivery = delivery(store, &world, &world.fake);
    delivery.tick_at(now()).await.expect("tick");
    let mut ids = RandomIds;
    for status in [RunStatus::Cancelled, RunStatus::Planned] {
        support::service(store, &mut ids, now())
            .as_origin(Origin::for_tests())
            .set_run_status(&id, status)
            .await
            .expect("status");
        delivery
            .tick_at(now() + TimeDelta::minutes(1))
            .await
            .expect("tick");
    }
    assert_eq!(prompts(&world.fake.calls()).len(), 2, "asked again");
    assert!(store.run_prompt(id, 1).await.unwrap().unwrap().is_open());
}

#[tokio::test]
async fn a_run_live_again_after_its_prompt_closed_is_asked_again() {
    on_both_stores!(reopened_run_is_asked_again);
}

async fn after_midnight_end<S: Store>(store: &S) {
    let world = world();
    // Thu 23:30 local: ends Fri 00:00, prompt Fri 00:30, cutoff Sat 00:00.
    let start = now() + TimeDelta::minutes(210);
    let id = run(store, RunStatus::Planned, start).await;
    let mut delivery = delivery(store, &world, &world.fake);
    let due = start + TimeDelta::hours(1);
    let report = delivery.tick_at(due).await.expect("tick");
    assert_eq!(report.prompts.posted, [format!("{id}:0")]);
    let report = delivery
        .tick_at(due + TimeDelta::hours(23))
        .await
        .expect("tick");
    assert!(report.done.is_empty(), "Friday is not over");
    let report = delivery
        .tick_at(due + TimeDelta::minutes(23 * 60 + 30))
        .await
        .expect("tick");
    assert_eq!(report.done, [id]);
}

#[tokio::test]
async fn a_run_ending_after_midnight_is_cut_off_at_the_next_midnight() {
    on_both_stores!(after_midnight_end);
}
