//! The redesigned proposal card as Components V2: posted with Apply /
//! Reject buttons instead of seeded ✅/❌, pressed through the same approval
//! path as a reaction (directly and through the dispatcher), closed to one
//! disabled button, never replayed from reactions, and kept V2 for good.

use std::sync::atomic::AtomicBool;

use kanade::bot::cards::{CLOSED_GREY, CardPress, Pressed};
use kanade::bot::commands::{
    AccessPolicy, CardPresses, Dispatcher, Disposition, INACTIVE, NOT_YOURS,
};
use kanade::bot::delivery::cards::DifficultyMarks;
use kanade::bot::transport::COMPONENTS_V2;
use kanade::domain::ids::tag;
use twilight_model::channel::message::Component;

use super::styled::{redesigned, styled_desk};
use super::*;
use crate::support::{self, BOSSING_ROLE, button_interaction, v2_accent, v2_buttons, v2_texts};

const PROPOSAL_PINK: u32 = 0xEB459E;
const SETTLED_GREEN: u32 = 0x3BA55C;
const HARD: &str = "<:diff_h:2000000000000000001>";

/// A card posted redesigned by `desk`: the proposal and message ids.
async fn v2_card(world: &World, desk: &Desk) -> (String, String) {
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    assert_eq!(
        desk.post_card(&card(vec![entry], Vec::new())).await,
        PostResult::Posted
    );
    let message = world.message_of(&id).await.expect("posted");
    (id, message)
}

fn edits(world: &World) -> Vec<MessageEdit> {
    world
        .discord
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Edit { edit, .. } => Some(edit),
            _ => None,
        })
        .collect()
}

/// The last edit, which must be a V2 one.
fn last_v2_edit(world: &World) -> Vec<Component> {
    edits(world)
        .pop()
        .and_then(|edit| edit.components)
        .expect("a V2 edit")
}

fn press(message: &str, proposal: &str, user: &str, answer: RsvpAnswer) -> CardPress {
    CardPress {
        message_id: message.into(),
        proposal_id: proposal.into(),
        user_id: user.into(),
        answer,
    }
}

fn open_buttons(id: &str) -> Vec<support::ButtonView> {
    vec![
        ("Apply".into(), format!("card:apply:{id}"), false),
        ("Reject".into(), format!("card:reject:{id}"), false),
    ]
}

#[tokio::test]
async fn a_redesigned_card_is_posted_as_v2_with_buttons_instead_of_reactions() {
    let world = World::new().await;
    let desk = styled_desk(
        &world,
        &redesigned(),
        DifficultyMarks::new().with("h", HARD),
    );
    let (id, _) = v2_card(&world, &desk).await;
    let Some(Call::Create { message, .. }) = world.creates().pop() else {
        panic!("a post");
    };
    assert_eq!(world.discord.create_flags(), [COMPONENTS_V2]);
    assert_eq!(
        (message.content.as_deref(), message.embeds.len()),
        (None, 0)
    );
    assert!(
        message.allowed_mentions.users.is_empty(),
        "nobody is pinged"
    );
    assert_eq!(v2_accent(&message.components), Some(PROPOSAL_PINK));
    let when = format!("**<t:{}:F>**", local(9, 2, 21, 30).timestamp());
    assert_eq!(
        v2_texts(&message.components),
        [
            format!(
                "### 📋 Move {HARD} Radiant Malefic Star + {HARD} The First Adversary?\n\
                 moving to wed"
            ),
            format!("**From** ~~Mon 31 Aug 21:30~~\n**To** {when}\n**Party** Mylene, Alvin"),
            format!("-# {} · 90% sure · /amend to edit", tag(&world.run)),
        ]
    );
    assert_eq!(v2_buttons(&message.components), open_buttons(&id));
    assert_eq!(
        world.discord.count(Op::AddReaction),
        0,
        "buttons replace the seeded ✅/❌"
    );
}

#[tokio::test]
async fn apply_answers_like_a_tick_and_leaves_one_disabled_button() {
    let world = World::new().await;
    let desk = styled_desk(&world, &redesigned(), DifficultyMarks::new());
    let (id, message) = v2_card(&world, &desk).await;
    let pressed = desk
        .on_press(&press(&message, &id, MY, RsvpAnswer::Yes))
        .await;
    assert!(
        matches!(
            &pressed,
            Pressed::Answered(CardReaction::Approved { approved, problems })
                if approved.len() == 1 && problems.is_empty()
        ),
        "{pressed:?}"
    );
    assert_eq!(world.status(&id).await, DraftStatus::Merged);
    let layout = last_v2_edit(&world);
    assert_eq!(v2_accent(&layout), Some(SETTLED_GREEN));
    let texts = v2_texts(&layout);
    assert!(
        texts[0].ends_with("\n-# ✅ Applied by Mylene at 13:07"),
        "{texts:?}"
    );
    assert_eq!(
        v2_buttons(&layout),
        [("Applied by Mylene".into(), "card:closed".into(), true)]
    );
    assert_eq!(
        texts.last().map(String::as_str),
        Some(format!("-# {} · applied", tag(&world.run)).as_str())
    );
}

#[tokio::test]
async fn reject_answers_like_a_cross_and_greys_the_card() {
    let world = World::new().await;
    let desk = styled_desk(&world, &redesigned(), DifficultyMarks::new());
    let (id, message) = v2_card(&world, &desk).await;
    assert_eq!(
        desk.on_press(&press(&message, &id, ADMIN, RsvpAnswer::No))
            .await,
        Pressed::Answered(CardReaction::Rejected {
            proposal_ids: vec![id.clone()]
        })
    );
    assert_eq!(world.status(&id).await, DraftStatus::Rejected);
    let layout = last_v2_edit(&world);
    assert_eq!(v2_accent(&layout), Some(CLOSED_GREY));
    assert!(v2_texts(&layout)[0].starts_with("### 📋 ~~Move"));
    assert_eq!(
        v2_buttons(&layout),
        [("Rejected by Boss".into(), "card:closed".into(), true)]
    );
}

#[tokio::test]
async fn a_press_by_someone_who_may_not_answer_changes_nothing() {
    let world = World::new().await;
    let desk = styled_desk(&world, &redesigned(), DifficultyMarks::new());
    let (id, message) = v2_card(&world, &desk).await;
    for answer in [RsvpAnswer::Yes, RsvpAnswer::No] {
        assert_eq!(
            desk.on_press(&press(&message, &id, STRANGER, answer)).await,
            Pressed::NotYours
        );
    }
    assert_eq!(world.status(&id).await, DraftStatus::Submitted);
    assert!(edits(&world).is_empty(), "the card is unchanged");
    assert_eq!(world.creates().len(), 1, "no notice either");
}

#[tokio::test]
async fn a_button_naming_no_proposal_on_the_message_is_inactive() {
    let world = World::new().await;
    let desk = styled_desk(&world, &redesigned(), DifficultyMarks::new());
    let (id, message) = v2_card(&world, &desk).await;
    assert_eq!(
        desk.on_press(&press(&message, "elsewhere", MY, RsvpAnswer::Yes))
            .await,
        Pressed::Inactive
    );
    assert_eq!(
        desk.on_press(&press("999999", &id, MY, RsvpAnswer::Yes))
            .await,
        Pressed::Inactive
    );
    assert_eq!(world.status(&id).await, DraftStatus::Submitted);
    assert!(edits(&world).is_empty());
}

/// A dispatcher whose card presses go straight to `desk`.
fn dispatcher(desk: Desk) -> Dispatcher {
    let desk = Arc::new(desk);
    let presses: CardPresses = Arc::new(move |press| {
        let desk = Arc::clone(&desk);
        Box::pin(async move { Some(desk.on_press(&press).await) })
    });
    Dispatcher::new(AccessPolicy {
        bossing_role_id: support::role(BOSSING_ROLE),
        admin_role_id: None,
        debug_user_ids: Vec::new(),
    })
    .with_card_presses(presses)
}

#[tokio::test]
async fn presses_through_the_dispatcher_ack_at_once_and_answer_only_the_presser() {
    let world = World::new().await;
    let desk = styled_desk(&world, &redesigned(), DifficultyMarks::new());
    let (id, message) = v2_card(&world, &desk).await;
    let dispatcher = dispatcher(desk);
    let message_u64: u64 = message.parse().unwrap();
    let channel: u64 = CHANNEL.parse().unwrap();
    let handle = |interaction| {
        let dispatcher = &dispatcher;
        let discord = Arc::clone(&world.discord);
        async move {
            dispatcher
                .handle(
                    discord.as_ref(),
                    support::guild(),
                    &interaction,
                    Some(support::user(support::OWNER)),
                )
                .await
                .expect("a guild button press")
        }
    };
    let ephemeral = |text: &str| InteractionReply::ephemeral(text);

    // A stranger: acknowledged, told ephemerally, nothing changes.
    let stranger = button_interaction(
        8001,
        STRANGER.parse().unwrap(),
        &[],
        channel,
        message_u64,
        &format!("card:apply:{id}"),
    );
    assert_eq!(
        handle(stranger).await,
        (Disposition::UserError, Outcome::Delivered(()))
    );
    assert_eq!(world.status(&id).await, DraftStatus::Submitted);
    assert!(matches!(
        world.discord.calls().as_slice(),
        [.., Call::DeferUpdate { .. }, Call::Followup { reply, .. }] if *reply == ephemeral(NOT_YOURS)
    ));

    // A participant's Apply: acknowledged, applied, the card edited, no
    // follow-up; a redelivery is not answered twice.
    let apply = button_interaction(
        8002,
        MY.parse().unwrap(),
        &[BOSSING_ROLE],
        channel,
        message_u64,
        &format!("card:apply:{id}"),
    );
    assert_eq!(
        handle(apply.clone()).await,
        (Disposition::Ran, Outcome::Delivered(()))
    );
    assert_eq!(world.status(&id).await, DraftStatus::Merged);
    let calls = world.discord.calls();
    assert!(
        matches!(
            calls.as_slice(),
            [.., Call::DeferUpdate { .. }, Call::Edit { .. }]
        ),
        "{calls:?}"
    );
    assert_eq!(
        v2_buttons(&last_v2_edit(&world)),
        [("Applied by Mylene".into(), "card:closed".into(), true)]
    );
    assert_eq!(handle(apply).await.0, Disposition::Duplicate);
    assert_eq!(world.discord.calls().len(), calls.len());

    // An old button of the now-closed card (another client had not
    // redrawn), a proposal that is not on this message, and a custom id the
    // bot never built: no longer active, and nothing changes.
    for (interaction, custom_id) in [
        (8003, format!("card:reject:{id}")),
        (8004, "card:reject:gone-proposal".to_owned()),
    ] {
        let stale = button_interaction(
            interaction,
            MY.parse().unwrap(),
            &[BOSSING_ROLE],
            channel,
            message_u64,
            &custom_id,
        );
        assert_eq!(
            handle(stale).await,
            (Disposition::Unknown, Outcome::Delivered(())),
            "{custom_id}"
        );
        assert!(matches!(
            world.discord.calls().as_slice(),
            [.., Call::DeferUpdate { .. }, Call::Followup { reply, .. }]
                if *reply == ephemeral(INACTIVE)
        ));
    }
    let forged = button_interaction(
        8005,
        MY.parse().unwrap(),
        &[BOSSING_ROLE],
        channel,
        message_u64,
        "card:apply:not an id",
    );
    assert_eq!(
        handle(forged).await,
        (Disposition::Unknown, Outcome::Delivered(()))
    );
    assert!(matches!(
        world.discord.calls().as_slice(),
        [.., Call::Respond { reply, .. }] if *reply == ephemeral(INACTIVE)
    ));
    assert_eq!(world.status(&id).await, DraftStatus::Merged);
}

#[tokio::test]
async fn reaction_replay_skips_v2_cards() {
    use twilight_model::channel::message::ReactionType;

    let world = World::new().await;
    let desk = styled_desk(&world, &redesigned(), DifficultyMarks::new());
    let (id, message) = v2_card(&world, &desk).await;
    // Someone reacts ✅ by hand while the bot is away.
    world.discord.seed_reactions(
        Id::new(message.parse().unwrap()),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MY.parse().unwrap())],
    );
    let me = Id::new(1003);
    assert_eq!(desk.replay_reactions(me, || true).await.approved, 0);
    assert_eq!(world.discord.count(Op::Flags), 0, "this process posted it");

    // A restarted desk does not know the card: it reads its flags once.
    let restarted = styled_desk(&world, &redesigned(), DifficultyMarks::new());
    assert_eq!(restarted.replay_reactions(me, || true).await.approved, 0);
    assert_eq!(world.discord.count(Op::Flags), 1);
    assert_eq!(world.status(&id).await, DraftStatus::Submitted);
    assert!(edits(&world).is_empty());
}

#[tokio::test]
async fn a_v2_card_stays_v2_after_the_style_flips_back() {
    let world = World::new().await;
    let style = Arc::new(AtomicBool::new(true));
    let desk = styled_desk(&world, &style, DifficultyMarks::new());
    let (id, message) = v2_card(&world, &desk).await;
    style.store(false, Ordering::SeqCst);
    assert!(desk.refresh(&message).await);
    let layout = last_v2_edit(&world);
    assert_eq!(v2_buttons(&layout), open_buttons(&id));
    assert!(world.discord.is_v2(Id::new(message.parse().unwrap())));
}

#[tokio::test]
async fn a_restarted_desk_learns_a_v2_card_from_the_refused_legacy_edit() {
    let world = World::new().await;
    let desk = styled_desk(&world, &redesigned(), DifficultyMarks::new());
    let (id, message) = v2_card(&world, &desk).await;
    let restarted = styled_desk(&world, &redesigned(), DifficultyMarks::new());
    assert!(restarted.refresh(&message).await);
    let calls: Vec<Call> = world
        .discord
        .calls()
        .into_iter()
        .filter(|call| matches!(call.op(), Op::Edit | Op::Flags))
        .collect();
    let [
        Call::Edit {
            edit: legacy,
            outcome: refused,
            ..
        },
        Call::Flags { .. },
        Call::Edit {
            edit: v2,
            outcome: Outcome::Delivered(()),
            ..
        },
    ] = calls.as_slice()
    else {
        panic!("a refused legacy edit, one flags read, a V2 edit: {calls:?}");
    };
    assert_eq!(legacy.components, None);
    assert!(matches!(
        refused,
        Outcome::DefinitelyRejected(RejectionKind::Http { status: 400, .. })
    ));
    assert_eq!(
        v2_buttons(v2.components.as_deref().expect("V2")),
        open_buttons(&id)
    );

    // From then on it knows: the next refresh goes straight to V2.
    assert!(restarted.refresh(&message).await);
    assert_eq!(world.discord.count(Op::Flags), 1);
    assert!(edits(&world).pop().unwrap().components.is_some());
}
