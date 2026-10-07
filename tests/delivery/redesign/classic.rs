//! The classic style is byte-exact: day-of, countdown and digest posts and a
//! day-of edit in both attendance modes, quiet and not, with art, against
//! `classic_cards.json` (captured from the cards before message styles
//! existed; the countdown and digest persona phrase was removed since, as v4
//! never showed one, so a stored phrase must not appear).

use kanade::bot::delivery::cards::{self, Card, fetch_art};
use kanade::domain::attendance::AttendancePolicy;
use kanade::domain::notify::IntentContent;
use serde_json::{Map, Value};

use super::{
    CLEARED, KALOS, MALEFIC, OWN_TIME, RISKY, art, context, edit_json, inclusion, members,
    message_json, week, week_start,
};
use crate::cards::catalog;

const FIXTURE: &str = include_str!("classic_cards.json");

fn day_of(ids: &[&str]) -> IntentContent {
    IntentContent::DayOf {
        run_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
    }
}

fn countdown(id: &str, minutes: i64) -> IntentContent {
    IntentContent::Countdown {
        run_id: id.to_owned(),
        minutes,
    }
}

async fn cases() -> Map<String, Value> {
    let schedule = week();
    let roster = members();
    let table = catalog();
    let art = art();
    let digest = IntentContent::Digest {
        week_start: super::week_start(),
        inclusion: inclusion(&schedule),
    };
    let empty = IntentContent::Digest {
        week_start: week_start(),
        inclusion: Default::default(),
    };
    let cases: Vec<(&str, IntentContent, Option<&str>, Vec<&str>)> = vec![
        (
            "day_of",
            day_of(&[KALOS, MALEFIC]),
            None,
            vec!["1001", "1003"],
        ),
        (
            "day_of_heading",
            day_of(&[MALEFIC, OWN_TIME]),
            Some("Two runs tonight!"),
            vec!["1003"],
        ),
        (
            "countdown_pending",
            countdown(MALEFIC, 15),
            None,
            vec!["1001", "1003"],
        ),
        (
            "countdown_settled",
            countdown(KALOS, 60),
            Some("Onward, Papa~ Let’s charge!"),
            vec!["1001"],
        ),
        ("countdown_at_risk", countdown(RISKY, 90), None, vec![]),
        ("countdown_cleared", countdown(CLEARED, 15), None, vec![]),
        ("digest", digest, Some("Kyahho~ Kirarin V!"), vec![]),
        ("digest_empty", empty, None, vec![]),
    ];
    let mut out = Map::new();
    for (mode, attendance) in [
        ("v4", AttendancePolicy::V4_COMPAT),
        ("v5", AttendancePolicy::V5),
    ] {
        for quiet in [false, true] {
            let ctx = context(&schedule, &roster, &table, attendance, quiet);
            for (name, content, header, mentioned) in &cases {
                let mentioned: Vec<String> = mentioned.iter().map(|id| (*id).to_owned()).collect();
                let card: Card = cards::build(content, &ctx, *header, &mentioned).expect("a card");
                let posted = fetch_art(Some(&art), &card, true).await;
                let referenced = fetch_art(Some(&art), &card, false).await;
                let key = format!("{mode}/{}/{name}", if quiet { "quiet" } else { "loud" });
                out.insert(
                    format!("{key}/post"),
                    message_json(&card.message(&mentioned, &posted)),
                );
                out.insert(format!("{key}/edit"), edit_json(&card.edit(&referenced)));
            }
        }
    }
    out
}

#[tokio::test]
async fn classic_cards_are_byte_exact() {
    let actual = Value::Object(cases().await);
    let expected: Value = serde_json::from_str(FIXTURE).expect("fixture");
    let (Value::Object(expected), Value::Object(actual)) = (&expected, &actual) else {
        unreachable!()
    };
    assert_eq!(expected.len(), actual.len(), "case count");
    for (key, want) in expected {
        assert_eq!(actual.get(key), Some(want), "{key}");
    }
}
