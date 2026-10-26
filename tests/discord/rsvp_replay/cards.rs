use kanade::bot::delivery::AdminAlert;
use kanade::bot::transport::{Call, DiscordTransport, Op, Outcome, RejectionKind, Step};
use kanade_domain::schedule::RsvpState;
use twilight_model::channel::message::ReactionType;
use twilight_model::id::Id;

use super::support::*;

/// `MEMBER` reacts ✅ and ❌ (on one card, or across two cards of the same
/// run) while `OTHER` answers ✅ on a card grouped with a second run.
async fn assert_conflict_skips_only_member(cross_card: bool) {
    let world = World::new().await;
    let first = world.planned(&[MEMBER, OTHER], 24).await;
    let second = world.planned(&[OTHER], 25).await;
    world.card(&first, 400).await;
    if cross_card {
        world.card(&first, 401).await;
    }
    world.store.test_map_card_run("400", &second).await;
    world.react(400, "✅", &[MEMBER, OTHER]);
    world.react(if cross_card { 401 } else { 400 }, "❌", &[MEMBER]);
    let report = world.replay_once().await;
    assert_eq!((report.conflicts, report.applied), (1, 2));
    for run in [first, second] {
        let answers = world.snapshot(&run).await.rsvps;
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].user_id, OTHER.to_string());
        assert_eq!(answers[0].state, RsvpState::Yes);
    }
}

/// A run whose 400 card was moved away from, with fresh card 401.
async fn moved_run(world: &World, member_answered_first: bool) -> String {
    let run = world.planned(&[MEMBER], 24).await;
    world.card(&run, 400).await;
    if member_answered_first {
        world.member_reacts_yes(&run).await;
    }
    world.move_run(&run).await;
    world.card(&run, 401).await;
    run
}

#[tokio::test]
async fn rows_6_7_a_conflict_on_one_card_or_across_cards_skips_only_that_user() {
    for cross_card in [false, true] {
        assert_conflict_skips_only_member(cross_card).await;
    }
}

#[tokio::test]
async fn rows_11_to_13_card_failures_and_two_page_cap_skip_without_guessing() {
    for step in [
        Some(Step::Reject(RejectionKind::MissingAccess)),
        Some(crate::support::timed_out(false)),
        None,
    ] {
        let world = World::new().await;
        let (run, message) = world.carded_run().await;
        if let Some(step) = step {
            world.fake.script(Op::ReactionUsers, step);
        } else {
            world.fake.seed_reactions(
                message,
                "✅",
                ReactionType::Normal,
                (1..=201).map(Id::new).collect(),
            );
        }
        assert_eq!(world.replay_once().await.applied, 0);
        assert!(world.snapshot(&run).await.rsvps.is_empty());
    }
}

#[tokio::test]
async fn rows_11_to_13_incomplete_reads_skip_bad_runs_continue_and_throttle_permissions() {
    let world = World::new().await;
    let mut runs = Vec::new();
    for index in 0..7 {
        let run = world.planned(&[MEMBER], 24 + index).await;
        world.card(&run, 400 + index as u64).await;
        runs.push(run);
    }
    world
        .fake
        .delete_message(Id::new(CHANNEL), Id::new(400))
        .await;
    for step in [
        Step::Succeed,
        Step::Reject(RejectionKind::MissingAccess),
        Step::Reject(RejectionKind::MissingPermissions),
        crate::support::timed_out(false),
    ] {
        world.fake.script(Op::ReactionUsers, step);
    }
    world.fake.script_reaction_page(
        Id::new(404),
        "✅",
        ReactionType::Normal,
        (1..=100).map(Id::new).collect(),
    );
    world
        .fake
        .script_reaction_page(Id::new(404), "✅", ReactionType::Normal, vec![Id::new(100)]);
    world.fake.seed_reactions(
        Id::new(405),
        "✅",
        ReactionType::Normal,
        (1..=201).map(Id::new).collect(),
    );
    world.react(406, "✅", &[MEMBER]);

    let report = world.replay_once().await;
    assert_eq!(report.messages, 7);
    assert_eq!(
        report.applied, 1,
        "the final run proceeds after every bad read"
    );
    for run in &runs[..6] {
        assert!(world.snapshot(run).await.rsvps.is_empty());
    }
    assert_eq!(
        world.snapshot(&runs[6]).await.rsvps[0].state,
        RsvpState::Yes
    );
    assert_eq!(
        world.alerts.alerts(),
        vec![AdminAlert::RsvpReplayPermissionDenied { messages: 2 }]
    );
    let calls = world.fake.calls();
    let rejected = |id: u64, kind: RejectionKind| {
        calls.iter().any(|call| {
            matches!(call,
                Call::ReactionUsers { message, outcome: Outcome::DefinitelyRejected(seen), .. }
                if message.get() == id && *seen == kind
            )
        })
    };
    assert!(rejected(400, RejectionKind::UnknownMessage));
    assert!(rejected(401, RejectionKind::MissingAccess));
    assert!(rejected(402, RejectionKind::MissingPermissions));
    assert!(calls.iter().any(|call| matches!(call,
        Call::ReactionUsers { message, after: Some(after), .. }
        if message.get() == 404 && after.get() == 100
    )));
    assert!(calls.iter().any(|call| matches!(call,
        Call::ReactionUsers { message, outcome: Outcome::Ambiguous(_), .. }
        if message.get() == 403
    )));
    assert_eq!(
        calls
            .iter()
            .filter(
                |call| matches!(call, Call::ReactionUsers { message, .. } if message.get() == 405)
            )
            .count(),
        2,
        "RSVP reads stop after the two-page cap"
    );
}

#[tokio::test]
async fn row_15_grouped_card_reads_once_and_applies_each_run() {
    let world = World::new().await;
    let first = world.planned(&[MEMBER], 24).await;
    let second = world.planned(&[MEMBER], 25).await;
    world.card(&first, 400).await;
    world.store.test_map_card_run("400", &second).await;
    world.react(400, "✅", &[MEMBER]);
    let observed = world.observe_run_writes();
    let report = world.replay_once().await;
    assert_eq!((report.messages, report.applied), (1, 2));
    assert_eq!(
        world.fake.count(Op::ReactionUsers),
        4,
        "cached grouped snapshot"
    );
    assert_eq!(world.snapshot(&first).await.rsvps[0].state, RsvpState::Yes);
    assert_eq!(world.snapshot(&second).await.rsvps[0].state, RsvpState::Yes);
    let refreshed = observed.lock().unwrap().clone();
    assert_eq!(refreshed.len(), 2);
    assert_eq!(refreshed.iter().filter(|id| **id == first).count(), 1);
    assert_eq!(refreshed.iter().filter(|id| **id == second).count(), 1);
}

#[tokio::test]
async fn row_15_row_16_grouped_card_does_not_apply_a_nonparticipant() {
    let world = World::new().await;
    let participating = world.planned(&[MEMBER], 24).await;
    let swapped_off = world.planned(&[OTHER], 25).await;
    world.card(&participating, 400).await;
    world.store.test_map_card_run("400", &swapped_off).await;
    world.react(400, "✅", &[MEMBER]);
    let report = world.replay_once().await;
    assert_eq!((report.messages, report.applied), (1, 1));
    assert_eq!(
        world.snapshot(&participating).await.rsvps[0].state,
        RsvpState::Yes
    );
    assert!(
        world.snapshot(&swapped_off).await.rsvps.is_empty(),
        "a reactor absent from this run's participant list is ignored"
    );
}

/// Row 17: a participant who lost the role still applies; row 18: so does
/// one who left the guild. Neither is looked up.
#[tokio::test]
async fn rows_17_18_a_participant_is_not_filtered_for_role_loss_or_departure() {
    for departed in [false, true] {
        let world = World::new().await;
        let (run, message) = world.carded_run().await;
        if departed {
            world.fake.seed_members(Id::new(999), Vec::new());
        }
        world.react(message.get(), "✅", &[MEMBER]);
        assert_eq!(world.replay_once().await.applied, 1, "departed {departed}");
        let answer = &world.snapshot(&run).await.rsvps[0];
        assert_eq!(answer.user_id, MEMBER.to_string());
        assert_eq!(answer.state, RsvpState::Yes);
        assert_eq!(world.fake.count(Op::ListMembers), 0);
    }
}

#[tokio::test]
async fn rows_21_21b_21c_moved_old_cards_are_guard_only_not_add_evidence() {
    let world = World::new().await;
    let run = moved_run(&world, false).await;
    world.member_reacts_yes(&run).await;
    world.react(400, "✅", &[MEMBER, SELF]);
    world.react(401, "✅", &[SELF]);
    assert_eq!(world.replay_once().await.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);

    let world = World::new().await;
    moved_run(&world, false).await;
    world
        .fake
        .delete_message(Id::new(CHANNEL), Id::new(400))
        .await;
    world.react(401, "✅", &[MEMBER]);
    assert_eq!(world.replay_once().await.applied, 1);
}

#[tokio::test]
async fn row_21_moved_old_card_only_add_is_not_evidence() {
    let world = World::new().await;
    let run = moved_run(&world, false).await;
    world.react(400, "✅", &[MEMBER, SELF]);
    world.react(401, "✅", &[SELF]);
    assert_eq!(world.replay_once().await.applied, 0);
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn row_21c_deleted_moved_card_blocks_guarded_removal() {
    let world = World::new().await;
    let run = moved_run(&world, true).await;
    world
        .fake
        .delete_message(Id::new(CHANNEL), Id::new(400))
        .await;
    world.react(401, "✅", &[SELF]);
    assert_eq!(world.replay_once().await.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}

#[tokio::test]
async fn row_22b_retired_but_present_card_blocks_a_guarded_clear() {
    let world = World::new().await;
    let run = world.planned(&[MEMBER], 24).await;
    let (_lease, attempt) = world.card_attempt(&run, 400).await;
    world.store.test_retire_bound_attempt(&attempt).await;
    world.card(&run, 401).await;
    world.member_reacts_yes(&run).await;
    world.react(400, "✅", &[MEMBER, SELF]);
    world.react(401, "✅", &[SELF]);
    assert_eq!(world.replay_once().await.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}
