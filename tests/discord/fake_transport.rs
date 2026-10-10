//! The fake transport's scripted outcomes and remote-state model.

use std::sync::Arc;

use twilight_model::channel::message::MessageFlags;
use twilight_model::id::Id;

use kanade::bot::delivery::cards::redesign::{container, text};
use kanade::bot::mentions;
use kanade::bot::transport::{
    AmbiguousKind, COMPONENTS_V2, Call, DiscordTransport, FakeDiscord, HistoryPage, InteractionRef,
    InteractionReply, MessageEdit, Op, Outcome, OutgoingMessage, Presence, RejectionKind, SILENT,
    Step,
};

use super::support::{
    ALICE, BOB, CHANNEL, GUILD, OWNER, TEXT, channel_json, member_json, message_json, parse,
    parse_channel, parse_message, user_json,
};

fn message(text: &str) -> OutgoingMessage {
    OutgoingMessage {
        content: Some(text.to_owned()),
        embeds: Vec::new(),
        allowed_mentions: mentions::allow_users(&["1001"]),
        reply_to: None,
        attachments: Vec::new(),
        components: Vec::new(),
    }
}

#[tokio::test]
async fn creates_get_sequential_ids_and_are_recorded() {
    let fake = FakeDiscord::new();
    let channel = Id::new(CHANNEL);
    let first = fake.create_message(channel, &message("a")).await;
    let second = fake.create_message(channel, &message("b")).await;
    let (Outcome::Delivered(first), Outcome::Delivered(second)) = (first, second) else {
        panic!("unscripted creates succeed");
    };
    assert_eq!(second.get(), first.get() + 1);
    assert_eq!(fake.messages(), vec![(channel, first), (channel, second)]);
    let calls = fake.calls();
    assert_eq!(calls.len(), 2);
    let Call::Create { message, .. } = &calls[0] else {
        panic!("create recorded");
    };
    assert_eq!(message.allowed_mentions, mentions::allow_users(&["1001"]));
}

#[tokio::test]
async fn ambiguous_create_may_have_posted_without_an_id() {
    let fake = FakeDiscord::new();
    let channel = Id::new(CHANNEL);
    fake.script(
        Op::Create,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: true,
        },
    );
    fake.script(
        Op::Create,
        Step::Ambiguous {
            kind: AmbiguousKind::Connection,
            applied: false,
        },
    );
    assert_eq!(
        fake.create_message(channel, &message("a")).await,
        Outcome::Ambiguous(AmbiguousKind::Timeout)
    );
    assert_eq!(fake.messages().len(), 1, "the ambiguous post landed");
    assert_eq!(
        fake.create_message(channel, &message("b")).await,
        Outcome::Ambiguous(AmbiguousKind::Connection)
    );
    assert_eq!(fake.messages().len(), 1, "this one did not");
}

#[tokio::test]
async fn definite_rejection_posts_nothing() {
    let fake = FakeDiscord::new();
    fake.script(Op::Create, Step::Reject(RejectionKind::MissingPermissions));
    assert_eq!(
        fake.create_message(Id::new(CHANNEL), &message("a")).await,
        Outcome::DefinitelyRejected(RejectionKind::MissingPermissions)
    );
    assert!(fake.messages().is_empty());
}

#[tokio::test]
async fn edits_deletes_and_presence_follow_remote_state() {
    let fake = FakeDiscord::new();
    let channel = Id::new(CHANNEL);
    let Outcome::Delivered(id) = fake.create_message(channel, &message("a")).await else {
        panic!("created");
    };
    let edit = MessageEdit {
        content: Some("b".into()),
        embeds: None,
        allowed_mentions: mentions::none(),
        components: None,
    };
    assert_eq!(
        fake.edit_message(channel, id, &edit).await,
        Outcome::Delivered(())
    );
    assert_eq!(
        fake.add_own_reaction(channel, id, "✅").await,
        Outcome::Delivered(())
    );
    assert_eq!(
        fake.message_presence(channel, id).await,
        Outcome::Delivered(Presence::Present)
    );
    assert_eq!(
        fake.delete_message(channel, id).await,
        Outcome::Delivered(())
    );
    assert_eq!(
        fake.message_presence(channel, id).await,
        Outcome::Delivered(Presence::Absent)
    );
    assert_eq!(
        fake.delete_message(channel, id).await,
        Outcome::DefinitelyRejected(RejectionKind::UnknownMessage)
    );
    assert_eq!(
        fake.edit_message(channel, id, &edit).await,
        Outcome::DefinitelyRejected(RejectionKind::UnknownMessage)
    );
}

fn layout(line: &str) -> Vec<twilight_model::channel::message::Component> {
    vec![container(0x4D5C9E, vec![text(line)])]
}

#[tokio::test]
async fn components_v2_posts_carry_the_flag_and_nothing_else() {
    let fake = FakeDiscord::new();
    let channel = Id::new(CHANNEL);
    let post = OutgoingMessage::v2(layout("hi"), mentions::none());
    let Outcome::Delivered(id) = fake.create_message(channel, &post).await else {
        panic!("created");
    };
    assert_eq!(fake.create_flags(), [COMPONENTS_V2], "the flag is recorded");
    assert!(fake.is_v2(id));
    let Some(Call::Create { message: sent, .. }) = fake.calls().pop() else {
        panic!("a create");
    };
    assert_eq!(sent.components, layout("hi"));
    assert_eq!((sent.content, sent.embeds.len()), (None, 0));
    assert_eq!(
        fake.message_flags(channel, id).await,
        Outcome::Delivered(COMPONENTS_V2)
    );

    // Content or embeds beside a V2 layout are refused unsent (no step
    // used), and so is the flag without components.
    fake.script(Op::Create, Step::Reject(RejectionKind::MissingAccess));
    let mixed = OutgoingMessage {
        content: Some("also text".into()),
        ..post.clone()
    };
    assert_eq!(
        fake.create_message(channel, &mixed).await,
        Outcome::DefinitelyRejected(RejectionKind::Invalid)
    );
    assert_eq!(
        fake.create_flagged_message(channel, &message("plain"), COMPONENTS_V2)
            .await,
        Outcome::DefinitelyRejected(RejectionKind::Invalid)
    );
    assert_eq!(
        fake.create_message(channel, &message("next")).await,
        Outcome::DefinitelyRejected(RejectionKind::MissingAccess),
        "the scripted step was still waiting"
    );
}

#[tokio::test]
async fn a_v2_edit_converts_and_a_legacy_edit_cannot_undo_it() {
    let fake = FakeDiscord::new();
    let channel = Id::new(CHANNEL);
    let Outcome::Delivered(id) = fake.create_message(channel, &message("a")).await else {
        panic!("created");
    };
    assert_eq!(
        fake.message_flags(channel, id).await,
        Outcome::Delivered(MessageFlags::empty())
    );
    let legacy = MessageEdit {
        content: Some("b".into()),
        embeds: None,
        allowed_mentions: mentions::none(),
        components: None,
    };
    let mixed = MessageEdit {
        content: Some("b".into()),
        ..MessageEdit::v2(layout("b"), mentions::none())
    };
    assert_eq!(
        fake.edit_message(channel, id, &mixed).await,
        Outcome::DefinitelyRejected(RejectionKind::Invalid),
        "content beside a V2 layout is refused unsent"
    );
    assert!(!fake.is_v2(id));
    assert_eq!(
        fake.edit_message(channel, id, &MessageEdit::v2(layout("b"), mentions::none()))
            .await,
        Outcome::Delivered(())
    );
    assert!(fake.is_v2(id), "converted");
    assert_eq!(
        fake.edit_message(channel, id, &legacy).await,
        Outcome::DefinitelyRejected(RejectionKind::Http {
            status: 400,
            code: Some(50035)
        }),
        "Discord cannot take the flag off"
    );
    assert_eq!(
        fake.message_flags(channel, Id::new(99)).await,
        Outcome::DefinitelyRejected(RejectionKind::UnknownMessage)
    );
}

#[tokio::test]
async fn v2_replies_and_deferred_updates_are_recorded() {
    let fake = FakeDiscord::new();
    let interaction = InteractionRef::new(Id::new(7), "token".into());
    let reply = InteractionReply::v2(layout("runs"), true);
    assert_eq!(
        fake.respond(&interaction, &reply).await,
        Outcome::Delivered(())
    );
    assert_eq!(
        reply.flags(),
        MessageFlags::EPHEMERAL | COMPONENTS_V2,
        "ephemeral and V2"
    );
    let mixed = InteractionReply {
        content: "and text".into(),
        ..reply.clone()
    };
    for outcome in [
        fake.respond(&interaction, &mixed).await,
        fake.followup(&interaction, &mixed).await,
        fake.complete_deferred(&interaction, &mixed).await,
    ] {
        assert_eq!(outcome, Outcome::DefinitelyRejected(RejectionKind::Invalid));
    }
    assert_eq!(
        fake.defer_update(&interaction).await,
        Outcome::Delivered(())
    );
    assert_eq!(fake.count(Op::DeferUpdate), 1);
    assert!(matches!(
        fake.calls().first(),
        Some(Call::Respond { reply: sent, .. }) if *sent == reply
    ));
}

#[tokio::test]
async fn ambiguous_delete_that_landed_is_confirmed_by_presence() {
    let fake = FakeDiscord::new();
    let channel = Id::new(CHANNEL);
    let message_id = Id::new(42);
    fake.seed_message(channel, message_id);
    fake.script(
        Op::Delete,
        Step::Ambiguous {
            kind: AmbiguousKind::ServerError { status: 502 },
            applied: true,
        },
    );
    assert_eq!(
        fake.delete_message(channel, message_id).await,
        Outcome::Ambiguous(AmbiguousKind::ServerError { status: 502 })
    );
    assert_eq!(
        fake.message_presence(channel, message_id).await,
        Outcome::Delivered(Presence::Absent)
    );
}

#[tokio::test]
async fn replies_are_recorded_with_their_target() {
    let fake = FakeDiscord::new();
    let mut reply = message("ok");
    reply.reply_to = Some(Id::new(55));
    assert!(
        fake.create_message(Id::new(CHANNEL), &reply)
            .await
            .is_delivered()
    );
    let Call::Create { message, .. } = &fake.calls()[0] else {
        panic!("create recorded");
    };
    assert_eq!(message.reply_to, Some(Id::new(55)));
}

#[tokio::test]
async fn members_page_by_user_id_like_discord() {
    let fake = FakeDiscord::new();
    let member = |id: u64| parse(member_json(user_json(id, "m", None, false), None, &[]));
    fake.seed_members(
        Id::new(GUILD),
        vec![member(OWNER), member(ALICE), member(BOB)],
    );
    let ids = |outcome: Outcome<Vec<twilight_model::guild::Member>>| match outcome {
        Outcome::Delivered(page) => page.iter().map(|m| m.user.id.get()).collect::<Vec<_>>(),
        other => panic!("unexpected {other:?}"),
    };
    let guild = Id::new(GUILD);
    assert_eq!(
        ids(fake.list_members(guild, None, 2).await),
        vec![ALICE, BOB]
    );
    assert_eq!(
        ids(fake.list_members(guild, Some(Id::new(BOB)), 2).await),
        vec![OWNER]
    );
    assert_eq!(
        ids(fake.list_members(Id::new(1), None, 2).await),
        Vec::<u64>::new()
    );
    assert_eq!(
        fake.list_members(guild, None, 0).await,
        Outcome::DefinitelyRejected(RejectionKind::Invalid)
    );
    fake.script(Op::ListMembers, Step::Reject(RejectionKind::MissingAccess));
    assert_eq!(
        fake.list_members(guild, None, 2).await,
        Outcome::DefinitelyRejected(RejectionKind::MissingAccess)
    );
    assert_eq!(fake.count(Op::ListMembers), 5);
}

#[tokio::test]
async fn history_pages_come_back_newest_first() {
    let fake = FakeDiscord::new();
    fake.seed_history(
        (1..=5)
            .map(|id| parse_message(message_json(id, CHANNEL, Some(GUILD), "m")))
            .collect(),
    );
    let channel = Id::new(CHANNEL);
    let ids = |outcome: Outcome<Vec<twilight_model::channel::Message>>| match outcome {
        Outcome::Delivered(page) => page.iter().map(|m| m.id.get()).collect::<Vec<_>>(),
        other => panic!("unexpected {other:?}"),
    };
    assert_eq!(
        ids(fake.channel_messages(channel, HistoryPage::Latest, 2).await),
        vec![5, 4]
    );
    assert_eq!(
        ids(fake
            .channel_messages(channel, HistoryPage::Before(Id::new(4)), 2)
            .await),
        vec![3, 2]
    );
    assert_eq!(
        ids(fake
            .channel_messages(channel, HistoryPage::After(Id::new(1)), 2)
            .await),
        vec![3, 2],
        "the oldest messages after the cursor"
    );
    assert_eq!(
        fake.channel_messages(channel, HistoryPage::Latest, 101)
            .await,
        Outcome::DefinitelyRejected(RejectionKind::Invalid)
    );
    fake.script(
        Op::ChannelMessages,
        Step::Ambiguous {
            kind: AmbiguousKind::ServerError { status: 500 },
            applied: false,
        },
    );
    assert_eq!(
        fake.channel_messages(channel, HistoryPage::Latest, 2).await,
        Outcome::Ambiguous(AmbiguousKind::ServerError { status: 500 })
    );
}

#[tokio::test]
async fn guild_channels_serve_the_seeded_list() {
    let fake = FakeDiscord::new();
    let channel = parse_channel(channel_json(CHANNEL, TEXT, "kalos", None, &[]));
    fake.seed_channels(Id::new(GUILD), vec![channel.clone()]);
    assert_eq!(
        fake.guild_channels(Id::new(GUILD)).await,
        Outcome::Delivered(vec![channel])
    );
    fake.set_default(
        Op::GuildChannels,
        Some(Step::Reject(RejectionKind::RateLimited)),
    );
    assert_eq!(
        fake.guild_channels(Id::new(GUILD)).await,
        Outcome::DefinitelyRejected(RejectionKind::RateLimited)
    );
    assert!(matches!(
        fake.calls().last(),
        Some(Call::GuildChannels { .. })
    ));
}

#[tokio::test]
async fn registration_is_recorded_per_guild() {
    let fake = FakeDiscord::new();
    assert!(
        fake.register_guild_commands(Id::new(GUILD), &[])
            .await
            .is_delivered()
    );
    assert!(matches!(
        fake.calls()[..],
        [Call::Register { guild, .. }] if guild == Id::new(GUILD)
    ));
}

#[tokio::test]
async fn flags_are_recorded_per_create_and_bad_flags_refused_unsent() {
    let fake = FakeDiscord::new();
    let channel = Id::new(CHANNEL);
    fake.script(Op::Create, Step::Reject(RejectionKind::MissingAccess));
    assert_eq!(
        fake.create_flagged_message(channel, &message("x"), SILENT | MessageFlags::EPHEMERAL)
            .await,
        Outcome::DefinitelyRejected(RejectionKind::Invalid)
    );
    assert_eq!(
        fake.create_flagged_message(channel, &message("a"), SILENT)
            .await,
        Outcome::DefinitelyRejected(RejectionKind::MissingAccess),
        "the refused call consumed no scripted step"
    );
    assert!(
        fake.create_flagged_message(channel, &message("b"), SILENT)
            .await
            .is_delivered()
    );
    assert!(
        fake.create_message(channel, &message("c"))
            .await
            .is_delivered()
    );
    assert_eq!(
        fake.create_flags(),
        vec![
            SILENT | MessageFlags::EPHEMERAL,
            SILENT,
            SILENT,
            MessageFlags::empty()
        ]
    );
    assert_eq!(fake.count(Op::Create), 4);
    assert_eq!(fake.messages().len(), 2);
}

#[tokio::test]
async fn typing_is_recorded_and_scriptable() {
    let fake = FakeDiscord::new();
    let channel = Id::new(CHANNEL);
    fake.script(Op::Typing, Step::Reject(RejectionKind::MissingPermissions));
    assert_eq!(
        fake.trigger_typing(channel).await,
        Outcome::DefinitelyRejected(RejectionKind::MissingPermissions)
    );
    assert_eq!(fake.trigger_typing(channel).await, Outcome::Delivered(()));
    assert_eq!(fake.count(Op::Typing), 2);
    assert!(matches!(
        fake.calls()[..],
        [Call::Typing { channel: a, .. }, Call::Typing { channel: b, .. }]
            if a == channel && b == channel
    ));
    assert!(fake.messages().is_empty(), "typing posts nothing");
}

#[tokio::test]
async fn a_held_call_parks_before_its_effect_until_released() {
    let fake = Arc::new(FakeDiscord::new());
    let channel = Id::new(CHANNEL);
    let hold = fake.hold(Op::Create);
    fake.script(
        Op::Create,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: true,
        },
    );
    let task = tokio::spawn({
        let fake = Arc::clone(&fake);
        async move {
            fake.create_flagged_message(channel, &message("held"), SILENT)
                .await
        }
    });
    hold.entered().await;
    assert!(fake.calls().is_empty(), "nothing recorded while parked");
    assert!(fake.messages().is_empty(), "no effect while parked");
    // Other operations, and later creates, are not held.
    assert!(fake.trigger_typing(channel).await.is_delivered());
    hold.release();
    assert_eq!(
        task.await.unwrap(),
        Outcome::Ambiguous(AmbiguousKind::Timeout),
        "the scripted step applies after release"
    );
    assert_eq!(fake.messages().len(), 1);
    assert_eq!(fake.create_flags(), vec![SILENT]);
    assert!(
        fake.create_message(channel, &message("free"))
            .await
            .is_delivered()
    );
}

#[tokio::test]
async fn a_hold_released_early_lets_the_call_through_and_a_cancelled_one_does_nothing() {
    let fake = FakeDiscord::new();
    let channel = Id::new(CHANNEL);
    fake.hold(Op::Typing).release();
    assert!(fake.trigger_typing(channel).await.is_delivered());

    let _never_released = fake.hold(Op::Edit);
    fake.seed_message(channel, Id::new(77));
    let edit = MessageEdit {
        content: Some("later".into()),
        embeds: None,
        allowed_mentions: mentions::none(),
        components: None,
    };
    let parked = tokio::time::timeout(
        std::time::Duration::from_millis(20),
        fake.edit_message(channel, Id::new(77), &edit),
    )
    .await;
    assert!(parked.is_err(), "still parked");
    assert_eq!(
        fake.count(Op::Edit),
        0,
        "a dropped parked call records nothing"
    );
    assert!(
        fake.edit_message(channel, Id::new(77), &edit)
            .await
            .is_delivered(),
        "the hold was consumed by the cancelled call"
    );
}
