//! History parameters: boss weeks, actors and `since` instants, read in the
//! guild zone. Anything else is refused rather than guessed.

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

use crate::domain::{
    history::Actor,
    schedule::{SchedulePolicy, utc_instant},
};

/// `YYYY-MM-DD` with ASCII digits only (chrono alone accepts signs and
/// other widths).
pub fn date(text: &str) -> Option<NaiveDate> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit());
    shaped
        .then(|| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
        .flatten()
}

/// `HH:MM` or `HH:MM:SS`.
fn clock(text: &str) -> Option<NaiveTime> {
    let bytes = text.as_bytes();
    let digits = |range: &[usize]| range.iter().all(|index| bytes[*index].is_ascii_digit());
    match bytes.len() {
        5 if bytes[2] == b':' && digits(&[0, 1, 3, 4]) => {
            NaiveTime::parse_from_str(text, "%H:%M").ok()
        }
        8 if bytes[2] == b':' && bytes[5] == b':' && digits(&[0, 1, 3, 4, 6, 7]) => {
            NaiveTime::parse_from_str(text, "%H:%M:%S").ok()
        }
        _ => None,
    }
}

fn local(policy: &SchedulePolicy, wall: NaiveDateTime) -> Option<DateTime<Utc>> {
    policy
        .zone()
        .from_local_datetime(&wall)
        .earliest()
        .map(|at| at.with_timezone(&Utc))
}

/// A boss week, as the records name it (the week start as an RFC 3339
/// instant) or as the Week read does (the guild-local start date). It must
/// be a week start under the policy.
pub fn week(policy: &SchedulePolicy, text: &str) -> Option<DateTime<Utc>> {
    let at = match date(text) {
        Some(day) => local(policy, day.and_time(policy.reset_time))?,
        None => DateTime::parse_from_rfc3339(text).ok()?.with_timezone(&Utc),
    };
    let start = utc_instant(&policy.week_of(&at).ok()?).ok()?;
    (start == at).then_some(at)
}

/// `kind:id` (`member:1005`, `admin:discord:1`, `system:delivery`).
pub fn actor(text: &str) -> Option<Actor> {
    let (kind, id) = text.split_once(':')?;
    if id.is_empty() {
        return None;
    }
    match kind {
        "member" => Some(Actor::member(id)),
        "admin" => Some(Actor::admin(id)),
        "system" => Some(Actor::system(id)),
        _ => None,
    }
}

/// A bare date (local midnight) or a naive local date-time, in the guild
/// zone (A5-7); offsets, other shapes and wall times in a DST gap are refused.
pub fn since(policy: &SchedulePolicy, text: &str) -> Option<DateTime<Utc>> {
    let wall = match text.split_once('T') {
        None => date(text)?.and_time(NaiveTime::MIN),
        Some((day, time)) => date(day)?.and_time(clock(time)?),
    };
    local(policy, wall)
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveTime, Weekday};

    use super::*;
    use crate::domain::schedule::ReminderPolicy;

    fn policy() -> SchedulePolicy {
        SchedulePolicy::new(
            ReminderPolicy {
                zone: chrono_tz::Asia::Kuala_Lumpur,
                ping_time: NaiveTime::MIN,
                countdowns: Vec::new(),
            },
            Weekday::Thu,
            NaiveTime::MIN,
        )
    }

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn weeks_are_week_starts_in_either_form() {
        let start = utc("2026-09-23T16:00:00+00:00");
        assert_eq!(week(&policy(), "2026-09-24"), Some(start));
        assert_eq!(week(&policy(), "2026-09-23T16:00:00+00:00"), Some(start));
        for bad in [
            "2026-09-25",
            "2026-09-23T17:00:00+00:00",
            "+2026-09-24",
            "this",
            "",
        ] {
            assert_eq!(week(&policy(), bad), None, "{bad}");
        }
    }

    #[test]
    fn since_is_guild_local() {
        assert_eq!(
            since(&policy(), "2026-09-24"),
            Some(utc("2026-09-23T16:00:00+00:00"))
        );
        assert_eq!(
            since(&policy(), "2026-09-24T08:30"),
            Some(utc("2026-09-24T00:30:00+00:00"))
        );
        assert_eq!(
            since(&policy(), "2026-09-24T08:30:15"),
            Some(utc("2026-09-24T00:30:15+00:00"))
        );
        for bad in [
            "2026-09-24T08:30:00+00:00",
            "2026-09-24T08:30Z",
            "24/09/2026",
            "2026-9-24",
            "2026-09-24T8:30",
            "",
        ] {
            assert_eq!(since(&policy(), bad), None, "{bad}");
        }
    }

    #[test]
    fn actors_are_kind_and_id() {
        assert_eq!(actor("member:1005"), Some(Actor::member("1005")));
        assert_eq!(actor("admin:discord:1"), Some(Actor::admin("discord:1")));
        for bad in ["member", "member:", "robot:1", ""] {
            assert_eq!(actor(bad), None, "{bad}");
        }
    }
}
