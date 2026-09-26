//! The fake transport's scripted outcomes and remote-state model.

use twilight_model::id::Id;

use kanade::bot::mentions;
use kanade::bot::transport::{
    AmbiguousKind, Call, DiscordTransport, FakeDiscord, HistoryPage, MessageEdit, Op, Outcome,
    OutgoingMessage, Presence, RejectionKind, Step,
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
