//! The R05 delivery matrix (T1–T28): placeholder, parts, retries, deletion,
//! shutdown and panics, over the shared rig's `FakeDiscord`.

use super::*;
use crate::bot::transport::{AmbiguousKind, RejectionKind, SILENT, Step as Discord};
use crate::chat::sanitize::reply_parts;
use twilight_model::channel::message::MessageFlags;

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

/// A reply that posts as `n` parts, and those parts.
fn long_reply(n: usize) -> (&'static str, Vec<String>) {
    let text = (1..=n)
        .map(|i| format!("Paragraph {i}: {}", "word ".repeat(197).trim_end()))
        .collect::<Vec<_>>()
        .join("\n\n");
    let parts = reply_parts(&text);
    assert_eq!(parts.len(), n);
    (leak(text), parts)
}

fn delivery_of(row: &ChatInteraction) -> serde_json::Value {
    row.guardrail["delivery"].clone()
}

fn placeholder() -> String {
    format!("post silent ^1001 {CHANNEL}: {}", staged())
}

fn ambiguous(applied: bool) -> Discord {
    Discord::Ambiguous {
        kind: AmbiguousKind::Timeout,
        applied,
    }
}

async fn one(rig: &Rig) -> ChatInteraction {
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    let rows = rig.rows();
    assert_eq!(rows.len(), 1, "{:?}", rig.effects());
    rows[0].clone()
}

#[tokio::test]
async fn t01_a_one_part_answer_is_edited_into_the_placeholder() {
    let rig = rig(vec![Step::Reply("Lotus is at 9.")]).await;
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [placeholder(), "edit 5001: Lotus is at 9.".to_owned()]
    );
    assert_eq!(rig.fake_discord().count(Op::Create), 1, "no extra create");
    assert_eq!(rig.fake_discord().create_flags(), [SILENT]);
    assert_eq!(row.error, None);
    assert_eq!(row.reply, "Lotus is at 9.");
    assert_eq!(
        delivery_of(&row),
        json!({"placeholder": "edited", "parts": 1, "delivered": 1})
    );
    assert!(row.guardrail.get("context").is_some(), "existing keys kept");
}

#[tokio::test]
async fn t02_continuations_follow_in_order_silently_without_a_reference() {
    let (reply, parts) = long_reply(3);
    let rig = rig(vec![Step::Reply(reply)]).await;
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            format!("edit 5001: {}", parts[0]),
            format!("post silent {CHANNEL}: {}", parts[1]),
            format!("post silent {CHANNEL}: {}", parts[2]),
        ]
    );
    assert_eq!(rig.fake_discord().create_flags(), [SILENT; 3]);
    assert_eq!(
        delivery_of(&row),
        json!({"placeholder": "edited", "parts": 3, "delivered": 3})
    );
    assert_eq!(row.error, None);
}

#[tokio::test]
async fn t03_a_rejected_placeholder_leaves_the_answer_as_todays_reply() {
    let rig = rig(vec![Step::Reply("Lotus is at 9.")]).await;
    rig.fake_discord().script(
        Op::Create,
        Discord::Reject(RejectionKind::MissingPermissions),
    );
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            format!("post ^1001 {CHANNEL}: Lotus is at 9.")
        ]
    );
    assert_eq!(
        rig.fake_discord().create_flags(),
        [SILENT, MessageFlags::empty()]
    );
    assert_eq!(
        delivery_of(&row),
        json!({"placeholder": "create_rejected", "parts": 1, "delivered": 1})
    );
}

#[tokio::test]
async fn t04_an_ambiguous_placeholder_is_never_recreated() {
    let rig = rig(vec![Step::Reply("Lotus is at 9.")]).await;
    rig.fake_discord().script(Op::Create, ambiguous(true));
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            format!("post ^1001 {CHANNEL}: Lotus is at 9.")
        ]
    );
    assert_eq!(rig.fake_discord().messages().len(), 2, "the orphan landed");
    assert_eq!(delivery_of(&row)["placeholder"], "create_ambiguous");
}

#[tokio::test]
async fn t05_an_unsent_placeholder_is_retried_once() {
    let rig = rig(vec![Step::Reply("Lotus is at 9.")]).await;
    rig.fake_discord()
        .script(Op::Create, Discord::Reject(RejectionKind::NotSent));
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            placeholder(),
            "edit 5001: Lotus is at 9.".to_owned()
        ]
    );
    assert_eq!(delivery_of(&row)["placeholder"], "edited");

    let rig = self::rig(vec![Step::Reply("x")]).await;
    let discord = rig.fake_discord();
    discord.script(Op::Create, Discord::Reject(RejectionKind::RateLimited));
    discord.script(Op::Create, Discord::Reject(RejectionKind::NotSent));
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            placeholder(),
            format!("post ^1001 {CHANNEL}: x")
        ],
        "one retry only, then today's reply"
    );
    assert_eq!(delivery_of(&row)["placeholder"], "create_rejected");
}

#[tokio::test]
async fn t06_a_vanished_placeholder_gets_a_new_reply() {
    let rig = rig(vec![Step::Reply("Lotus is at 9.")]).await;
    rig.fake_discord()
        .script(Op::Edit, Discord::Reject(RejectionKind::UnknownMessage));
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            "edit 5001: Lotus is at 9.".to_owned(),
            format!("post ^1001 {CHANNEL}: Lotus is at 9."),
        ]
    );
    assert_eq!(
        delivery_of(&row),
        json!({"placeholder": "deleted", "parts": 1, "delivered": 1})
    );
}

#[tokio::test]
async fn t06b_a_refused_edit_posts_a_new_reply_then_deletes_the_placeholder() {
    let rig = rig(vec![Step::Reply("Lotus is at 9.")]).await;
    rig.fake_discord().script(
        Op::Edit,
        Discord::Reject(RejectionKind::Http {
            status: 400,
            code: Some(50035),
        }),
    );
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            "edit 5001: Lotus is at 9.".to_owned(),
            format!("post ^1001 {CHANNEL}: Lotus is at 9."),
            "delete 5001".to_owned(),
        ]
    );
    assert_eq!(delivery_of(&row)["placeholder"], "deleted");
    assert_eq!(row.error, None);
}

#[tokio::test]
async fn t07_an_ambiguous_edit_is_retried_once_identically() {
    let rig = rig(vec![Step::Reply("Lotus is at 9.")]).await;
    rig.fake_discord().script(Op::Edit, ambiguous(false));
    let row = one(&rig).await;
    let edit = "edit 5001: Lotus is at 9.".to_owned();
    assert_eq!(rig.effects(), [placeholder(), edit.clone(), edit]);
    assert_eq!(
        delivery_of(&row),
        json!({"placeholder": "edited", "parts": 1, "delivered": 1})
    );
}

#[tokio::test]
async fn t08_an_edit_ambiguous_twice_counts_as_landed_and_delivery_continues() {
    let (reply, parts) = long_reply(2);
    let rig = rig(vec![Step::Reply(reply)]).await;
    rig.fake_discord().script(Op::Edit, ambiguous(false));
    rig.fake_discord().script(Op::Edit, ambiguous(true));
    let row = one(&rig).await;
    let edit = format!("edit 5001: {}", parts[0]);
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            edit.clone(),
            edit,
            format!("post silent {CHANNEL}: {}", parts[1]),
        ]
    );
    assert_eq!(
        delivery_of(&row),
        json!({"placeholder": "edited", "parts": 2, "delivered": 2, "unknown": [1]})
    );
    assert_eq!(row.error, None);
}

#[tokio::test]
async fn t09_a_rejected_continuation_ends_with_the_marker_on_the_last_part() {
    let (reply, parts) = long_reply(3);
    let rig = rig(vec![Step::Reply(reply)]).await;
    let discord = rig.fake_discord();
    discord.script(Op::Create, Discord::Succeed);
    discord.script(Op::Create, Discord::Succeed);
    discord.script(
        Op::Create,
        Discord::Reject(RejectionKind::MissingPermissions),
    );
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            format!("edit 5001: {}", parts[0]),
            format!("post silent {CHANNEL}: {}", parts[1]),
            format!("post silent {CHANNEL}: {}", parts[2]),
            format!("edit 5002: {}{INCOMPLETE_MARKER}", parts[1]),
        ]
    );
    assert_eq!(
        row.error.as_deref(),
        Some("incomplete: delivered 2 of 3 parts")
    );
    assert_eq!(
        delivery_of(&row),
        json!({"placeholder": "edited", "parts": 3, "delivered": 2,
               "incomplete": true, "cause": "rejected"})
    );
}

#[tokio::test]
async fn t10_an_ambiguous_continuation_is_never_replayed() {
    let (reply, parts) = long_reply(3);
    let rig = rig(vec![Step::Reply(reply)]).await;
    let discord = rig.fake_discord();
    discord.script(Op::Create, Discord::Succeed);
    discord.script(Op::Create, ambiguous(false));
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            format!("edit 5001: {}", parts[0]),
            format!("post silent {CHANNEL}: {}", parts[1]),
            format!("edit 5001: {}{INCOMPLETE_MARKER}", parts[0]),
        ]
    );
    assert_eq!(
        delivery_of(&row),
        json!({"placeholder": "edited", "parts": 3, "delivered": 1, "unknown": [2],
               "incomplete": true, "cause": "ambiguous"})
    );
    assert_eq!(
        row.error.as_deref(),
        Some("incomplete: delivered 1 of 3 parts")
    );
}

#[tokio::test]
async fn t11_a_marker_that_does_not_fit_is_its_own_silent_message() {
    let heavy = "😀".repeat(995);
    let text = format!("{heavy}\n\nThe rest: {}", "word ".repeat(60).trim_end());
    let parts = reply_parts(&text);
    assert_eq!(parts, [heavy.clone(), parts[1].clone()]);
    let rig = rig(vec![Step::Reply(leak(text))]).await;
    let discord = rig.fake_discord();
    discord.script(Op::Create, Discord::Succeed);
    discord.script(
        Op::Create,
        Discord::Reject(RejectionKind::MissingPermissions),
    );
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            format!("edit 5001: {heavy}"),
            format!("post silent {CHANNEL}: {}", parts[1]),
            format!("post silent {CHANNEL}: *(reply incomplete)*"),
        ]
    );
    assert_eq!(delivery_of(&row)["delivered"], 1);
}

#[tokio::test]
async fn t12_a_model_failure_is_the_failure_line_edited_in() {
    let rig = rig(vec![Step::Reply("")]).await;
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [placeholder(), format!("edit 5001: {FAILURE_REPLY}")]
    );
    assert_eq!(row.reply, FAILURE_REPLY);
    assert_eq!(delivery_of(&row)["placeholder"], "edited");
}

#[tokio::test]
async fn t13_a_failure_without_a_placeholder_is_todays_reply() {
    let rig = rig(vec![Step::Reply("")]).await;
    rig.fake_discord()
        .script(Op::Create, Discord::Reject(RejectionKind::MissingAccess));
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            format!("post ^1001 {CHANNEL}: {FAILURE_REPLY}")
        ]
    );
    assert_eq!(delivery_of(&row)["placeholder"], "create_rejected");
}

#[tokio::test]
async fn t14_a_deleted_waiter_never_gets_a_placeholder() {
    let first = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&first), "a"), Step::Reply("b")]).await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    assert!(rig.driver.offer(asked("1002", "12", CHANNEL, &[ROLE])));
    rig.settle().await;
    rig.driver.deleted(&["1002".into()]);
    first.notify_one();
    rig.settle().await;
    assert!(!rig.effects().iter().any(|e| e.contains("^1002")));
    assert_eq!(rig.rows().len(), 1);
}

#[tokio::test]
async fn t15_a_question_deleted_while_preparing_posts_nothing() {
    let rig = rig(vec![Step::Reply("never")]).await;
    let gate = Arc::new(Notify::new());
    *rig.fake.prepare_gate.lock().unwrap() = Some(Arc::clone(&gate));
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    rig.driver.deleted(&["1001".into()]);
    gate.notify_one();
    rig.settle().await;
    assert!(rig.fake_discord().calls().is_empty());
    assert_eq!(rig.pool_used(), 0);
}

#[tokio::test]
async fn t16_a_deletion_while_staged_withdraws_beside_the_running_answer() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&held), "late answer")]).await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    rig.driver.deleted(&["1001".into()]);
    rig.settle().await;
    assert_eq!(
        rig.effects(),
        [placeholder(), "delete 5001".to_owned()],
        "withdrawn while the answer still runs"
    );
    assert!(rig.rows().is_empty(), "the answer is not cut");
    held.notify_one();
    rig.settle().await;
    assert_eq!(rig.effects().len(), 2);
    let row = &rig.rows()[0];
    assert_eq!(
        row.error.as_deref(),
        Some("cancelled: the question was deleted")
    );
    assert_eq!(
        delivery_of(row),
        json!({"placeholder": "deleted", "parts": 0, "delivered": 0, "cause": "deleted"})
    );
}

#[tokio::test]
async fn t17_a_deletion_during_the_placeholder_create_withdraws_it_once_posted() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&held), "x")]).await;
    let create = rig.fake_discord().hold(Op::Create);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    create.entered().await;
    rig.driver.deleted(&["1001".into()]);
    rig.settle().await;
    assert!(rig.effects().is_empty(), "the create is still in flight");
    create.release();
    rig.settle().await;
    assert_eq!(rig.effects(), [placeholder(), "delete 5001".to_owned()]);
    held.notify_one();
    rig.settle().await;
    assert_eq!(rig.effects().len(), 2);
    assert_eq!(delivery_of(&rig.rows()[0])["placeholder"], "deleted");
}

#[tokio::test]
async fn t18_a_deletion_after_the_answer_but_before_its_edit_withdraws() {
    let rig = rig(vec![Step::Reply("answer")]).await;
    let create = rig.fake_discord().hold(Op::Create);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    create.entered().await;
    rig.settle().await;
    assert_eq!(rig.fake.seen.lock().unwrap().conversations.len(), 1);
    rig.driver.deleted(&["1001".into()]);
    create.release();
    rig.settle().await;
    assert_eq!(rig.effects(), [placeholder(), "delete 5001".to_owned()]);
    let row = &rig.rows()[0];
    assert_eq!(
        row.error.as_deref(),
        Some("cancelled: the question was deleted")
    );
}

#[tokio::test]
async fn t19_a_deletion_during_the_first_edit_keeps_it_and_stops() {
    let (reply, parts) = long_reply(3);
    let rig = rig(vec![Step::Reply(reply)]).await;
    let edit = rig.fake_discord().hold(Op::Edit);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    edit.entered().await;
    rig.driver.deleted(&["1001".into()]);
    edit.release();
    rig.settle().await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            format!("edit 5001: {}", parts[0]),
            format!("edit 5001: {}{INCOMPLETE_MARKER}", parts[0]),
        ]
    );
    let row = &rig.rows()[0];
    assert_eq!(
        row.error.as_deref(),
        Some("cancelled: the question was deleted; incomplete: delivered 1 of 3 parts")
    );
    assert_eq!(delivery_of(row)["cause"], "deleted");
}

#[tokio::test]
async fn t20_a_deletion_after_part_two_of_three_stops_there() {
    let (reply, parts) = long_reply(3);
    let rig = rig(vec![Step::Reply(reply)]).await;
    rig.fake_discord().hold(Op::Create).release();
    let second = rig.fake_discord().hold(Op::Create);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    second.entered().await;
    rig.driver.deleted(&["1001".into()]);
    second.release();
    rig.settle().await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            format!("edit 5001: {}", parts[0]),
            format!("post silent {CHANNEL}: {}", parts[1]),
            format!("edit 5002: {}{INCOMPLETE_MARKER}", parts[1]),
        ]
    );
    let row = &rig.rows()[0];
    assert_eq!(delivery_of(row)["delivered"], 2);
    // Every delivered part is withheld from later context.
    assert!(rig.driver.offer(asked("1002", "12", CHANNEL, &[ROLE])));
    rig.settle().await;
    let state = rig.driver.state();
    for id in ["1001", "5001", "5002"] {
        assert!(state.pilot.conversations.is_withheld(id), "{id}");
    }
}

#[tokio::test]
async fn t21_an_ambiguous_withdrawal_is_retried_once_then_orphaned() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&held), "x")]).await;
    rig.fake_discord().script(Op::Delete, ambiguous(false));
    rig.fake_discord().script(Op::Delete, ambiguous(false));
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    rig.driver.deleted(&["1001".into()]);
    held.notify_one();
    rig.settle().await;
    let delete = "delete 5001".to_owned();
    assert_eq!(rig.effects(), [placeholder(), delete.clone(), delete]);
    assert_eq!(delivery_of(&rig.rows()[0])["placeholder"], "orphaned");
}

#[tokio::test]
async fn t22_a_cut_while_staged_turns_the_placeholder_into_the_failure_line() {
    let config = DriverConfig {
        stop_grace: Duration::from_millis(50),
        ..DriverConfig::default()
    };
    let rig = rig_with(
        Fake::new(vec![Step::Forever]),
        config,
        &MemoryScheduleStore::new(),
    )
    .await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    rig.driver.stop().await;
    assert_eq!(
        rig.effects(),
        [placeholder(), format!("edit 5001: {FAILURE_REPLY}")]
    );
    let row = &rig.rows()[0];
    assert_eq!(row.error.as_deref(), Some("cancelled: serve shut down"));
    assert_eq!(
        delivery_of(row),
        json!({"placeholder": "edited", "parts": 1, "delivered": 1, "cause": "shutdown"})
    );
}

#[tokio::test]
async fn t23_a_continuation_past_the_cut_budget_is_recorded_not_marked() {
    let config = DriverConfig {
        stop_grace: Duration::from_millis(50),
        cut_budget: Duration::from_millis(50),
        ..DriverConfig::default()
    };
    let (reply, parts) = long_reply(3);
    let rig = rig_with(
        Fake::new(vec![Step::Reply(reply)]),
        config,
        &MemoryScheduleStore::new(),
    )
    .await;
    rig.fake_discord().hold(Op::Create).release();
    let _stuck = rig.fake_discord().hold(Op::Create);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    _stuck.entered().await;
    rig.driver.stop().await;
    assert_eq!(
        rig.effects(),
        [placeholder(), format!("edit 5001: {}", parts[0])],
        "no marker after the hard abort"
    );
    let row = &rig.rows()[0];
    assert_eq!(
        row.error.as_deref(),
        Some("incomplete: delivered 1 of 3 parts")
    );
    assert_eq!(
        delivery_of(row),
        json!({"placeholder": "edited", "parts": 3, "delivered": 1, "unknown": [2],
               "incomplete": true, "cause": "shutdown"})
    );
    assert_eq!(row.reply, reply, "anchored on the answer");
}

#[tokio::test]
async fn t24_a_panic_while_staged_edits_the_placeholder_into_the_failure_line() {
    let boom = Arc::new(Notify::new());
    let rig = rig(vec![Step::PanicAfter(Arc::clone(&boom))]).await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    boom.notify_one();
    rig.settle().await;
    assert_eq!(
        rig.effects(),
        [placeholder(), format!("edit 5001: {FAILURE_REPLY}")]
    );
    let row = &rig.rows()[0];
    assert_eq!(
        row.error.as_deref(),
        Some("failed: the question stopped unexpectedly")
    );
    assert_eq!(delivery_of(row)["placeholder"], "edited");
    assert_eq!(rig.driver.limits().clean_retry.pending, 0);
}

#[tokio::test]
async fn t25_a_panic_after_part_one_records_what_was_delivered() {
    let (reply, parts) = long_reply(3);
    let rig = rig(vec![Step::Reply(reply)]).await;
    rig.surface.1.panic_on_post.store(2, Ordering::SeqCst);
    let row = one(&rig).await;
    assert_eq!(
        rig.effects(),
        [placeholder(), format!("edit 5001: {}", parts[0])],
        "no Discord effect after the panic"
    );
    assert_eq!(
        row.error.as_deref(),
        Some("incomplete: delivered 1 of 3 parts"),
        "never `failed: the question stopped unexpectedly` once parts landed"
    );
    assert_eq!(
        delivery_of(&row),
        json!({"placeholder": "edited", "parts": 3, "delivered": 1, "unknown": [2],
               "incomplete": true, "cause": "aborted"})
    );
    assert!(rig.driver.limits().queue.answering.is_empty());
}

#[tokio::test(start_paused = true)]
async fn t26_typing_ticks_every_eight_seconds_until_the_answer_and_failures_are_ignored() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&held), "done")]).await;
    rig.fake_discord().set_default(
        Op::Typing,
        Some(Discord::Reject(RejectionKind::MissingPermissions)),
    );
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert_eq!(
        rig.fake_discord().count(Op::Typing),
        1,
        "at once when staged"
    );
    tokio::time::sleep(Duration::from_secs(17)).await;
    assert_eq!(rig.fake_discord().count(Op::Typing), 3, "at 0, 8 and 16 s");
    held.notify_one();
    rig.settle().await;
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(
        rig.fake_discord().count(Op::Typing),
        3,
        "stopped, never retried"
    );
    assert_eq!(rig.effects(), [placeholder(), "edit 5001: done".to_owned()]);
}

#[tokio::test]
async fn t27_every_create_and_edit_mentions_nobody() {
    let (reply, _) = long_reply(3);
    let rig = rig(vec![Step::Reply(reply)]).await;
    rig.fake_discord().script(Op::Create, Discord::Succeed);
    rig.fake_discord().script(Op::Create, ambiguous(false));
    one(&rig).await;
    // `effects` asserts `mentions::none()` on each create and edit.
    let effects = rig.effects();
    assert_eq!(effects.len(), 4, "{effects:?}");
}

#[tokio::test]
async fn t28_shed_and_limited_questions_never_get_a_placeholder() {
    let config = DriverConfig {
        traffic: TrafficLimits {
            per_channel: 1,
            guild: 10,
            max_wait_s: 120.0,
        },
        ..DriverConfig::default()
    };
    let held = Arc::new(Notify::new());
    let mut fake = Fake::new(vec![Step::Held(Arc::clone(&held), "a"), Step::Reply("b")]);
    fake.member_rate = (1, 300.0);
    let rig = rig_with(fake, config, &MemoryScheduleStore::new()).await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    assert!(rig.driver.offer(asked("1002", "12", CHANNEL, &[ROLE])));
    assert!(rig.driver.offer(asked("1003", "13", CHANNEL, &[ROLE])));
    assert!(rig.driver.offer(asked("1004", "11", OTHER, &[ROLE])));
    rig.settle().await;
    let events = rig.surface.events();
    assert!(events.contains(&format!("+{CHANNEL_BUSY_REACTION} {CHANNEL}/1003")));
    assert!(events.contains(&format!("+{RATE_LIMITED_REACTION} {OTHER}/1004")));
    assert!(events.contains(&format!("+{} {CHANNEL}/1002", position_reaction(1))));
    let effects = rig.effects();
    assert!(
        !effects
            .iter()
            .any(|e| e.contains("^1002") || e.contains("^1003"))
    );
    assert!(
        effects
            .iter()
            .any(|e| e.starts_with(&format!("post ^1004 {OTHER}: That's your 1 answer"))),
        "{effects:?}"
    );
    assert_eq!(effects[0], placeholder());
    held.notify_one();
    rig.settle().await;
    let events = rig.surface.events();
    assert!(events.contains(&format!("-{SEEN_REACTION} {CHANNEL}/1001")));
    assert!(
        rig.effects()
            .contains(&format!("post silent ^1002 {CHANNEL}: {}", staged()))
    );
}

fn quick_stop() -> DriverConfig {
    DriverConfig {
        stop_grace: Duration::from_millis(50),
        cut_budget: Duration::from_millis(50),
        ..DriverConfig::default()
    }
}

/// Wait until `stop` has passed its grace and cut running answers.
async fn cut_sent(rig: &Rig) {
    while !*rig.driver.shared.cut.borrow() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn an_aborted_failure_line_edit_after_a_cut_reads_as_the_cut() {
    let rig = rig_with(
        Fake::new(vec![Step::Forever]),
        quick_stop(),
        &MemoryScheduleStore::new(),
    )
    .await;
    let _stuck = rig.fake_discord().hold(Op::Edit);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    rig.driver.stop().await;
    assert_eq!(rig.effects(), [placeholder()], "the edit never returned");
    let row = &rig.rows()[0];
    assert_eq!(
        row.error.as_deref(),
        Some("cancelled: serve shut down; incomplete: delivered 0 of 1 parts")
    );
    assert_eq!(
        delivery_of(row),
        json!({"placeholder": "orphaned", "parts": 1, "delivered": 0, "unknown": [1],
               "incomplete": true, "cause": "shutdown"})
    );
}

#[tokio::test]
async fn a_deleted_question_aborted_mid_delivery_keeps_its_cancel_string() {
    let (reply, _) = long_reply(3);
    let rig = rig_with(
        Fake::new(vec![Step::Reply(reply)]),
        quick_stop(),
        &MemoryScheduleStore::new(),
    )
    .await;
    rig.fake_discord().hold(Op::Create).release();
    let stuck = rig.fake_discord().hold(Op::Create);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    stuck.entered().await;
    rig.driver.deleted(&["1001".into()]);
    rig.driver.stop().await;
    let row = &rig.rows()[0];
    assert_eq!(
        row.error.as_deref(),
        Some("cancelled: the question was deleted; incomplete: delivered 1 of 3 parts")
    );
    assert_eq!(delivery_of(row)["unknown"], json!([2]));
    assert_eq!(
        delivery_of(row)["cause"],
        "deleted",
        "deletion wins over shutdown"
    );
}

#[tokio::test]
async fn t17b_an_unsent_placeholder_is_not_retried_after_a_deletion() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&held), "x")]).await;
    rig.fake_discord()
        .script(Op::Create, Discord::Reject(RejectionKind::NotSent));
    let create = rig.fake_discord().hold(Op::Create);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    create.entered().await;
    rig.driver.deleted(&["1001".into()]);
    create.release();
    held.notify_one();
    rig.settle().await;
    assert_eq!(rig.effects(), [placeholder()], "one create, never retried");
    let row = &rig.rows()[0];
    assert_eq!(
        row.error.as_deref(),
        Some("cancelled: the question was deleted")
    );
    assert_eq!(delivery_of(row)["placeholder"], "create_rejected");
}

#[tokio::test]
async fn t17c_an_unsent_placeholder_is_not_retried_after_the_cut() {
    let rig = rig_with(
        Fake::new(vec![Step::Forever]),
        quick_stop(),
        &MemoryScheduleStore::new(),
    )
    .await;
    rig.fake_discord()
        .script(Op::Create, Discord::Reject(RejectionKind::NotSent));
    let create = rig.fake_discord().hold(Op::Create);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    create.entered().await;
    let driver = rig.driver.clone();
    let stopping = tokio::spawn(async move { driver.stop().await });
    cut_sent(&rig).await;
    create.release();
    stopping.await.unwrap();
    assert_eq!(rig.effects(), [placeholder()], "one create, never retried");
    let row = &rig.rows()[0];
    assert_eq!(row.error.as_deref(), Some("cancelled: serve shut down"));
    assert_eq!(
        delivery_of(row),
        json!({"placeholder": "create_rejected", "parts": 0, "delivered": 0, "cause": "shutdown"})
    );
}

/// The `Held` guard's own withdrawal: the question is deleted and its task
/// aborted before the worker runs again, so the delivery loop never sees the
/// deletion and the placeholder is still live and idle when the guard drops.
#[tokio::test]
async fn an_abort_after_a_deletion_withdraws_the_placeholder_from_the_guard() {
    let rig = rig(vec![Step::Forever]).await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert_eq!(rig.effects(), [placeholder()]);
    let running: Vec<_> = std::mem::take(&mut *rig.driver.tasks());
    // No await between: the worker is not polled after the deletion.
    rig.driver.deleted(&["1001".into()]);
    for task in &running {
        task.abort();
    }
    for task in running {
        let _ = task.await;
    }
    rig.settle().await;
    assert_eq!(
        rig.effects(),
        [placeholder(), "delete 5001".to_owned()],
        "exactly one delete, issued by the guard (not a failure-line edit)"
    );
    assert!(
        rig.fake
            .observed
            .lock()
            .unwrap()
            .contains(&"cancelled aborted".to_owned()),
        "concluded by the guard"
    );
    let row = &rig.rows()[0];
    assert_eq!(
        row.error.as_deref(),
        Some("cancelled: the question was deleted")
    );
    assert_eq!(
        delivery_of(row),
        json!({"placeholder": "deleted", "parts": 0, "delivered": 0, "cause": "deleted"})
    );
}

#[tokio::test]
async fn an_ambiguous_then_refused_edit_counts_as_landed() {
    let (reply, parts) = long_reply(2);
    let rig = rig(vec![Step::Reply(reply)]).await;
    rig.fake_discord().script(Op::Edit, ambiguous(false));
    rig.fake_discord().script(
        Op::Edit,
        Discord::Reject(RejectionKind::Http {
            status: 400,
            code: None,
        }),
    );
    let row = one(&rig).await;
    let edit = format!("edit 5001: {}", parts[0]);
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            edit.clone(),
            edit,
            format!("post silent {CHANNEL}: {}", parts[1]),
        ],
        "I1: never deleted, no new reply"
    );
    assert_eq!(
        delivery_of(&row),
        json!({"placeholder": "edited", "parts": 2, "delivered": 2, "unknown": [1]})
    );
}

#[tokio::test]
async fn a_deletion_after_a_refused_first_edit_withdraws_without_a_marker() {
    let (reply, parts) = long_reply(2);
    let rig = rig(vec![Step::Reply(reply)]).await;
    rig.fake_discord()
        .script(Op::Edit, Discord::Reject(RejectionKind::MissingPermissions));
    let edit = rig.fake_discord().hold(Op::Edit);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    edit.entered().await;
    rig.driver.deleted(&["1001".into()]);
    edit.release();
    rig.settle().await;
    assert_eq!(
        rig.effects(),
        [
            placeholder(),
            format!("edit 5001: {}", parts[0]),
            "delete 5001".to_owned(),
        ]
    );
    let row = &rig.rows()[0];
    assert_eq!(
        row.error.as_deref(),
        Some("cancelled: the question was deleted")
    );
    assert_eq!(
        delivery_of(row),
        json!({"placeholder": "deleted", "parts": 2, "delivered": 0, "cause": "deleted"})
    );
}
