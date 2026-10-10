//! The redesigned proposal card through the desk as an embed: the style is
//! read live per render, labels use the catalog and difficulty marks, and
//! an outcome recolours the card instead of appending v4's notice. A card
//! posted redesigned goes out as Components V2 (`v2.rs`); the embed is what
//! a card posted in the classic style shows once the style is redesigned
//! (a post keeps its format), so these tests post classic, then flip.

use std::sync::atomic::AtomicBool;

use super::*;
use kanade::bot::cards::CLOSED_GREY;
use kanade::bot::delivery::cards::{CardKit, DifficultyMarks};
use kanade::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec};
use kanade::domain::ids::tag;
use kanade::domain::settings::MessageStyle;
use twilight_model::channel::message::Embed;

const PROPOSAL_PINK: u32 = 0xEB459E;
const SETTLED_GREEN: u32 = 0x3BA55C;
const HARD: &str = "<:diff_h:2000000000000000001>";

fn catalog() -> BossTable {
    let boss = |short: &str, full: &str| BossSpec {
        short: short.into(),
        full: Some(full.into()),
        ..BossSpec::default()
    };
    BossTable::from_spec(&CatalogSpec {
        difficulties: vec![
            DifficultySpec {
                prefix: "n".into(),
                label: "Normal".into(),
            },
            DifficultySpec {
                prefix: "h".into(),
                label: "Hard".into(),
            },
        ],
        bosses: vec![
            boss("MaleficStar", "Radiant Malefic Star"),
            boss("FA", "The First Adversary"),
        ],
    })
    .expect("catalog")
}

/// A desk over `world` rendering redesigned while `redesign` is set, read
/// on every render.
pub(super) fn styled_desk(
    world: &World,
    redesign: &Arc<AtomicBool>,
    marks: DifficultyMarks,
) -> Desk {
    let redesign = Arc::clone(redesign);
    desk(&world.store, &world.discord, &world.alerts, &world.ids).with_cards(CardKit {
        catalog: Some(Arc::new(catalog())),
        style: Some(Arc::new(move || {
            if redesign.load(Ordering::SeqCst) {
                MessageStyle::Redesigned
            } else {
                MessageStyle::Classic
            }
        })),
        marks,
        ..CardKit::default()
    })
}

pub(super) fn redesigned() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(true))
}

/// A card posted in the classic style (with ✅/❌), then the style flipped
/// to redesigned: the desk, the proposal id and the message id.
async fn legacy_card(world: &World, marks: DifficultyMarks) -> (Desk, String, String) {
    let style = Arc::new(AtomicBool::new(false));
    let desk = styled_desk(world, &style, marks);
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    assert_eq!(
        desk.post_card(&card(vec![entry], Vec::new())).await,
        PostResult::Posted
    );
    style.store(true, Ordering::SeqCst);
    let message = world.message_of(&id).await.expect("posted");
    (desk, id, message)
}

fn last_edit(world: &World) -> (String, Embed) {
    world
        .discord
        .calls()
        .into_iter()
        .rev()
        .find_map(|call| match call {
            Call::Edit { edit, .. } => Some((
                edit.content.expect("content"),
                edit.embeds.expect("embeds").remove(0),
            )),
            _ => None,
        })
        .expect("an edit")
}

fn footer(embed: &Embed) -> &str {
    &embed.footer.as_ref().expect("footer").text
}

#[tokio::test]
async fn a_legacy_card_shown_redesigned_asks_the_question_with_from_to_and_party() {
    let world = World::new().await;
    let (desk, _, message) = legacy_card(&world, DifficultyMarks::new().with("h", HARD)).await;
    assert_eq!(world.discord.count(Op::AddReaction), 2, "✅ and ❌ seeded");
    assert!(desk.refresh(&message).await);
    let (content, embed) = last_edit(&world);
    assert_eq!(
        content,
        format!(
            "📋 **Move {HARD} Radiant Malefic Star + {HARD} The First Adversary?** \
             moving to wed"
        )
    );
    assert_eq!(embed.color, Some(PROPOSAL_PINK));
    let fields: Vec<(&str, &str, bool)> = embed
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.value.as_str(), field.inline))
        .collect();
    let when = format!("**<t:{}:F>**", local(9, 2, 21, 30).timestamp());
    assert_eq!(
        fields,
        [
            ("From", "~~Mon 31 Aug 21:30~~", true),
            ("To", when.as_str(), true),
            ("Party", "Mylene, Alvin", true),
        ]
    );
    assert_eq!(
        footer(&embed),
        format!(
            "{} · 90% sure · ✅ apply · ❌ reject · /amend to edit",
            tag(&world.run)
        ),
        "an embed card is still answered by reaction"
    );
    assert_eq!(world.discord.count(Op::Flags), 0, "its format is known");
}

#[tokio::test]
async fn an_applied_card_turns_green_and_says_who_and_when() {
    let world = World::new().await;
    let (desk, _, message) = legacy_card(&world, DifficultyMarks::new()).await;
    assert!(matches!(
        desk.on_reaction(&message, MY, RsvpAnswer::Yes, true).await,
        CardReaction::Approved { ref approved, .. } if approved.len() == 1
    ));
    let (content, embed) = last_edit(&world);
    assert_eq!(
        content,
        "📋 **Move Hard Radiant Malefic Star + Hard The First Adversary?** moving to wed\n\
         -# ✅ Applied by Mylene at 13:07"
    );
    assert_eq!(embed.color, Some(SETTLED_GREEN));
    assert_eq!(footer(&embed), format!("{} · applied", tag(&world.run)));
}

#[tokio::test]
async fn a_rejected_card_greys_out_with_its_heading_struck() {
    let world = World::new().await;
    let (desk, id, message) = legacy_card(&world, DifficultyMarks::new()).await;
    desk.on_reaction(&message, ADMIN, RsvpAnswer::No, true)
        .await;
    assert_eq!(world.status(&id).await, DraftStatus::Rejected);
    let (content, embed) = last_edit(&world);
    assert_eq!(
        content,
        "📋 ~~**Move Hard Radiant Malefic Star + Hard The First Adversary?**~~ moving to wed\n\
         -# ❌ Rejected by Boss at 13:07 · run unchanged"
    );
    assert_eq!(embed.color, Some(CLOSED_GREY));
    assert_eq!(footer(&embed), format!("{} · rejected", tag(&world.run)));
}

#[tokio::test]
async fn a_card_left_out_of_date_greys_out_while_still_open() {
    let world = World::new().await;
    let (desk, id, message) = legacy_card(&world, DifficultyMarks::new()).await;
    service(&world.store, &world.ids)
        .as_origin(Origin::for_tests())
        .amend_run(&world.run, local(9, 1, 22, 0), &policy())
        .await
        .expect("moved by hand");
    desk.on_reaction(&message, MY, RsvpAnswer::Yes, true).await;
    assert_eq!(world.status(&id).await, DraftStatus::Submitted);
    let (content, embed) = last_edit(&world);
    assert!(content.starts_with("📋 ~~**Move"), "{content}");
    assert!(
        content.ends_with("\n-# ⚠️ Out of date · run unchanged"),
        "{content}"
    );
    assert_eq!(embed.color, Some(CLOSED_GREY));
    assert_eq!(footer(&embed), format!("{} · out of date", tag(&world.run)));
    assert_eq!(
        embed.fields[0].value, "~~Tue 01 Sep 22:00~~",
        "From reads the run as it stands now"
    );
}

#[tokio::test]
async fn the_style_is_read_live_on_every_render() {
    let world = World::new().await;
    let style = Arc::new(AtomicBool::new(false));
    let desk = styled_desk(&world, &style, DifficultyMarks::new());
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    desk.post_card(&card(vec![entry], Vec::new())).await;
    let Some(Call::Create { message, .. }) = world.creates().pop() else {
        panic!("a post");
    };
    assert_eq!(
        message.content.as_deref(),
        Some("📋 Proposed change\nMylene"),
        "classic stays v4's card"
    );

    style.store(true, Ordering::SeqCst);
    let posted = world.message_of(&id).await.expect("posted");
    assert!(desk.refresh(&posted).await);
    let (content, embed) = last_edit(&world);
    assert!(
        content.starts_with("📋 **Move Hard Radiant Malefic Star"),
        "{content}"
    );
    assert!(embed.fields.iter().all(|field| field.inline));
}

#[tokio::test]
async fn an_offline_replay_after_restart_keeps_the_redesigned_style() {
    use kanade::bot::cards::OFFLINE_CONFLICT_NOTICE;
    use twilight_model::channel::message::ReactionType;

    let seed = |world: &World, message: &str, emoji: &str, user: &str| {
        world.discord.seed_reactions(
            Id::new(message.parse().unwrap()),
            emoji,
            ReactionType::Normal,
            vec![Id::new(user.parse().unwrap())],
        );
    };
    // The bot's own user (an admin, so ignoring it must come first).
    let me = Id::new(1003);

    // An offline ✅ on a card posted classic: the restarted desk, now
    // redesigned, applies it and the card turns green.
    let world = World::new().await;
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    styled_desk(
        &world,
        &Arc::new(AtomicBool::new(false)),
        DifficultyMarks::new(),
    )
    .post_card(&card(vec![entry], Vec::new()))
    .await;
    let message = world.message_of(&id).await.expect("posted");
    seed(&world, &message, "✅", MY);
    let restarted = styled_desk(&world, &redesigned(), DifficultyMarks::new());
    assert_eq!(restarted.replay_reactions(me, || true).await.approved, 1);
    let (content, embed) = last_edit(&world);
    assert!(
        content.ends_with("\n-# ✅ Applied by Mylene at 13:07"),
        "{content}"
    );
    assert_eq!(embed.color, Some(SETTLED_GREEN));

    // Opposing offline answers: still open and pink, the note as subtext,
    // and an ordinary refresh keeps both style and note.
    let world = World::new().await;
    let (desk, _, message) = legacy_card(&world, DifficultyMarks::new()).await;
    seed(&world, &message, "✅", MY);
    seed(&world, &message, "❌", ALVIN);
    assert_eq!(desk.replay_reactions(me, || true).await.conflicts, 1);
    for refreshed in [false, true] {
        if refreshed {
            assert!(desk.refresh(&message).await);
        }
        let (content, embed) = last_edit(&world);
        assert!(content.starts_with("📋 **Move"), "{content}");
        assert!(
            content.ends_with(&format!("\n-# {OFFLINE_CONFLICT_NOTICE}")),
            "{content}"
        );
        assert_eq!(embed.color, Some(PROPOSAL_PINK));
        assert!(footer(&embed).ends_with("✅ apply · ❌ reject · /amend to edit"));
    }
}
