//! Words, marks and colours every redesigned card shares.

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::bot::cards::format;
use crate::domain::attendance::AnswerState;
use crate::domain::catalog::BossTable;
use crate::domain::settings::MessageStyle;

use super::super::common::People;

/// Kanade's own summaries (the digest), and a day-of run whose lead boss
/// has no catalog colour.
pub const INK_BLUE: u32 = 0x4D5C9E;
/// Everyone has answered.
pub const SETTLED_GREEN: u32 = 0x3BA55C;
/// Someone is still to answer.
pub const WAITING_AMBER: u32 = 0xF0B232;
/// The run is at risk.
pub const AT_RISK_RED: u32 = 0xDA373C;

/// The live message style; serve reads the saved setting per card.
pub type StyleSource = Arc<dyn Fn() -> MessageStyle + Send + Sync>;

/// Difficulty letter (lowercase) → Discord emoji markup (`<:name:id>`).
/// Empty until the application emojis are listed; a missing letter falls
/// back to the written difficulty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DifficultyMarks(BTreeMap<String, String>);

/// No marks: every label is written out.
pub static NO_MARKS: DifficultyMarks = DifficultyMarks::new();

impl DifficultyMarks {
    pub const fn new() -> Self {
        Self(BTreeMap::new())
    }

    #[must_use]
    pub fn with(mut self, letter: &str, markup: &str) -> Self {
        self.insert(letter, markup);
        self
    }

    pub fn insert(&mut self, letter: &str, markup: &str) {
        self.0.insert(letter.to_lowercase(), markup.to_owned());
    }

    pub fn get(&self, letter: &str) -> Option<&str> {
        self.0.get(&letter.to_lowercase()).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `HMaleficStar` → `Hard Radiant Malefic Star`, or `<:diff_h:1> Radiant
/// Malefic Star` with a mark for `h`. Off the catalog: the proposal cards'
/// `Hard MaleficStar`, else the token.
pub fn boss_label(token: &str, catalog: Option<&BossTable>, marks: &DifficultyMarks) -> String {
    let Some((difficulty, boss)) = catalog.and_then(|table| table.split(token)) else {
        return format::boss_label(token);
    };
    match marks.get(difficulty.letter()) {
        Some(mark) => format!("{mark} {}", boss.full()),
        None => format!("{} {}", difficulty.label(), boss.full()),
    }
}

/// Every boss labelled, joined with ` + `.
pub fn boss_labels(
    tokens: &[String],
    catalog: Option<&BossTable>,
    marks: &DifficultyMarks,
) -> String {
    if tokens.is_empty() {
        return "(no bosses)".to_owned();
    }
    tokens
        .iter()
        .map(|token| boss_label(token, catalog, marks))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// `<t:…:t>`: the clock in each reader's own zone.
pub fn short_time(at: DateTime<Utc>) -> String {
    format!("<t:{}:t>", at.timestamp())
}

/// `<t:…:R>`: a live "in 2 hours".
pub fn relative_time(at: DateTime<Utc>) -> String {
    format!("<t:{}:R>", at.timestamp())
}

/// `<t:…:F>`: weekday, date and clock.
pub fn full_time(at: DateTime<Utc>) -> String {
    format!("<t:{}:F>", at.timestamp())
}

/// A small grey line under the content.
pub fn subtext(line: &str) -> String {
    format!("-# {line}")
}

/// A run's party by answer, each rendered as the card names them: In
/// (explicit or assumed, the assumed marked), Waiting (unknown), Out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roster {
    pub in_: Vec<String>,
    pub waiting: Vec<String>,
    pub out: Vec<String>,
}

pub fn roster(people: &People, states: &[(String, AnswerState)]) -> Roster {
    let mut split = Roster::default();
    for (user, state) in states {
        let name = people.each(std::slice::from_ref(user)).remove(0);
        match state {
            AnswerState::Confirmed => split.in_.push(name),
            AnswerState::Assumed(_) => split.in_.push(format!("{name} (assumed)")),
            AnswerState::Unknown => split.waiting.push(name),
            AnswerState::Declined => split.out.push(name),
        }
    }
    split
}

/// Names joined `, `, or `—` for nobody.
pub fn names(list: &[String]) -> String {
    if list.is_empty() {
        "—".to_owned()
    } else {
        list.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::domain::catalog::{BossSpec, CatalogSpec, DifficultySpec};

    fn catalog() -> BossTable {
        let difficulty = |prefix: &str, label: &str| DifficultySpec {
            prefix: prefix.into(),
            label: label.into(),
        };
        BossTable::from_spec(&CatalogSpec {
            difficulties: vec![difficulty("h", "Hard"), difficulty("x", "Extreme")],
            bosses: vec![
                BossSpec {
                    short: "MaleficStar".into(),
                    full: Some("Radiant Malefic Star".into()),
                    ..BossSpec::default()
                },
                BossSpec {
                    short: "Kalos".into(),
                    full: Some("Gatekeeper Kalos".into()),
                    ..BossSpec::default()
                },
            ],
        })
        .expect("catalog")
    }

    #[test]
    fn labels_use_the_catalog_name_and_a_mark_when_one_exists() {
        let table = catalog();
        let none = DifficultyMarks::new();
        assert_eq!(
            boss_label("HMaleficStar", Some(&table), &none),
            "Hard Radiant Malefic Star"
        );
        assert_eq!(
            boss_label("XKalos", Some(&table), &none),
            "Extreme Gatekeeper Kalos"
        );
        let marks = DifficultyMarks::new().with("H", "<:diff_h:123>");
        assert_eq!(marks.get("h"), Some("<:diff_h:123>"));
        assert_eq!(
            boss_labels(
                &["HMaleficStar".into(), "XKalos".into()],
                Some(&table),
                &marks
            ),
            "<:diff_h:123> Radiant Malefic Star + Extreme Gatekeeper Kalos"
        );
        // Off the catalog: the written difficulty, else the token.
        assert_eq!(boss_label("XFoo", Some(&table), &marks), "Extreme Foo");
        assert_eq!(boss_label("HKalos", None, &marks), "Hard Kalos");
        assert_eq!(boss_label("Gollux", Some(&table), &none), "Gollux");
        assert_eq!(boss_labels(&[], Some(&table), &none), "(no bosses)");
        assert!(NO_MARKS.is_empty());
    }

    #[test]
    fn timestamps_and_subtext() {
        let at = Utc.with_ymd_and_hms(2026, 9, 9, 13, 30, 0).unwrap();
        assert_eq!(short_time(at), "<t:1788960600:t>");
        assert_eq!(relative_time(at), "<t:1788960600:R>");
        assert_eq!(full_time(at), "<t:1788960600:F>");
        assert_eq!(subtext("react ✅"), "-# react ✅");
    }

    #[test]
    fn nobody_reads_as_a_dash() {
        assert_eq!(names(&[]), "—");
        assert_eq!(names(&["Aria".into(), "Bex".into()]), "Aria, Bex");
    }
}
