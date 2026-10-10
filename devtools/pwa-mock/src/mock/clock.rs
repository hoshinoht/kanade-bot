//! Guild-local calendar arithmetic without a date crate (UTC+8, Thursday reset).

use std::{
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};

pub const TZ_OFFSET_SECS: i64 = 8 * 3600;
pub const DOW: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];

/// Unit tests' instant (the e2e suite's Tue 12:00 guild time): seeded
/// deadlines and reset-day rollovers must not follow the wall clock.
const TEST_NOW: &str = "2026-09-29T04:00:00Z";

fn pinned_text() -> Option<String> {
    if cfg!(test) {
        Some(TEST_NOW.into())
    } else {
        std::env::var("KANADE_MOCK_NOW").ok()
    }
}

/// `KANADE_MOCK_NOW` (e.g. `2026-09-29T04:00:00Z`) freezes the clock so tests
/// never depend on the real date; unset follows the system clock.
fn pinned() -> Option<i64> {
    static PINNED: OnceLock<Option<i64>> = OnceLock::new();
    *PINNED.get_or_init(|| pinned_text().and_then(|v| parse_instant(&v)))
}

/// The pinned instant as given, for `/__mock/whoami`; None when unpinned.
pub fn pinned_raw() -> Option<String> {
    pinned().and(pinned_text())
}

pub fn now_secs() -> i64 {
    pinned().unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    })
}

/// Civil date to days since 1970-01-01 (inverse of `civil`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DDTHH:MM:SSZ` only: the one shape the tests use.
pub fn parse_instant(text: &str) -> Option<i64> {
    let b = text.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| text.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, s) = (
        n(0..4)?,
        n(5..7)?,
        n(8..10)?,
        n(11..13)?,
        n(14..16)?,
        n(17..19)?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    Some(days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + s)
}

/// An absolute guild-local minute as the server writes log instants
/// (`2026-09-23T16:34:00Z`, UTC).
pub fn iso_z(local_minute: i64) -> String {
    let secs = local_minute * 60 - TZ_OFFSET_SECS;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        iso_date(days),
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Local day number (days since 1970-01-01) and minute of that day.
pub fn local_now() -> (i64, i64) {
    let local = now_secs() + TZ_OFFSET_SECS;
    (local.div_euclid(86_400), local.rem_euclid(86_400) / 60)
}

/// First day of the boss week containing `day`. 1970-01-01 was a Thursday.
pub fn week_start(day: i64) -> i64 {
    day - day.rem_euclid(7)
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub fn iso_date(days: i64) -> String {
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn iso_now() -> String {
    iso_secs(now_secs())
}

/// A Unix second as an ISO-8601 UTC instant (`2026-09-29T04:03:12Z`).
pub fn iso_secs(secs: i64) -> String {
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        iso_date(days),
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn valid_time(time: &str) -> bool {
    let bytes = time.as_bytes();
    bytes.len() == 5
        && bytes[2] == b':'
        && time[..2].parse::<u8>().is_ok_and(|h| h < 24)
        && time[3..].parse::<u8>().is_ok_and(|m| m < 60)
}

/// `YYYY-MM-DD` in ASCII digits naming a real (proleptic Gregorian) day, as
/// the server's chrono check.
pub fn valid_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit());
    if !shaped {
        return false;
    }
    let n = |range: std::ops::Range<usize>| text[range].parse::<u32>().unwrap_or(0);
    let (y, m, d) = (n(0..4), n(5..7), n(8..10));
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let last = match m {
        2 => 28 + u32::from(leap),
        4 | 6 | 9 | 11 => 30,
        1..=12 => 31,
        _ => 0,
    };
    (1..=last).contains(&d)
}

pub fn minutes(time: &str) -> i64 {
    let h: i64 = time[..2].parse().unwrap_or(0);
    let m: i64 = time[3..].parse().unwrap_or(0);
    h * 60 + m
}

pub fn clock(minutes: i64) -> String {
    let m = minutes.rem_euclid(24 * 60);
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// "in 47 min", "in 2 h 5 min", "in 3 days" (v4's now-strip wording).
pub fn countdown(delta_minutes: i64) -> String {
    match delta_minutes {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_reads_like_v4() {
        assert_eq!(countdown(47), "in 47 min");
        assert_eq!(countdown(125), "in 2 h 5 min");
        assert_eq!(countdown(120), "in 2 h");
        assert_eq!(countdown(3 * 24 * 60 + 5), "in 3 days");
    }

    #[test]
    fn pinned_instants_round_trip() {
        let secs = parse_instant("2026-09-29T04:00:00Z").unwrap();
        assert_eq!(iso_date(secs / 86_400), "2026-09-29");
        assert_eq!(secs % 86_400, 4 * 3600);
        assert_eq!(parse_instant("2026-09-29 04:00"), None);
    }

    #[test]
    fn weeks_start_on_thursday() {
        // 2026-09-24 is a Thursday.
        let thursday = 20_720;
        assert_eq!(iso_date(thursday), "2026-09-24");
        assert_eq!(week_start(thursday + 6), thursday);
    }
}
