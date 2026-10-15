use std::sync::LazyLock;

use chrono::{NaiveDate, NaiveTime};
use regex::Regex;

use crate::extract::text::{SPACE, pattern};

/// Which side of the stated instant a run must be on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// At or after it.
    After,
    /// Strictly before it.
    Before,
}

/// "after 0930 17th": when a run may be, not a time to move it to. Holds only
/// what was said: no implicit day for a bare time, no roll past the anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bound {
    pub side: Side,
    pub day: Option<NaiveDate>,
    pub clock: Option<NaiveTime>,
}

/// Negated forms come first, so the leftmost-first match reads "not before"
/// as `After` and "no later than" as `Before`. `by` stays a target ("by 9").
pub(super) static BOUND_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    let s = SPACE;
    pattern(&format!(
        r"(?i)\b(?:(no{s}+earlier{s}+than|not{s}+before)|(no{s}+later{s}+than|not{s}+after)|(after|later{s}+than)|(before|earlier{s}+than))\b"
    ))
});

fn sides(text: &str) -> impl Iterator<Item = Side> + '_ {
    BOUND_WORDS.captures_iter(text).map(|found| {
        if found.get(1).is_some() || found.get(3).is_some() {
            Side::After
        } else {
            Side::Before
        }
    })
}

/// The one side every bound word in `day_ref` and `time_ref` agrees on;
/// `None` without any, or for a window ("after 9 before 11").
pub(super) fn parse_side(day_ref: Option<&str>, time_ref: Option<&str>) -> Option<Side> {
    let mut found = None;
    for side in [day_ref, time_ref].into_iter().flatten().flat_map(sides) {
        match found {
            None => found = Some(side),
            Some(seen) if seen != side => return None,
            Some(_) => {}
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bound_words_pick_a_side() {
        for (day, time, side) in [
            (Some("17th"), Some("after 0930"), Some(Side::After)),
            (Some("after sat"), None, Some(Side::After)),
            (None, Some("later than 10"), Some(Side::After)),
            (None, Some("no earlier than 10"), Some(Side::After)),
            (None, Some("not before 10"), Some(Side::After)),
            (None, Some("before 11"), Some(Side::Before)),
            (None, Some("earlier than 11"), Some(Side::Before)),
            (None, Some("no later than 11"), Some(Side::Before)),
            (Some("before sat"), Some("after 9"), None),
            (None, Some("by 9"), None),
            (None, Some("9pm"), None),
            (None, None, None),
        ] {
            assert_eq!(parse_side(day, time), side, "{day:?} {time:?}");
        }
    }
}
