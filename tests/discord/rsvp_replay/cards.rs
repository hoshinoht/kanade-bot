use std::sync::{Arc, Mutex};

use kanade::bot::delivery::AdminAlert;
use kanade::bot::transport::{DiscordTransport, Op, Step};
use kanade::domain::history::{Actor, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::schedule::{RsvpState, RunStatus};
use kanade::domain::scheduler::SchedulerService;
use twilight_model::channel::message::ReactionType;
use twilight_model::id::Id;

use super::support::*;

#[tokio::test]
async fn row_6_same_card_conflict_skips_only_conflicting_user_on_grouped_runs() {
    let world = World::new();
    let first = world
        .run(
            &[MEMBER, OTHER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    let second = world
        .run(
            &[OTHER],
            now() + chrono::TimeDelta::hours(25),
            RunStatus::Planned,
        )
        .await;
    world.card(&first, 400).await;
    world.store.test_map_card_run("400", &second);
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER), Id::new(OTHER)],
    );
    world.fake.seed_reactions(
        Id::new(400),
        "❌",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!((report.conflicts, report.applied), (1, 2));
    for run in [first, second] {
        let answers = world.snapshot(&run).await.rsvps;
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].user_id, OTHER.to_string());
        assert_eq!(answers[0].state, RsvpState::Yes);
    }
}

#[tokio::test]
async fn row_7_cross_card_conflict_skips_user_but_grouped_sibling_applies() {
    let world = World::new();
    let first = world
        .run(
            &[MEMBER, OTHER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    let second = world
        .run(
            &[OTHER],
            now() + chrono::TimeDelta::hours(25),
            RunStatus::Planned,
        )
        .await;
    world.card(&first, 400).await;
    world.card(&first, 401).await;
    world.store.test_map_card_run("400", &second);
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER), Id::new(OTHER)],
    );
    world.fake.seed_reactions(
        Id::new(401),
        "❌",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!((report.conflicts, report.applied), (1, 2));
    for run in [first, second] {
        let answers = world.snapshot(&run).await.rsvps;
        assert_eq!(answers.len(), 1);
        assert_eq!(answers[0].user_id, OTHER.to_string());
        assert_eq!(answers[0].state, RsvpState::Yes);
    }
}

#[tokio::test]
async fn rows_11_to_13_card_failures_and_two_page_cap_skip_without_guessing() {
    for step in [
        Some(Step::Reject(
            kanade::bot::transport::RejectionKind::MissingAccess,
        )),
        Some(Step::Ambiguous {
            kind: kanade::bot::transport::AmbiguousKind::Timeout,
            applied: false,
        }),
        None,
    ] {
        let world = World::new();
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
        let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
        assert_eq!(report.applied, 0);
        assert!(world.snapshot(&run).await.rsvps.is_empty());
    }
}

#[tokio::test]
async fn rows_11_to_13_incomplete_reads_skip_bad_runs_continue_and_throttle_permissions() {
    let world = World::new();
    let mut runs = Vec::new();
    for index in 0..7 {
        let run = world
            .run(
                &[MEMBER],
                now() + chrono::TimeDelta::hours(24 + index),
                RunStatus::Planned,
            )
            .await;
        world.card(&run, 400 + index as u64).await;
        runs.push(run);
    }
    world
        .fake
        .delete_message(Id::new(CHANNEL), Id::new(400))
        .await;
    world.fake.script(Op::ReactionUsers, Step::Succeed);
    world.fake.script(
        Op::ReactionUsers,
        Step::Reject(kanade::bot::transport::RejectionKind::MissingAccess),
    );
    world.fake.script(
        Op::ReactionUsers,
        Step::Reject(kanade::bot::transport::RejectionKind::MissingPermissions),
    );
    world.fake.script(
        Op::ReactionUsers,
        Step::Ambiguous {
            kind: kanade::bot::transport::AmbiguousKind::Timeout,
            applied: false,
        },
    );
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
    world.fake.seed_reactions(
        Id::new(406),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );

    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
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
    assert!(calls.iter().any(|call| matches!(call,
        kanade::bot::transport::Call::ReactionUsers { message, outcome: kanade::bot::transport::Outcome::DefinitelyRejected(kanade::bot::transport::RejectionKind::UnknownMessage), .. }
        if message.get() == 400
    )));
    assert!(calls.iter().any(|call| matches!(call,
        kanade::bot::transport::Call::ReactionUsers { message, after: Some(after), .. }
        if message.get() == 404 && after.get() == 100
    )));
    assert!(calls.iter().any(|call| matches!(call,
        kanade::bot::transport::Call::ReactionUsers { message, outcome: kanade::bot::transport::Outcome::DefinitelyRejected(kanade::bot::transport::RejectionKind::MissingAccess), .. }
        if message.get() == 401
    )));
    assert!(calls.iter().any(|call| matches!(call,
        kanade::bot::transport::Call::ReactionUsers { message, outcome: kanade::bot::transport::Outcome::DefinitelyRejected(kanade::bot::transport::RejectionKind::MissingPermissions), .. }
        if message.get() == 402
    )));
    assert!(calls.iter().any(|call| matches!(call,
        kanade::bot::transport::Call::ReactionUsers { message, outcome: kanade::bot::transport::Outcome::Ambiguous(_), .. }
        if message.get() == 403
    )));
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, kanade::bot::transport::Call::ReactionUsers { message, .. } if message.get() == 405))
            .count(),
        2,
        "RSVP reads stop after the two-page cap"
    );
}

#[tokio::test]
async fn deleted_old_card_does_not_create_an_answer() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world.fake.delete_message(Id::new(CHANNEL), message).await;
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn row_15_grouped_card_reads_once_and_applies_each_run() {
    let world = World::new();
    let first = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            kanade::domain::schedule::RunStatus::Planned,
        )
        .await;
    let second = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(25),
            kanade::domain::schedule::RunStatus::Planned,
        )
        .await;
    world.card(&first, 400).await;
    world.store.test_map_card_run("400", &second);
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    let observed = Arc::new(Mutex::new(Vec::new()));
    let capture = Arc::clone(&observed);
    assert!(world.store.observe_run_writes(Arc::new(move |runs| {
        capture.lock().unwrap().extend_from_slice(runs)
    })));
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!((report.messages, report.applied), (1, 2));
    assert_eq!(
        world.fake.count(Op::ReactionUsers),
        4,
        "cached grouped snapshot"
    );
    assert_eq!(
        world.snapshot(&first).await.rsvps[0].state,
        kanade::domain::schedule::RsvpState::Yes
    );
    assert_eq!(
        world.snapshot(&second).await.rsvps[0].state,
        kanade::domain::schedule::RsvpState::Yes
    );
    let refreshed = observed.lock().unwrap().clone();
    assert_eq!(refreshed.len(), 2);
    assert_eq!(refreshed.iter().filter(|id| **id == first).count(), 1);
    assert_eq!(refreshed.iter().filter(|id| **id == second).count(), 1);
}

#[tokio::test]
async fn row_15_row_16_grouped_card_does_not_apply_a_nonparticipant() {
    let world = World::new();
    let participating = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    let swapped_off = world
        .run(
            &[OTHER],
            now() + chrono::TimeDelta::hours(25),
            RunStatus::Planned,
        )
        .await;
    world.card(&participating, 400).await;
    world.store.test_map_card_run("400", &swapped_off);
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
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

#[tokio::test]
async fn row_17_participating_member_is_not_filtered_for_role_loss() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 1);
    let answer = &world.snapshot(&run).await.rsvps[0];
    assert_eq!(answer.user_id, MEMBER.to_string());
    assert_eq!(answer.state, RsvpState::Yes);
    assert_eq!(world.fake.count(Op::ListMembers), 0);
}

#[tokio::test]
async fn row_18_departed_member_who_remains_a_participant_still_applies() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world.fake.seed_members(Id::new(999), Vec::new());
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 1);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
    assert_eq!(world.fake.count(Op::ListMembers), 0);
}

#[tokio::test]
async fn rows_21_21b_21c_moved_old_cards_are_guard_only_not_add_evidence() {
    let world = World::new();
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    world.card(&run, 400).await;
    world.clock.set(now() + chrono::TimeDelta::minutes(1));
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(world.clock.get()),
    )
    .as_origin(Origin::new(Actor::admin("move"), Surface::AdminPortal))
    .amend_run(&run, now() + chrono::TimeDelta::hours(30), &policy())
    .await
    .unwrap();
    world.card(&run, 401).await;
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now()),
    )
    .as_origin(Origin::new(
        Actor::member(MEMBER.to_string()),
        Surface::Discord,
    ))
    .apply_reaction(&run, &MEMBER.to_string(), "✅", true)
    .await
    .unwrap();
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER), Id::new(SELF)],
    );
    world.fake.seed_reactions(
        Id::new(401),
        "✅",
        ReactionType::Normal,
        vec![Id::new(SELF)],
    );
    assert_eq!(
        world
            .replay()
            .replay(Id::new(SELF), || true, &mut ())
            .await
            .applied,
        0
    );
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
    let world = World::new();
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    world.card(&run, 400).await;
    world.clock.set(now() + chrono::TimeDelta::minutes(1));
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(world.clock.get()),
    )
    .as_origin(Origin::new(Actor::admin("move"), Surface::AdminPortal))
    .amend_run(&run, now() + chrono::TimeDelta::hours(30), &policy())
    .await
    .unwrap();
    world.card(&run, 401).await;
    world
        .fake
        .delete_message(Id::new(CHANNEL), Id::new(400))
        .await;
    world.fake.seed_reactions(
        Id::new(401),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    assert_eq!(
        world
            .replay()
            .replay(Id::new(SELF), || true, &mut ())
            .await
            .applied,
        1
    );
}

#[tokio::test]
async fn row_21_moved_old_card_only_add_is_not_evidence() {
    let world = World::new();
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    world.card(&run, 400).await;
    world.clock.set(now() + chrono::TimeDelta::minutes(1));
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(world.clock.get()),
    )
    .as_origin(Origin::new(Actor::admin("move"), Surface::AdminPortal))
    .amend_run(&run, now() + chrono::TimeDelta::hours(30), &policy())
    .await
    .unwrap();
    world.card(&run, 401).await;
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER), Id::new(SELF)],
    );
    world.fake.seed_reactions(
        Id::new(401),
        "✅",
        ReactionType::Normal,
        vec![Id::new(SELF)],
    );
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn row_21c_deleted_moved_card_blocks_guarded_removal() {
    let world = World::new();
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    world.card(&run, 400).await;
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now()),
    )
    .as_origin(Origin::new(
        Actor::member(MEMBER.to_string()),
        Surface::Discord,
    ))
    .apply_reaction(&run, &MEMBER.to_string(), "✅", true)
    .await
    .unwrap();
    world.clock.set(now() + chrono::TimeDelta::minutes(1));
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(world.clock.get()),
    )
    .as_origin(Origin::new(Actor::admin("move"), Surface::AdminPortal))
    .amend_run(&run, now() + chrono::TimeDelta::hours(30), &policy())
    .await
    .unwrap();
    world.card(&run, 401).await;
    world
        .fake
        .delete_message(Id::new(CHANNEL), Id::new(400))
        .await;
    world.fake.seed_reactions(
        Id::new(401),
        "✅",
        ReactionType::Normal,
        vec![Id::new(SELF)],
    );
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}

#[tokio::test]
async fn row_22b_retired_but_present_card_blocks_a_guarded_clear() {
    let world = World::new();
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    let (_lease, attempt) = world.card_attempt(&run, 400).await;
    world.store.test_retire_bound_attempt(&attempt);
    world.card(&run, 401).await;
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now()),
    )
    .as_origin(Origin::new(
        Actor::member(MEMBER.to_string()),
        Surface::Discord,
    ))
    .apply_reaction(&run, &MEMBER.to_string(), "✅", true)
    .await
    .unwrap();
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER), Id::new(SELF)],
    );
    world.fake.seed_reactions(
        Id::new(401),
        "✅",
        ReactionType::Normal,
        vec![Id::new(SELF)],
    );
    assert_eq!(
        world
            .replay()
            .replay(Id::new(SELF), || true, &mut ())
            .await
            .applied,
        0
    );
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}
