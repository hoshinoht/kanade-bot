//! `/schedule`: one Components V2 container that pings nobody, or the
//! embed for a week over the V2 budget. The digest's "My runs" button
//! answers the presser alone.

use kanade::bot::commands::Disposition;
use kanade::bot::delivery::cards::redesign::{INK_BLUE, SCHEDULE_FOOTER, SCHEDULE_FOOTER_HIDDEN};
use kanade::bot::transport::InteractionReply;
use kanade_domain::schedule::{Change, Run, RunSource, RunStatus};
use serde_json::json;

use super::super::support::{BOSSING_ROLE, button_interaction, v2_accent, v2_texts};
use super::{ALICE, BOB, CARA, DAN, KALOS, LOUNGE, R_KALOS, Slash, opt, user_opt, utc};

/// Dan stands in for Bob on `R_KALOS` this week, and Alice answers yes.
async fn stand_in_dan(slash: &Slash) {
    slash
        .run(
            ALICE,
            "swap",
            json!([
                opt("run_id", R_KALOS),
                user_opt("out", BOB),
                user_opt("in", DAN)
            ]),
        )
        .await;
    slash
        .run(
            ALICE,
            "rsvp",
            json!([opt("run_id", R_KALOS), opt("answer", "yes")]),
        )
        .await;
}

async fn redesigned() -> Slash {
    Slash::new().await
}

/// One public ink-blue V2 container with no content, no embeds and no
/// mention tags: its text displays.
fn only_v2(reply: &InteractionReply) -> Vec<String> {
    assert!(!reply.ephemeral, "public");
    assert_eq!((reply.content.as_str(), reply.embeds.len()), ("", 0));
    assert_eq!(v2_accent(&reply.components), Some(INK_BLUE));
    let texts = v2_texts(&reply.components);
    assert!(
        texts.iter().all(|text| !text.contains("<@")),
        "names, not tags: {texts:?}"
    );
    texts
}

#[tokio::test]
async fn the_redesign_names_the_channel_and_splits_the_party_by_answer() {
    let slash = redesigned().await;
    let reply = slash
        .run_in(DAN, &[BOSSING_ROLE], KALOS, "schedule", json!([]))
        .await;
    assert_eq!(
        only_v2(&reply),
        [
            "## This boss week in #kalos-four\n-# Thu 24 → Wed 30 Sep · 1 to go".to_owned(),
            "### Tue 29 Sep\n⚠️ `22:00`  Extreme Gatekeeper Kalos\n\
             -#   Waiting Alice, Bobby · #11111111"
                .to_owned(),
            format!("-# {SCHEDULE_FOOTER}"),
        ]
    );

    let reply = slash
        .run_in(
            ALICE,
            &[BOSSING_ROLE],
            KALOS,
            "schedule",
            json!([opt("week", "next")]),
        )
        .await;
    let texts = only_v2(&reply);
    assert_eq!(
        texts[0],
        "## Next boss week in #kalos-four\n-# Thu 01 → Wed 07 Oct · 1 to go"
    );
    assert!(texts[1].starts_with("### Tue 06 Oct\n"), "{texts:?}");
}

#[tokio::test]
async fn the_redesign_titles_your_runs_and_counts_hidden_ones() {
    let slash = redesigned().await;
    // Elsewhere the default is `mine`: Dan has nothing.
    let reply = slash
        .run_in(DAN, &[BOSSING_ROLE], LOUNGE, "schedule", json!([]))
        .await;
    assert_eq!(
        only_v2(&reply),
        [
            "## Your runs this boss week\n-# Thu 24 → Wed 30 Sep · 0 to go\n\
          You have nothing left this week. `/schedule scope:all` shows everyone's."
        ]
    );

    // Alice's: the cleared lounge run is hidden and counted.
    let reply = slash
        .run_in(ALICE, &[BOSSING_ROLE], LOUNGE, "schedule", json!([]))
        .await;
    let texts = only_v2(&reply);
    assert_eq!(
        texts[0],
        "## Your runs this boss week\n-# Thu 24 → Wed 30 Sep · 1 to go · 1 cleared (hidden)"
    );
    assert_eq!(texts.len(), 3, "{texts:?}");
    assert_eq!(texts[2], format!("-# {SCHEDULE_FOOTER_HIDDEN}"));

    let reply = slash
        .run_in(
            ALICE,
            &[BOSSING_ROLE],
            LOUNGE,
            "schedule",
            json!([opt("scope", "all"), opt("show_past", true)]),
        )
        .await;
    let texts = only_v2(&reply);
    assert_eq!(
        texts[0],
        "## Every run this boss week\n-# Thu 24 → Wed 30 Sep · 1 to go · 1 cleared"
    );
    assert_eq!(
        texts[1],
        "### Sat 26 Sep\n🏁 `21:00`  Extreme Gatekeeper Kalos\n-#   Waiting Alice · #22222222"
    );
    assert_eq!(texts.last(), Some(&format!("-# {SCHEDULE_FOOTER}")));
}

#[tokio::test]
async fn the_redesign_names_a_one_week_stand_in() {
    let slash = redesigned().await;
    stand_in_dan(&slash).await;
    let reply = slash
        .run_in(ALICE, &[BOSSING_ROLE], KALOS, "schedule", json!([]))
        .await;
    assert_eq!(
        only_v2(&reply)[1],
        "### Tue 29 Sep\n❗ `22:00`  Extreme Gatekeeper Kalos\n\
         -#   In Alice · Waiting Dan · Dan standing in for Bobby · #11111111"
    );
}

/// `count` more runs of the party channel on Tue 29 Sep, a minute apart.
async fn crowd(slash: &Slash, count: u64) {
    let changes = (0..count)
        .map(|n| {
            Change::PutRun(Run {
                id: format!("{:08x}-0000-4000-8000-{n:012x}", 0x5000_0000 + n),
                fixed_run_id: None,
                channel_id: Some(KALOS.to_string()),
                week_start: utc(9, 23, 16, 0),
                datetime: utc(9, 29, 15, 0) + chrono::TimeDelta::minutes(n as i64),
                bosses: vec!["XKalos".into()],
                participants: vec![ALICE.to_string(), BOB.to_string()],
                status: RunStatus::Planned,
                source: RunSource::Amend,
                attendance: Vec::new(),
                status_pin: None,
            })
        })
        .collect();
    super::seed_changes(&slash.store, changes, utc(9, 28, 0, 0)).await;
}

#[tokio::test]
async fn a_week_over_the_v2_budget_gets_the_redesigned_embed() {
    let slash = redesigned().await;
    // One day's text display would pass Discord's 2,000-byte limit.
    crowd(&slash, 30).await;
    let reply = slash
        .run_in(DAN, &[BOSSING_ROLE], KALOS, "schedule", json!([]))
        .await;
    assert!(reply.components.is_empty(), "no V2 layout: {reply:?}");
    assert!(!reply.ephemeral);
    let embed = reply.embeds.first().expect("the redesigned embed");
    assert_eq!(embed.color, Some(INK_BLUE));
    assert_eq!(
        embed.title.as_deref(),
        Some("This boss week in #kalos-four")
    );
    assert_eq!(
        embed.description.as_deref(),
        Some("-# Thu 24 → Wed 30 Sep · 31 to go")
    );
}

#[tokio::test]
async fn my_runs_answers_the_presser_alone_with_their_runs() {
    let slash = redesigned().await;
    let press = button_interaction(9001, ALICE, &[BOSSING_ROLE], KALOS, 4242, "digest:mine");
    let (disposition, reply) = slash.send(&press).await;
    assert_eq!(disposition, Disposition::Ran);
    assert!(reply.ephemeral, "only the presser sees it");
    assert_eq!((reply.content.as_str(), reply.embeds.len()), ("", 0));
    let texts = v2_texts(&reply.components);
    assert_eq!(
        texts[0],
        "## Your runs this boss week\n-# Thu 24 → Wed 30 Sep · 1 to go · 1 cleared (hidden)",
        "Alice's, wherever the digest is"
    );

    // `/schedule`'s own gate applies: no bossing role, no schedule.
    let press = button_interaction(9002, CARA, &[], KALOS, 4242, "digest:mine");
    let (disposition, reply) = slash.send(&press).await;
    assert!(
        matches!(disposition, Disposition::Denied(_)),
        "{disposition:?}"
    );
    assert!(reply.ephemeral);
    assert!(reply.components.is_empty());
}
