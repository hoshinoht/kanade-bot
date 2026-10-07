//! `/schedule`: a public embed that pings nobody in the classic (v4)
//! style; in the redesigned style one Components V2 container, or the
//! redesigned embed for a week over the V2 budget. The digest's "My runs"
//! button answers the presser alone.

use kanade::bot::commands::Disposition;
use kanade::bot::delivery::cards::redesign::{INK_BLUE, SCHEDULE_FOOTER, SCHEDULE_FOOTER_HIDDEN};
use kanade::bot::transport::InteractionReply;
use kanade::domain::history::{Actor, ChangeMeta, Origin, Surface};
use kanade::domain::schedule::{Change, ChangeSet, Run, RunSource, RunStatus};
use kanade::domain::scheduler::{ScheduleStore, Scope};
use kanade::domain::settings::MessageStyle;
use serde_json::json;

use super::super::support::{BOSSING_ROLE, button_interaction, v2_accent, v2_texts};
use super::{ALICE, BOB, CARA, DAN, KALOS, LOUNGE, Ports, R_KALOS, Slash, opt, user_opt, utc};

/// v4 `formatting.schedule_line` for `R_KALOS` with no answers, printed by
/// the rollback tree.
const KALOS_LINE: &str =
    "`#11111111` · 22:00 · **XKalos** · <@1001> <@1002> · ⚠️ unconfirmed · 0/2 ✅";
const FOOTER: &str = "✅/❌ react on a reminder to RSVP · /amend to move a run";

#[tokio::test]
async fn party_channels_default_to_their_own_runs() {
    let slash = Slash::new().await;
    let reply = slash
        .run_in(DAN, &[BOSSING_ROLE], KALOS, "schedule", json!([]))
        .await;
    assert!(!reply.ephemeral, "public");
    assert_eq!(reply.content, "");
    let [embed] = reply.embeds.as_slice() else {
        panic!("one embed");
    };
    assert_eq!(
        embed.title.as_deref(),
        Some("Boss week of Thu 24 Sep (channel)")
    );
    assert_eq!(embed.color, Some(0x5865F2));
    assert_eq!(embed.fields.len(), 1);
    assert_eq!(embed.fields[0].name, "Tue 29 Sep");
    assert_eq!(embed.fields[0].value, KALOS_LINE);
    assert_eq!(embed.footer.as_ref().unwrap().text, FOOTER);
}

#[tokio::test]
async fn scopes_weeks_and_hidden_runs_follow_v4() {
    let slash = Slash::new().await;
    // Elsewhere the default is `mine`: Dan has nothing.
    let reply = slash
        .run_in(DAN, &[BOSSING_ROLE], LOUNGE, "schedule", json!([]))
        .await;
    let embed = &reply.embeds[0];
    assert_eq!(
        embed.title.as_deref(),
        Some("Boss week of Thu 24 Sep (mine)")
    );
    assert_eq!(
        embed.description.as_deref(),
        Some("You have nothing left this week. `/schedule scope:all` shows everyone's.")
    );

    // Everyone's: the finished lounge run is hidden and counted.
    let reply = slash
        .run_in(
            ALICE,
            &[BOSSING_ROLE],
            LOUNGE,
            "schedule",
            json!([opt("scope", "all")]),
        )
        .await;
    let embed = &reply.embeds[0];
    assert_eq!(embed.fields.len(), 1);
    assert_eq!(
        embed.footer.as_ref().unwrap().text,
        format!("{FOOTER} · 1 past/cancelled run(s) hidden — `show_past:True` to see them")
    );
    let reply = slash
        .run_in(
            ALICE,
            &[BOSSING_ROLE],
            LOUNGE,
            "schedule",
            json!([opt("scope", "all"), opt("show_past", true)]),
        )
        .await;
    let names: Vec<&str> = reply.embeds[0]
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect();
    assert_eq!(names, ["Sat 26 Sep", "Tue 29 Sep"]);
    assert!(reply.embeds[0].fields[0].value.contains("🏁 done"));

    let reply = slash
        .run_in(
            ALICE,
            &[BOSSING_ROLE],
            KALOS,
            "schedule",
            json!([opt("week", "next")]),
        )
        .await;
    let embed = &reply.embeds[0];
    assert_eq!(
        embed.title.as_deref(),
        Some("Boss week of Thu 01 Oct (channel)")
    );
    assert_eq!(embed.fields[0].name, "Tue 06 Oct");
}

#[tokio::test]
async fn a_one_week_stand_in_shows_its_roster_delta() {
    let slash = Slash::new().await;
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
    let reply = slash
        .run_in(ALICE, &[BOSSING_ROLE], KALOS, "schedule", json!([]))
        .await;
    // Bob's nickname is what v4 `_member_name` shows.
    assert_eq!(
        reply.embeds[0].fields[0].value,
        "`#11111111` · 22:00 · **XKalos** · <@1001> <@1004> · ⚠️ unconfirmed · 1/2 ✅ · \
         _this week: −Bobby +Dan_"
    );
}

async fn redesigned() -> Slash {
    Slash::with(Ports {
        style: MessageStyle::Redesigned,
        ..Ports::default()
    })
    .await
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
    let reply = slash
        .run_in(ALICE, &[BOSSING_ROLE], KALOS, "schedule", json!([]))
        .await;
    assert_eq!(
        only_v2(&reply)[1],
        "### Tue 29 Sep\n⚠️ `22:00`  Extreme Gatekeeper Kalos\n\
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
    let revision = slash.store.load(&Scope::All).await.unwrap().revision;
    slash
        .store
        .commit(
            revision,
            ChangeSet { changes },
            ChangeMeta {
                origin: Origin::new(Actor::admin("seed"), Surface::AdminPortal),
                at: utc(9, 28, 0, 0),
                notices: Vec::new(),
                refs: Vec::new(),
                request_digest: None,
                expect: Default::default(),
                outbox: Vec::new(),
            },
        )
        .await
        .unwrap();
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
