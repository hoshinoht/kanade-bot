//! `/schedule`: a public embed that pings nobody.

use serde_json::json;

use super::super::support::BOSSING_ROLE;
use super::{ALICE, BOB, DAN, KALOS, LOUNGE, R_KALOS, Slash, opt, user_opt};

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
