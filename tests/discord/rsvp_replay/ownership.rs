use kanade::discord::transport::DiscordTransport;
use kanade::extract::pipeline::{ChatAnswer, Outbox};
use kanade_domain::history::{Actor, ChangeHistory, Origin, Surface};
use kanade_domain::schedule::{RsvpSource, RsvpState};
use twilight_model::id::Id;

use super::support::*;

/// A card-outbox Yes for `MEMBER`, then a stale ❌ on the card that replay must
/// not apply. `fail_second_write` fails the outbox's second commit.
async fn assert_card_outbox_answer(fail_second_write: bool) {
    let world = World::new().await;
    let (run, message) = world.carded_run().await;
    let head = world.store.history_head().await.unwrap().seq;
    if fail_second_write {
        world.store.fail_commit_after(1);
    }
    let outbox = world.card_outbox();
    Outbox::answers(
        &outbox,
        vec![ChatAnswer {
            channel_id: CHANNEL.to_string(),
            run_id: run.clone(),
            user_ids: vec![MEMBER.to_string()],
            state: RsvpState::Yes,
        }],
    )
    .await;
    let (writes, source) = if fail_second_write {
        (1, RsvpSource::Reaction)
    } else {
        (2, RsvpSource::Chat)
    };
    assert_eq!(world.store.history_head().await.unwrap().seq, head + writes);
    let first = world.store.load_change(head + 1).await.unwrap().unwrap();
    assert!(
        first
            .origin
            .request_id
            .unwrap()
            .starts_with("extract-answer:")
    );
    assert_eq!(world.snapshot(&run).await.rsvps[0].source, source);
    world.react(message.get(), "❌", &[MEMBER]);
    assert_eq!(world.replay_once().await.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}

/// The live answer survives a replay pass that sees only the bot's ✅.
async fn assert_guarded_yes_kept(world: &World, run: &str) {
    assert_eq!(world.replay_once().await.applied, 0);
    assert_eq!(world.snapshot(run).await.rsvps[0].state, RsvpState::Yes);
}

/// Without the bot's own ✅ (never added, or gone with a moderator's
/// remove-all, row 14) a live answer cannot be cleared.
#[tokio::test]
async fn row_3_guard_g_c_requires_the_bot_reaction_for_removal() {
    for own in [false, true] {
        let world = World::new().await;
        let (run, message) = world.carded_run().await;
        world.member_reacts_yes(&run).await;
        if own {
            world.react(message.get(), "✅", &[SELF]);
        }
        world.replay_once().await;
        assert_eq!(
            world.snapshot(&run).await.rsvps.is_empty(),
            own,
            "own={own}"
        );
    }
}

#[tokio::test]
async fn row_8a_portal_answer_after_reaction_wins_over_stale_card() {
    let world = World::new().await;
    let (run, message) = world.carded_run().await;
    world.member_reacts_yes(&run).await;
    world.portal_answer(&run, Some(RsvpState::No)).await;
    world.react(message.get(), "✅", &[MEMBER]);
    assert_eq!(world.replay_once().await.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::No);
}

#[tokio::test]
async fn row_8b_slash_rsvp_cannot_be_removed_by_card_absence() {
    let world = World::new().await;
    let (run, message) = world.carded_run().await;
    world.member_reacts_yes(&run).await;
    world
        .service_at(now())
        .as_origin(
            Origin::new(Actor::member(MEMBER.to_string()), Surface::Discord)
                .with_request_id("discord:123"),
        )
        .set_rsvp(&run, &MEMBER.to_string(), RsvpState::No, RsvpSource::Chat)
        .await
        .unwrap();
    world.react(message.get(), "❌", &[SELF]);
    assert_eq!(world.replay_once().await.applied, 0);
    let answer = &world.snapshot(&run).await.rsvps[0];
    assert_eq!(answer.state, RsvpState::No);
    assert_eq!(answer.source, RsvpSource::Chat);
}

#[tokio::test]
async fn rows_8c_8d_card_outbox_answers_keep_their_source_and_extract_request() {
    // 8c: both writes succeed and source chat wins; 8d: the second write
    // fails and the extract request stays local.
    for fail_second_write in [false, true] {
        assert_card_outbox_answer(fail_second_write).await;
    }
}

#[tokio::test]
async fn guard_g_a_nonreaction_answer_is_not_removed_when_remote_is_absent() {
    let world = World::new().await;
    let (run, message) = world.carded_run().await;
    world.portal_answer(&run, Some(RsvpState::No)).await;
    world.react(message.get(), "❌", &[SELF]);
    assert_eq!(world.replay_once().await.applied, 0);
    let answer = &world.snapshot(&run).await.rsvps[0];
    assert_eq!(answer.state, RsvpState::No);
    assert_eq!(answer.source, RsvpSource::Chat);
}

/// Guard G-b: an unreadable mapped card prevents removal; row 22: a card
/// mapping cleared by `/debug clear_test` is still read for that safety.
#[tokio::test]
async fn guard_g_b_unreadable_mapped_card_prevents_removal_even_once_cleared() {
    use kanade_domain::notify::DebugCardStore;

    for cleared in [false, true] {
        let world = World::new().await;
        let (run, fresh) = world.carded_run().await;
        world.card(&run, 401).await;
        if cleared {
            world.store.clear_debug_card("401", now()).await.unwrap();
        }
        world.member_reacts_yes(&run).await;
        world.react(fresh.get(), "✅", &[SELF]);
        world
            .fake
            .delete_message(Id::new(CHANNEL), Id::new(401))
            .await;
        assert_guarded_yes_kept(&world, &run).await;
    }
}

#[tokio::test]
async fn row_22_unbound_reminder_mapping_is_still_guarded() {
    let world = World::new().await;
    let (run, fresh) = world.carded_run().await;
    world.unbound_card_mapping(&run, 401).await;
    world.member_reacts_yes(&run).await;
    world.react(fresh.get(), "✅", &[SELF]);
    assert_guarded_yes_kept(&world, &run).await;
}
