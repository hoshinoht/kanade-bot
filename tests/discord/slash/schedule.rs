//! `/schedule`: a public embed that pings nobody, in the classic (v4) and
//! the redesigned message style.

use kanade::bot::delivery::cards::redesign::{INK_BLUE, SCHEDULE_FOOTER, SCHEDULE_FOOTER_HIDDEN};
use kanade::bot::transport::InteractionReply;
use kanade::domain::settings::MessageStyle;
use serde_json::json;
use twilight_model::channel::message::Embed;

use super::super::support::BOSSING_ROLE;
use super::{ALICE, BOB, DAN, KALOS, LOUNGE, Ports, R_KALOS, Slash, opt, user_opt};

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

fn fields(embed: &Embed) -> Vec<(&str, &str)> {
    embed
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.value.as_str()))
        .collect()
}

/// One public ink-blue embed with no content and no mention tags.
fn only_embed(reply: &InteractionReply) -> &Embed {
    assert!(!reply.ephemeral, "public");
    assert_eq!(reply.content, "");
    let [embed] = reply.embeds.as_slice() else {
        panic!("one embed: {reply:?}");
    };
    assert_eq!(embed.color, Some(INK_BLUE));
    let text = serde_json::to_string(embed).unwrap();
    assert!(!text.contains("<@"), "names, not tags: {text}");
    embed
}

#[tokio::test]
async fn the_redesign_names_the_channel_and_splits_the_party_by_answer() {
    let slash = redesigned().await;
    let reply = slash
        .run_in(DAN, &[BOSSING_ROLE], KALOS, "schedule", json!([]))
        .await;
    let embed = only_embed(&reply);
    assert_eq!(
        embed.title.as_deref(),
        Some("This boss week in #kalos-four")
    );
    assert_eq!(
        embed.description.as_deref(),
        Some("-# Thu 24 → Wed 30 Sep · 1 to go")
    );
    assert_eq!(
        fields(embed),
        [(
            "Tue 29 Sep",
            "⚠️ `22:00`  Extreme Gatekeeper Kalos\n-#   Waiting Alice, Bobby · #11111111"
        )]
    );
    assert_eq!(embed.footer.as_ref().unwrap().text, SCHEDULE_FOOTER);

    let reply = slash
        .run_in(
            ALICE,
            &[BOSSING_ROLE],
            KALOS,
            "schedule",
            json!([opt("week", "next")]),
        )
        .await;
    let embed = only_embed(&reply);
    assert_eq!(
        embed.title.as_deref(),
        Some("Next boss week in #kalos-four")
    );
    assert_eq!(
        embed.description.as_deref(),
        Some("-# Thu 01 → Wed 07 Oct · 1 to go")
    );
    assert_eq!(embed.fields[0].name, "Tue 06 Oct");
}

#[tokio::test]
async fn the_redesign_titles_your_runs_and_counts_hidden_ones() {
    let slash = redesigned().await;
    // Elsewhere the default is `mine`: Dan has nothing.
    let reply = slash
        .run_in(DAN, &[BOSSING_ROLE], LOUNGE, "schedule", json!([]))
        .await;
    let embed = only_embed(&reply);
    assert_eq!(embed.title.as_deref(), Some("Your runs this boss week"));
    assert_eq!(
        embed.description.as_deref(),
        Some(
            "-# Thu 24 → Wed 30 Sep · 0 to go\nYou have nothing left this week. \
             `/schedule scope:all` shows everyone's."
        )
    );
    assert!(embed.fields.is_empty());
    assert!(embed.footer.is_none());

    // Alice's: the cleared lounge run is hidden and counted.
    let reply = slash
        .run_in(ALICE, &[BOSSING_ROLE], LOUNGE, "schedule", json!([]))
        .await;
    let embed = only_embed(&reply);
    assert_eq!(embed.title.as_deref(), Some("Your runs this boss week"));
    assert_eq!(
        embed.description.as_deref(),
        Some("-# Thu 24 → Wed 30 Sep · 1 to go · 1 cleared (hidden)")
    );
    assert_eq!(embed.fields.len(), 1);
    assert_eq!(embed.footer.as_ref().unwrap().text, SCHEDULE_FOOTER_HIDDEN);

    let reply = slash
        .run_in(
            ALICE,
            &[BOSSING_ROLE],
            LOUNGE,
            "schedule",
            json!([opt("scope", "all"), opt("show_past", true)]),
        )
        .await;
    let embed = only_embed(&reply);
    assert_eq!(embed.title.as_deref(), Some("Every run this boss week"));
    assert_eq!(
        embed.description.as_deref(),
        Some("-# Thu 24 → Wed 30 Sep · 1 to go · 1 cleared")
    );
    assert_eq!(
        fields(embed)[0],
        (
            "Sat 26 Sep",
            "🏁 `21:00`  Extreme Gatekeeper Kalos\n-#   Waiting Alice · #22222222"
        )
    );
    assert_eq!(embed.footer.as_ref().unwrap().text, SCHEDULE_FOOTER);
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
        only_embed(&reply).fields[0].value,
        "⚠️ `22:00`  Extreme Gatekeeper Kalos\n\
         -#   In Alice · Waiting Dan · Dan standing in for Bobby · #11111111"
    );
}
