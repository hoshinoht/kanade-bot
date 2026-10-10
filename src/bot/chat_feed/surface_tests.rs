//! `DiscordSurface`: each driver effect is one transport call, never
//! retried, mentioning nobody, with its outcome classified for the driver.

use std::sync::Arc;

use twilight_model::channel::message::MessageFlags;

use super::DiscordSurface;
use crate::bot::mentions;
use crate::bot::transport::{AmbiguousKind, Call, FakeDiscord, Op, RejectionKind, SILENT, Step};
use crate::chat::driver::{Effect, Post, Surface};

const CHANNEL: &str = "900";
const ASKED: &str = "777";

fn fake_surface() -> (Arc<FakeDiscord>, DiscordSurface<FakeDiscord>) {
    let fake = Arc::new(FakeDiscord::with_first_message_id(5001));
    (fake.clone(), DiscordSurface(fake))
}

fn post<'a>(text: &'a str, reply_to: Option<&'static str>, silent: bool) -> Post<'a> {
    Post {
        text,
        reply_to,
        silent,
    }
}

#[tokio::test]
async fn posts_reply_or_stay_silent_and_never_mention() {
    let (fake, surface) = fake_surface();
    let placeholder = surface
        .post(CHANNEL, post("Checking…", Some(ASKED), true))
        .await;
    let reply = surface
        .post(CHANNEL, post("Hi <@1>", Some(ASKED), false))
        .await;
    let more = surface.post(CHANNEL, post("More", None, true)).await;
    assert_eq!(placeholder, Effect::Done("5001".into()));
    assert_eq!(reply, Effect::Done("5002".into()));
    assert_eq!(more, Effect::Done("5003".into()));
    assert_eq!(fake.create_flags(), [SILENT, MessageFlags::empty(), SILENT]);
    let sent: Vec<_> = fake
        .calls()
        .into_iter()
        .map(|call| match call {
            Call::Create { message, .. } => {
                assert_eq!(message.allowed_mentions, mentions::none());
                (
                    message.content.unwrap_or_default(),
                    message.reply_to.map(|id| id.get()),
                )
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        sent,
        [
            ("Checking…".to_owned(), Some(777)),
            ("Hi <@1>".to_owned(), Some(777)),
            ("More".to_owned(), None),
        ]
    );
}

#[tokio::test]
async fn edits_replace_the_text_only_and_mention_nobody() {
    let (fake, surface) = fake_surface();
    surface.post(CHANNEL, post("staging", None, true)).await;
    assert_eq!(
        surface.edit(CHANNEL, "5001", "the answer").await,
        Effect::Done(())
    );
    let Some(Call::Edit { message, edit, .. }) = fake.calls().pop() else {
        panic!("edit recorded");
    };
    assert_eq!(message.get(), 5001);
    assert_eq!(edit.content.as_deref(), Some("the answer"));
    assert_eq!(edit.embeds, None);
    assert_eq!(edit.allowed_mentions, mentions::none());
}

#[tokio::test]
async fn outcomes_are_classified_once_and_never_retried() {
    let (fake, surface) = fake_surface();
    for kind in [RejectionKind::NotSent, RejectionKind::RateLimited] {
        fake.script(Op::Create, Step::Reject(kind));
        assert_eq!(
            surface.post(CHANNEL, post("x", None, true)).await,
            Effect::NotSent
        );
    }
    fake.script(Op::Create, Step::Reject(RejectionKind::MissingAccess));
    assert_eq!(
        surface.post(CHANNEL, post("x", None, false)).await,
        Effect::Rejected("missing_access".into())
    );
    fake.script(
        Op::Create,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: true,
        },
    );
    assert_eq!(
        surface.post(CHANNEL, post("x", None, true)).await,
        Effect::Ambiguous("ambiguous_timeout".into())
    );
    assert_eq!(fake.count(Op::Create), 4, "one call each");
    assert_eq!(
        surface.edit(CHANNEL, "4242", "gone").await,
        Effect::UnknownMessage
    );
    assert_eq!(
        surface.delete(CHANNEL, "4242").await,
        Effect::UnknownMessage
    );
    assert_eq!(surface.delete(CHANNEL, "5001").await, Effect::Done(()));
    assert_eq!(
        surface.post("not-an-id", post("x", None, true)).await,
        Effect::Rejected("invalid_id".into())
    );
}

#[tokio::test]
async fn typing_is_fire_and_forget() {
    let (fake, surface) = fake_surface();
    fake.set_default(
        Op::Typing,
        Some(Step::Reject(RejectionKind::MissingPermissions)),
    );
    surface.typing(CHANNEL).await;
    surface.typing("bad").await;
    assert_eq!(fake.count(Op::Typing), 1, "refused once, not retried");
}
