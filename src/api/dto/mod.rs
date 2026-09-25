//! Wire shapes of the admin reads (`docs/v5/api-schemas`, A0) and the
//! projections from domain rows. Guild-local text is built by hand because
//! chrono runs without its formatting features here.

pub mod bosses;
pub mod config;
pub mod fixed;
pub mod history;
pub mod inbox;
pub mod logs;
pub mod members;
pub mod reminders;
pub mod rescan;
pub mod week;

use std::path::Path;

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use serde::Serialize;

use super::assets::art_file;
use crate::domain::catalog::BossTable;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const LETTERS: [&str; 5] = ["e", "n", "h", "c", "x"];

pub fn dow(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

pub fn weekday_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

pub fn iso_date(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

pub fn hhmm(time: impl Timelike) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// `2026-09-29T04:00:00Z`.
pub fn iso_instant(at: DateTime<Utc>) -> String {
    format!(
        "{}T{}:{:02}Z",
        iso_date(at.date_naive()),
        hhmm(at),
        at.second()
    )
}

/// "Tue 29 Sep 21:00" in the guild zone.
pub fn when(at: DateTime<Utc>, zone: Tz) -> String {
    let local = zone.from_utc_datetime(&at.naive_utc());
    format!(
        "{} {:02} {} {}",
        dow(local.weekday()),
        local.day(),
        MONTHS[local.month0() as usize],
        hhmm(local)
    )
}

/// "in 47 min", "in 2 h 5 min", "in 3 days" (v4's now-strip wording).
pub fn countdown(minutes: i64) -> String {
    match minutes.max(0) {
        m if m < 60 => format!("in {m} min"),
        m if m < 24 * 60 => match m % 60 {
            0 => format!("in {} h", m / 60),
            r => format!("in {} h {r} min", m / 60),
        },
        m => match m / (24 * 60) {
            1 => "in 1 day".into(),
            d => format!("in {d} days"),
        },
    }
}

/// `common.json#/$defs/Boss`.
#[derive(Clone, Debug, Serialize)]
pub struct Boss {
    pub token: String,
    pub key: String,
    pub name: String,
    pub difficulty: String,
    pub level: Option<u64>,
    pub portrait: Option<String>,
    pub portrait_sm: Option<String>,
    pub art: Option<String>,
    pub hue: u16,
}

/// Hue (0..=359) of an RGB colour; greys and absent colours are 0.
pub fn hue(colour: Option<u32>) -> u16 {
    let Some(rgb) = colour else { return 0 };
    let [r, g, b] = [(rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff].map(|c| c as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let delta = max - min;
    if delta <= f64::EPSILON {
        return 0;
    }
    let degrees = if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    (degrees.round() as u16) % 360
}

/// Art URLs for a catalog boss; `None` where no file exists ("absent means absent").
pub struct Art<'a> {
    pub root: Option<&'a Path>,
}

impl Art<'_> {
    pub fn url(&self, kind: &str, key: &str, basename: &str) -> Option<String> {
        art_file(self.root, kind, basename).map(|_| format!("/art/{kind}/{key}"))
    }
}

/// Project stored tokens; a token the catalog no longer knows is shown by its
/// text when its first letter is a difficulty, and dropped otherwise.
pub fn bosses(catalog: &BossTable, art: &Art<'_>, tokens: &[String]) -> Vec<Boss> {
    tokens
        .iter()
        .filter_map(|token| boss(catalog, art, token))
        .collect()
}

pub fn boss(catalog: &BossTable, art: &Art<'_>, token: &str) -> Option<Boss> {
    match catalog.split(token) {
        Some((difficulty, entry)) => {
            let key = entry.short();
            let basename = entry.portrait().unwrap_or(key);
            let portrait = art.url("portraits", key, basename);
            Some(Boss {
                token: token.to_owned(),
                key: key.to_owned(),
                name: entry.full().to_owned(),
                difficulty: difficulty.letter().to_owned(),
                level: entry.level(),
                portrait_sm: art.url("icons", key, basename).or_else(|| portrait.clone()),
                portrait,
                art: art.url("entry", key, basename),
                hue: hue(entry.guide_colour()),
            })
        }
        None => {
            let letter = token.get(..1)?.to_ascii_lowercase();
            LETTERS.contains(&letter.as_str()).then(|| Boss {
                token: token.to_owned(),
                key: token[1..].to_owned(),
                name: token.to_owned(),
                difficulty: letter,
                level: None,
                portrait: None,
                portrait_sm: None,
                art: None,
                hue: 0,
            })
        }
    }
}

/// `{id, name}` for members and channels.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Named {
    pub id: String,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_helpers_read_like_v4() {
        assert_eq!(countdown(47), "in 47 min");
        assert_eq!(countdown(125), "in 2 h 5 min");
        assert_eq!(countdown(120), "in 2 h");
        assert_eq!(countdown(3 * 24 * 60 + 5), "in 3 days");
        let at = Utc.with_ymd_and_hms(2026, 9, 29, 13, 0, 0).unwrap();
        assert_eq!(when(at, chrono_tz::Asia::Kuala_Lumpur), "Tue 29 Sep 21:00");
        assert_eq!(iso_instant(at), "2026-09-29T13:00:00Z");
        assert_eq!(hue(Some(0xFF0000)), 0);
        assert_eq!(hue(Some(0x00FF00)), 120);
        assert_eq!(hue(Some(0x0000FF)), 240);
        assert_eq!(hue(Some(0x39BFFF)), 199);
        assert_eq!(hue(None), 0);
    }
}
