//! The fake transport's scripted outcomes and remote-state model.

use twilight_model::id::Id;

use kanade::bot::mentions;
use kanade::bot::transport::{
    AmbiguousKind, Call, DiscordTransport, FakeDiscord, MessageEdit, Op, Outcome, OutgoingMessage,
    Presence, RejectionKind, Step,
};

use super::support::CHANNEL;

fn message(text: &str) -> OutgoingMessage {
    OutgoingMessage {
        content: Some(text.to_owned()),
        embeds: Vec::new(),
        allowed_mentions: mentions::allow_users(&["1001"]),
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
