//! `DiscordSurface::reply`: one message within the bound, follow-ups past it.

use std::sync::Arc;

use serde_json::Map;

use super::DiscordSurface;
use crate::bot::mentions;
use crate::bot::transport::{Call, FakeDiscord, Op, Outcome, RejectionKind, Step};
use crate::chat::driver::Surface;
use crate::chat::sanitize::shape_reply;
use crate::chat::tools::{MAX_MEMBER_REPLY, ToolOutcome};

const CHANNEL: &str = "900";
const ASKED: &str = "777";

/// `(content, reply_to)` of every create call, delivered or not.
fn sent(fake: &FakeDiscord) -> Vec<(String, Option<u64>)> {
    fake.calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Create { message, .. } => {
                assert_eq!(message.allowed_mentions, mentions::none());
                Some((
                    message.content.unwrap_or_default(),
                    message.reply_to.map(|id| id.get()),
                ))
            }
            _ => None,
        })
        .collect()
}

fn fake_surface() -> (Arc<FakeDiscord>, DiscordSurface<FakeDiscord>) {
    let fake = Arc::new(FakeDiscord::new());
    (fake.clone(), DiscordSurface(fake))
}

fn schedule() -> ToolOutcome {
    let records: Vec<String> = [
        (
            "1a2b3c4d",
            "Hard Lucid",
            "Mon 21 Sep · 21:00",
            " · *already happened*",
        ),
        ("7c5f4680", "Hard Baldrix", "Sun 27 Sep · 22:00", ""),
        ("8d6e5791", "Extreme Kalos", "Sun 27 Sep · 23:30", ""),
    ]
    .iter()
    .map(|(id, boss, when, past)| {
        format!("`[{id}]` **{boss}**\n*{when}* · `planned` · `2/6 yes` · <#9001>{past}")
    })
    .collect();
    ToolOutcome {
        name: "get_schedule".to_owned(),
        output: format!(
            "**Your 3 runs this week · All channels**\n\n{}",
            records.join("\n\n")
        ),
        arguments: Map::new(),
        ok: true,
        error: None,
        created: Vec::new(),
        cards: Vec::new(),
        detail: None,
    }
}

fn rust_fence(lines: usize) -> String {
    let body: Vec<String> = (0..lines)
        .map(|n| format!("        let centre_{n} = expand(bytes, {n}, {n} + 1); // odd  and even"))
        .collect();
    format!(
        "```rust\nfn longest_palindrome(s: &str) -> &str {{\n    let bytes = s.as_bytes();\n{}\n    &s[..]\n}}\n```",
        body.join("\n")
    )
}

#[tokio::test]
async fn a_short_reply_is_one_unchanged_message() {
    let (fake, surface) = fake_surface();
    let id = surface.reply(CHANNEL, ASKED, "Hi  there~").await.unwrap();
    assert_eq!(sent(&fake), vec![("Hi  there~".to_owned(), Some(777))]);
    assert_eq!(fake.count(Op::Create), 1);
    assert!(!id.is_empty());
}

/// The live 11fedae9 shape over the bound: the schedule answer, then the
/// code whole and in order, then the closing lines, as follow-ups.
#[tokio::test]
async fn a_long_answer_posts_in_order_as_follow_ups() {
    let code = rust_fence(15);
    let closing = "Good luck tonight, you've got this!\n\nSee you at the run~";
    let reply = format!(
        "**Your next boss run**\n\n[7c5f4680] **Hard Baldrix** – *Sun 27 Sep · 22:00* – #hbaldguy\n\n{code}\n\n{closing}"
    );
    let shaped = shape_reply(&reply, &[schedule()]);
    assert!(shaped.chars().count() > MAX_MEMBER_REPLY);
    assert!(shaped.contains(&code));

    let (fake, surface) = fake_surface();
    let first = surface.reply(CHANNEL, ASKED, &shaped).await.unwrap();
    let posted = sent(&fake);
    assert!(posted.len() >= 2, "{posted:?}");
    assert_eq!(posted[0].1, Some(777));
    assert!(posted[1..].iter().all(|(_, to)| to.is_none()));
    assert!(
        posted[0]
            .0
            .starts_with("**Your next boss run**\n\n`[7c5f4680]`")
    );
    assert!(
        posted.iter().any(|(text, _)| text.contains(&code)),
        "{posted:?}"
    );
    let texts: Vec<&str> = posted.iter().map(|(text, _)| text.as_str()).collect();
    assert_eq!(texts.join("\n\n"), shaped);
    assert!(texts.last().unwrap().ends_with("See you at the run~"));
    let Call::Create {
        outcome: Outcome::Delivered(id),
        ..
    } = &fake.calls()[0]
    else {
        panic!("first create")
    };
    assert_eq!(first, id.get().to_string());
}

/// A failed follow-up is logged and ends the reply; the reply itself stands.
#[tokio::test]
async fn a_failed_follow_up_stops_without_failing_the_reply() {
    let text = (0..6)
        .map(|n| format!("Paragraph {n}: {}", "words ".repeat(60)))
        .collect::<Vec<_>>()
        .join("\n\n");
    let (fake, surface) = fake_surface();
    fake.script(Op::Create, Step::Succeed);
    fake.script(Op::Create, Step::Reject(RejectionKind::MissingPermissions));
    assert!(surface.reply(CHANNEL, ASKED, &text).await.is_ok());
    assert_eq!(fake.count(Op::Create), 2);

    let (fake, surface) = fake_surface();
    fake.script(Op::Create, Step::Reject(RejectionKind::MissingAccess));
    assert!(surface.reply(CHANNEL, ASKED, &text).await.is_err());
    assert_eq!(fake.count(Op::Create), 1);
}
