//! The redesigned day-of, countdown and digest cards over the sample week:
//! content and subtext, per-run embeds, colours, timestamps, art and the
//! Discord limits.

use chrono::TimeDelta;
use kanade::bot::delivery::cards::redesign::{
    AT_RISK_RED, DifficultyMarks, INK_BLUE, MAX_EMBEDS, MAX_TOTAL_CHARS, NO_MARKS, SETTLED_GREEN,
    WAITING_AMBER,
};
use kanade::bot::delivery::cards::{self, Card, DIGEST_EMPTY, REACT_HINT, fetch_art};
use kanade::bot::transport::OutgoingMessage;
use kanade::domain::attendance::AttendancePolicy;
use kanade::domain::members::{Member, PingLevel};
use kanade::domain::notify::IntentContent;
use kanade::domain::schedule::{RunStatus, ScheduleSnapshot};
use twilight_model::channel::message::Embed;

use super::{
    CLEARED, KALOS, MALEFIC, RISKY, art, inclusion, members, redesigned, week, week_start,
};
use crate::cards::{STAR_COLOUR, catalog};

const STAR_AT: i64 = 1_788_960_600;
const KALOS_AT: i64 = STAR_AT + 90 * 60;

fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

async fn post(card: &Card, mentioned: &[String]) -> OutgoingMessage {
    let pictures = fetch_art(Some(&art()), card, true).await;
    card.message(mentioned, &pictures)
}

fn fields(embed: &Embed) -> Vec<(&str, &str, bool)> {
    embed
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.value.as_str(), field.inline))
        .collect()
}

fn url(picture: Option<&String>) -> Option<&str> {
    picture.map(String::as_str)
}

fn thumbnail(embed: &Embed) -> Option<&str> {
    url(embed.thumbnail.as_ref().map(|thumb| &thumb.url))
}

fn image(embed: &Embed) -> Option<&str> {
    url(embed.image.as_ref().map(|image| &image.url))
}

fn uploads(message: &OutgoingMessage) -> Vec<&str> {
    message
        .attachments
        .iter()
        .map(|upload| upload.filename.as_str())
        .collect()
}

fn day_of(run_ids: &[&str]) -> IntentContent {
    IntentContent::DayOf {
        run_ids: ids(run_ids),
    }
}

fn countdown(run_id: &str) -> IntentContent {
    IntentContent::Countdown {
        run_id: run_id.into(),
        minutes: 15,
    }
}

#[tokio::test]
async fn a_v5_day_of_has_one_embed_per_run_and_pings_the_waiting_in_subtext() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let ctx = redesigned(
        &schedule,
        &roster,
        &table,
        AttendancePolicy::V5,
        false,
        &NO_MARKS,
    );
    let mentioned = ids(&["1003"]);
    let card = cards::build(&day_of(&[KALOS, MALEFIC]), &ctx, None, &mentioned).unwrap();
    let message = post(&card, &mentioned).await;
    assert_eq!(
        message.content.as_deref(),
        Some("📅 **Today — Wed 09 Sep**\n-# Waiting on <@1003> · react ✅ in / ❌ out")
    );
    assert_eq!(
        serde_json::to_value(&message.allowed_mentions).unwrap(),
        serde_json::json!({"parse": [], "users": ["1003"]})
    );
    let [star, kalos] = message.embeds.as_slice() else {
        panic!("one embed per run: {:?}", message.embeds);
    };
    // Time order; each run in its lead boss's colour (ink-blue without one).
    assert_eq!(
        star.title.as_deref(),
        Some("Hard Radiant Malefic Star + Hard Gatekeeper Kalos")
    );
    assert_eq!(star.color, Some(STAR_COLOUR));
    assert_eq!(
        star.description.as_deref(),
        Some(format!("🕘 **<t:{STAR_AT}:t>** · <t:{STAR_AT}:R> · ⚠️ waiting on 1").as_str())
    );
    assert_eq!(
        fields(star),
        [
            ("In ✅", "Aria", true),
            ("Waiting", "<@1003>", true),
            ("Out ❌", "Bex", true)
        ]
    );
    assert_eq!(
        star.footer.as_ref().map(|footer| footer.text.as_str()),
        Some("#a1b2c3d4 · Lv280 + Lv265")
    );
    assert_eq!(thumbnail(star), Some("attachment://MaleficStar.png"));
    assert_eq!(image(star), Some("attachment://image-MaleficStar.png"));

    assert_eq!(kalos.title.as_deref(), Some("Extreme Gatekeeper Kalos"));
    assert_eq!(kalos.color, Some(INK_BLUE));
    assert_eq!(
        kalos.description.as_deref(),
        Some(format!("🕘 **<t:{KALOS_AT}:t>** · <t:{KALOS_AT}:R> · ✅ all in").as_str())
    );
    assert_eq!(
        fields(kalos),
        [
            ("In ✅", "Aria, <@1003>", true),
            ("Waiting", "—", true),
            ("Out ❌", "—", true)
        ]
    );
    assert_eq!(
        kalos.footer.as_ref().map(|footer| footer.text.as_str()),
        Some("#e5f6a7b8 · Lv265")
    );
    assert_eq!(thumbnail(kalos), Some("attachment://Kalos.png"));
    assert_eq!(image(kalos), None, "entry art on the first run only");
    assert_eq!(
        uploads(&message),
        ["MaleficStar.png", "image-MaleficStar.png", "Kalos.png"]
    );
}

#[tokio::test]
async fn v4_compat_names_the_pinged_party_and_quiet_pings_nobody() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let loud = redesigned(
        &schedule,
        &roster,
        &table,
        AttendancePolicy::V4_COMPAT,
        false,
        &NO_MARKS,
    );
    let mentioned = ids(&["1001", "1003"]);
    let card = cards::build(&day_of(&[MALEFIC]), &loud, Some("Tonight!"), &mentioned).unwrap();
    assert_eq!(
        card.content,
        "📅 **Tonight!**\n-# <@1001> <@1003> · react ✅ in / ❌ out"
    );
    assert_eq!(card.embeds.len(), 1);

    let quiet = redesigned(
        &schedule,
        &roster,
        &table,
        AttendancePolicy::V4_COMPAT,
        true,
        &NO_MARKS,
    );
    let card = cards::build(&day_of(&[MALEFIC]), &quiet, Some("Tonight!"), &[]).unwrap();
    assert_eq!(card.content, "📅 **Tonight!**\n-# react ✅ in / ❌ out");
    let message = post(&card, &[]).await;
    assert!(!format!("{:?}", message.embeds).contains("<@"), "no tags");
    assert_eq!(message.embeds[0].fields[1].value, "(unnamed)");
}

#[tokio::test]
async fn difficulty_marks_replace_the_written_difficulty() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let marks = DifficultyMarks::new().with("h", "<:diff_h:123>");
    let ctx = redesigned(
        &schedule,
        &roster,
        &table,
        AttendancePolicy::V5,
        false,
        &marks,
    );
    let card = cards::build(&day_of(&[MALEFIC, KALOS]), &ctx, None, &[]).unwrap();
    assert_eq!(
        card.embeds[0].title.as_deref(),
        Some("<:diff_h:123> Radiant Malefic Star + <:diff_h:123> Gatekeeper Kalos")
    );
    assert_eq!(
        card.embeds[1].title.as_deref(),
        Some("Extreme Gatekeeper Kalos"),
        "no mark for x"
    );
}

#[tokio::test]
async fn one_boss_on_two_runs_is_uploaded_once() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let ctx = redesigned(
        &schedule,
        &roster,
        &table,
        AttendancePolicy::V5,
        false,
        &NO_MARKS,
    );
    let card = cards::build(&day_of(&[MALEFIC, RISKY]), &ctx, None, &[]).unwrap();
    let message = post(&card, &[]).await;
    assert_eq!(
        uploads(&message),
        ["MaleficStar.png", "image-MaleficStar.png"]
    );
    let [risky, star] = message.embeds.as_slice() else {
        panic!("two embeds");
    };
    assert_eq!(thumbnail(risky), Some("attachment://MaleficStar.png"));
    assert_eq!(thumbnail(star), Some("attachment://MaleficStar.png"));
    assert!(
        risky
            .description
            .as_deref()
            .unwrap()
            .ends_with("· ❗ at risk")
    );

    // An edit refers to the posted names and uploads nothing.
    let referenced = fetch_art(Some(&art()), &card, false).await;
    let edit = card.edit(&referenced);
    let embeds = edit.embeds.expect("embeds");
    assert_eq!(embeds, message.embeds);
    assert_eq!(
        serde_json::to_value(&edit.allowed_mentions).unwrap(),
        serde_json::json!({"parse": []})
    );
}

/// Twelve runs on one day: nine full embeds, then one listing the rest.
#[tokio::test]
async fn runs_past_ten_embeds_share_a_last_compact_embed() {
    let (mut schedule, roster, table) = (week(), members(), catalog());
    let base = schedule
        .runs
        .iter()
        .find(|run| run.id == KALOS)
        .unwrap()
        .clone();
    let mut run_ids = Vec::new();
    schedule.runs.clear();
    for index in 0..12 {
        let id = format!("{index:08x}-0000-4000-8000-000000000000");
        schedule.runs.push(kanade::domain::schedule::Run {
            id: id.clone(),
            datetime: base.datetime + TimeDelta::minutes(10 * index),
            ..base.clone()
        });
        run_ids.push(id);
    }
    let ctx = redesigned(
        &schedule,
        &roster,
        &table,
        AttendancePolicy::V5,
        false,
        &NO_MARKS,
    );
    let content = IntentContent::DayOf { run_ids };
    let card = cards::build(&content, &ctx, None, &[]).unwrap();
    assert_eq!(card.embeds.len(), MAX_EMBEDS);
    assert!(card.embeds[..9].iter().all(|embed| embed.fields.len() == 3));
    let last = &card.embeds[9];
    assert_eq!(last.title.as_deref(), Some("3 more runs"));
    let lines: Vec<&str> = last.description.as_deref().unwrap().lines().collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[0].contains("Extreme Gatekeeper Kalos"), "{lines:?}");
    assert!(card.embeds[0].image.is_some());
    assert!(card.embeds[1..].iter().all(|embed| embed.image.is_none()));
    let message = post(&card, &[]).await;
    assert_eq!(message.embeds.len(), MAX_EMBEDS);
    assert_eq!(uploads(&message), ["Kalos.png", "image-Kalos.png"]);
}

/// Big parties: the 6000-character budget folds runs into the last embed
/// before Discord would refuse the post.
#[tokio::test]
async fn the_character_budget_moves_runs_into_the_last_embed() {
    let (mut schedule, mut roster, table) = (week(), members(), catalog());
    let party: Vec<String> = (0..40).map(|n| format!("{}", 2000 + n)).collect();
    for user in &party {
        roster.upsert(Member {
            user_id: user.clone(),
            display_name: Some(format!("A rather long member name {user}")),
            has_role: true,
            ping_level: PingLevel::All,
            ..Member::default()
        });
    }
    let base = schedule
        .runs
        .iter()
        .find(|run| run.id == KALOS)
        .unwrap()
        .clone();
    let mut run_ids = Vec::new();
    schedule.runs.clear();
    schedule.rsvps.clear();
    for index in 0..6 {
        let id = format!("{index:08x}-0000-4000-8000-000000000000");
        schedule.runs.push(kanade::domain::schedule::Run {
            id: id.clone(),
            datetime: base.datetime + TimeDelta::minutes(10 * index),
            participants: party.clone(),
            status: RunStatus::Planned,
            ..base.clone()
        });
        run_ids.push(id);
    }
    let ctx = redesigned(
        &schedule,
        &roster,
        &table,
        AttendancePolicy::V5,
        false,
        &NO_MARKS,
    );
    let card = cards::build(&IntentContent::DayOf { run_ids }, &ctx, None, &[]).unwrap();
    let message = post(&card, &[]).await;
    let chars: usize = message
        .embeds
        .iter()
        .map(|embed| {
            let count = |text: Option<&str>| text.map_or(0, |text| text.chars().count());
            count(embed.title.as_deref())
                + count(embed.description.as_deref())
                + count(embed.footer.as_ref().map(|footer| footer.text.as_str()))
                + embed
                    .fields
                    .iter()
                    .map(|field| field.name.chars().count() + field.value.chars().count())
                    .sum::<usize>()
        })
        .sum();
    assert!(chars <= MAX_TOTAL_CHARS, "{chars}");
    assert!(message.embeds.iter().all(|embed| {
        embed
            .fields
            .iter()
            .all(|field| field.value.chars().count() <= 1024)
    }));
    let last = message.embeds.last().unwrap();
    assert!(
        last.title.as_deref().unwrap().ends_with("more runs"),
        "{:?}",
        last.title
    );
}

#[tokio::test]
async fn countdowns_are_coloured_by_state_not_boss() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let ctx = redesigned(
        &schedule,
        &roster,
        &table,
        AttendancePolicy::V5,
        false,
        &NO_MARKS,
    );

    let mentioned = ids(&["1001", "1003"]);
    let card = cards::build(&countdown(MALEFIC), &ctx, None, &mentioned).unwrap();
    let message = post(&card, &mentioned).await;
    assert_eq!(
        message.content.as_deref(),
        Some(
            format!(
                "⏰ **Hard Radiant Malefic Star + Hard Gatekeeper Kalos** <t:{STAR_AT}:R> · Onward! <@1001> <@1003>"
            )
            .as_str()
        )
    );
    let [embed] = message.embeds.as_slice() else {
        panic!("one embed");
    };
    assert_eq!(embed.color, Some(WAITING_AMBER), "someone still to answer");
    assert_eq!(
        embed.description.as_deref(),
        Some(format!("🕘 **<t:{STAR_AT}:t>** · ✅ 1 in · Bex out · waiting on <@1003>").as_str())
    );
    assert_eq!(
        embed.footer.as_ref().map(|footer| footer.text.as_str()),
        Some(REACT_HINT)
    );
    assert_eq!(thumbnail(embed), Some("attachment://MaleficStar.png"));
    assert_eq!(image(embed), None);
    assert_eq!(uploads(&message), ["MaleficStar.png"]);

    let settled = cards::build(&countdown(KALOS), &ctx, Some("Let's roll!"), &[]).unwrap();
    assert!(
        settled.content.contains("· Let's roll! Aria"),
        "{}",
        settled.content
    );
    assert_eq!(settled.embeds[0].colour, SETTLED_GREEN, "everyone answered");
    assert_eq!(
        settled.embeds[0].description.as_deref(),
        Some(format!("🕘 **<t:{KALOS_AT}:t>** · ✅ 2 in").as_str())
    );
    assert_eq!(settled.embeds[0].footer, None);

    let risky = cards::build(&countdown(RISKY), &ctx, None, &[]).unwrap();
    assert_eq!(
        risky.embeds[0].colour, AT_RISK_RED,
        "at risk wins over waiting"
    );

    let cleared = cards::build(&countdown(CLEARED), &ctx, None, &[]).unwrap();
    assert_eq!(cleared.embeds[0].colour, SETTLED_GREEN);
}

#[tokio::test]
async fn the_digest_is_one_line_per_run_with_a_cleared_bar() {
    let (schedule, roster, table) = (week(), members(), catalog());
    let ctx = redesigned(
        &schedule,
        &roster,
        &table,
        AttendancePolicy::V5,
        false,
        &NO_MARKS,
    );
    let content = IntentContent::Digest {
        week_start: week_start(),
        inclusion: inclusion(&schedule),
    };
    let card = cards::build(&content, &ctx, None, &[]).unwrap();
    assert_eq!(card.content, "🗓️ **Let's go!**");
    let [embed] = card.embeds.as_slice() else {
        panic!("one embed");
    };
    assert_eq!(embed.colour, INK_BLUE);
    assert_eq!(
        embed.title.as_deref(),
        Some("Boss week · Thu 03 Sep → Wed 09 Sep")
    );
    assert_eq!(
        embed.description.as_deref(),
        Some("**▰▱▱▱▱  1 of 5 cleared**\n⚠️ 1 waiting on answers · ❗ 1 at risk")
    );
    let days: Vec<(&str, &str, bool)> = embed
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.value.as_str(), field.inline))
        .collect();
    assert_eq!(
        days,
        [
            (
                "Sat 05 Sep",
                "🏁 `21:00`  Hard Gatekeeper Kalos · 2 in · <#222>",
                false
            ),
            (
                "Mon 07 Sep",
                "🕒 own time  Extreme Foo · 0 in, 1 waiting · <#111>",
                false
            ),
            (
                "Tue 08 Sep",
                "❗ `22:00`  Normal Radiant Malefic Star · 0 in, 1 out, 2 waiting · <#111>",
                false
            ),
            (
                "Wed 09 Sep",
                "⚠️ `21:30`  Hard Radiant Malefic Star + Hard Gatekeeper Kalos · 1 in, 1 out, 1 waiting · <#111>\n✅ `23:00`  Extreme Gatekeeper Kalos · 2 in · <#111>",
                false
            ),
        ]
    );
    assert_eq!(
        embed.footer.as_deref(),
        Some("Edited live · /schedule scope:mine for just your runs")
    );
    assert_eq!((&embed.thumbnail, &embed.image), (&None, &None));
}

#[tokio::test]
async fn a_one_channel_digest_names_no_channel_and_an_empty_week_keeps_the_classic_text() {
    let (mut schedule, roster, table) = (week(), members(), catalog());
    for run in &mut schedule.runs {
        run.channel_id = Some("111".into());
    }
    let ctx = redesigned(
        &schedule,
        &roster,
        &table,
        AttendancePolicy::V5,
        false,
        &NO_MARKS,
    );
    let content = IntentContent::Digest {
        week_start: week_start(),
        inclusion: inclusion(&schedule),
    };
    let card = cards::build(&content, &ctx, Some("Busy week!"), &[]).unwrap();
    assert_eq!(card.content, "🗓️ **Busy week!**");
    assert!(
        card.embeds[0]
            .fields
            .iter()
            .all(|field| !field.value.contains("<#")),
        "{:?}",
        card.embeds[0].fields
    );

    let empty = ScheduleSnapshot::default();
    let ctx = redesigned(
        &empty,
        &roster,
        &table,
        AttendancePolicy::V5,
        false,
        &NO_MARKS,
    );
    let content = IntentContent::Digest {
        week_start: week_start(),
        inclusion: Default::default(),
    };
    let card = cards::build(&content, &ctx, None, &[]).unwrap();
    assert_eq!(card.content, "🗓️ **Let's go!**");
    assert_eq!(card.embeds[0].description.as_deref(), Some(DIGEST_EMPTY));
    assert_eq!(card.embeds[0].colour, INK_BLUE);
}
