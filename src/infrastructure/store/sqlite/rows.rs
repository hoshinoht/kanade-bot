//! Column encodings for scheduler rows: canonical UTC ISO instants, JSON
//! string lists, v4 enum spellings, `HH:MM` wall times and 0=Monday weekdays.

use chrono::{DateTime, NaiveTime, Timelike, Utc, Weekday};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use crate::domain::attendance::{
    AttendanceDefault, AttendanceRecord, PastRun, StandingAnswer, StatusPin,
};
use crate::domain::schedule::{
    FixedRun, Reminder, Rsvp, RsvpSource, RsvpState, Run, RunSource, RunStatus,
};
use crate::domain::scheduler::StoreError;
use crate::domain::time::{from_iso, to_iso};

pub(super) const FIXED_COLUMNS: &str =
    "id, owner_id, channel_id, bosses, weekday, time, participants, note, attendance_default";
pub(super) const RUN_COLUMNS: &str =
    "id, fixed_run_id, channel_id, week_start, datetime, bosses, participants, status, source";
pub(super) const REMINDER_COLUMNS: &str = "id, run_id, kind, fire_at, sent_at, message_id";
pub(super) const RSVP_COLUMNS: &str = "run_id, user_id, state, source, at";

fn corrupt(detail: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(format!("stored row is unreadable: {detail}"))
}

pub(super) fn instant(at: &DateTime<Utc>) -> Result<String, StoreError> {
    to_iso(at).map_err(|error| StoreError::Constraint(format!("{at:?}: {error}")))
}

pub(super) fn optional_instant(at: Option<&DateTime<Utc>>) -> Result<Option<String>, StoreError> {
    at.map(instant).transpose()
}

pub(super) fn list(items: &[String]) -> String {
    // Serialising strings cannot fail.
    serde_json::to_string(items).unwrap_or_default()
}

pub(super) fn wall_time(time: NaiveTime) -> Result<String, StoreError> {
    if time.nanosecond() != 0 {
        return Err(StoreError::Constraint(format!(
            "weekly time {time} has sub-second precision"
        )));
    }
    Ok(if time.second() == 0 {
        format!("{:02}:{:02}", time.hour(), time.minute())
    } else {
        format!(
            "{:02}:{:02}:{:02}",
            time.hour(),
            time.minute(),
            time.second()
        )
    })
}

pub(super) fn weekday(day: Weekday) -> i64 {
    i64::from(day.num_days_from_monday())
}

fn text(row: &SqliteRow, column: &str) -> Result<String, StoreError> {
    row.try_get(column).map_err(corrupt)
}

fn optional_text(row: &SqliteRow, column: &str) -> Result<Option<String>, StoreError> {
    row.try_get(column).map_err(corrupt)
}

fn read_instant(row: &SqliteRow, column: &str) -> Result<DateTime<Utc>, StoreError> {
    from_iso(&text(row, column)?).map_err(corrupt)
}

fn read_optional_instant(
    row: &SqliteRow,
    column: &str,
) -> Result<Option<DateTime<Utc>>, StoreError> {
    optional_text(row, column)?
        .map(|value| from_iso(&value).map_err(corrupt))
        .transpose()
}

fn read_list(row: &SqliteRow, column: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(&text(row, column)?).map_err(corrupt)
}

fn read_wall_time(row: &SqliteRow) -> Result<NaiveTime, StoreError> {
    let value = text(row, "time")?;
    let parts: Vec<u32> = value
        .split(':')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(corrupt)?;
    let time = match parts[..] {
        [hour, minute] => NaiveTime::from_hms_opt(hour, minute, 0),
        [hour, minute, second] => NaiveTime::from_hms_opt(hour, minute, second),
        _ => None,
    };
    time.ok_or_else(|| corrupt(format!("weekly time {value:?}")))
}

fn read_weekday(row: &SqliteRow) -> Result<Weekday, StoreError> {
    let day: i64 = row.try_get("weekday").map_err(corrupt)?;
    u8::try_from(day)
        .ok()
        .and_then(|day| Weekday::try_from(day).ok())
        .ok_or_else(|| corrupt(format!("weekday {day}")))
}

pub(super) fn fixed_run(row: &SqliteRow) -> Result<FixedRun, StoreError> {
    Ok(FixedRun {
        id: text(row, "id")?,
        owner_id: text(row, "owner_id")?,
        channel_id: optional_text(row, "channel_id")?,
        bosses: read_list(row, "bosses")?,
        weekday: read_weekday(row)?,
        time: read_wall_time(row)?,
        participants: read_list(row, "participants")?,
        note: optional_text(row, "note")?,
        attendance_default: {
            let value = text(row, "attendance_default")?;
            AttendanceDefault::parse(&value)
                .ok_or_else(|| corrupt(format!("attendance default {value:?}")))?
        },
        // Attached from `standing_answers` by the caller.
        standing: Vec::new(),
    })
}

/// Every standing answer, by timing then user.
pub(super) async fn standing_answers(
    conn: &mut sqlx::SqliteConnection,
    fixed_id: Option<&str>,
) -> Result<Vec<(String, StandingAnswer)>, StoreError> {
    let found = sqlx::query(
        "SELECT fixed_run_id, user_id, set_by, at FROM standing_answers \
         WHERE ?1 IS NULL OR fixed_run_id = ?1 ORDER BY fixed_run_id, user_id",
    )
    .bind(fixed_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(corrupt)?;
    found
        .iter()
        .map(|row| {
            Ok((
                text(row, "fixed_run_id")?,
                StandingAnswer {
                    user_id: text(row, "user_id")?,
                    set_by: text(row, "set_by")?,
                    at: read_instant(row, "at")?,
                },
            ))
        })
        .collect()
}

/// Put each timing's standing answers on it.
pub(super) async fn attach_standing(
    conn: &mut sqlx::SqliteConnection,
    fixed_runs: &mut [FixedRun],
    fixed_id: Option<&str>,
) -> Result<(), StoreError> {
    for (id, answer) in standing_answers(conn, fixed_id).await? {
        if let Some(row) = fixed_runs.iter_mut().find(|row| row.id == id) {
            row.standing.push(answer);
        }
    }
    Ok(())
}

pub(super) fn run(row: &SqliteRow) -> Result<Run, StoreError> {
    Ok(Run {
        id: text(row, "id")?,
        fixed_run_id: optional_text(row, "fixed_run_id")?,
        channel_id: optional_text(row, "channel_id")?,
        week_start: read_instant(row, "week_start")?,
        datetime: read_instant(row, "datetime")?,
        bosses: read_list(row, "bosses")?,
        participants: read_list(row, "participants")?,
        status: RunStatus::parse(&text(row, "status")?).map_err(corrupt)?,
        source: RunSource::parse(&text(row, "source")?).map_err(corrupt)?,
        // Attached from `run_attendance` by the caller.
        attendance: Vec::new(),
        status_pin: None,
    })
}

fn attendance_record(row: &SqliteRow) -> Result<AttendanceRecord, StoreError> {
    let attended: i64 = row.try_get("attended").map_err(corrupt)?;
    Ok(AttendanceRecord {
        user_id: text(row, "user_id")?,
        attended: attended != 0,
        recorded_by: text(row, "recorded_by")?,
        at: read_instant(row, "at")?,
    })
}

/// Put each run's recorded attendance on it; `run_ids` is a JSON array of
/// the runs to fill.
pub(super) async fn attach_attendance(
    conn: &mut sqlx::SqliteConnection,
    runs: &mut [Run],
    run_ids: &str,
) -> Result<(), StoreError> {
    let found = sqlx::query(
        "SELECT run_id, user_id, attended, recorded_by, at FROM run_attendance \
         WHERE run_id IN (SELECT value FROM json_each(?1)) ORDER BY run_id, user_id",
    )
    .bind(run_ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(corrupt)?;
    for row in &found {
        let run_id = text(row, "run_id")?;
        if let Some(run) = runs.iter_mut().find(|run| run.id == run_id) {
            run.attendance.push(attendance_record(row)?);
        }
    }
    Ok(())
}

/// Put each run's status pin on it; `run_ids` is a JSON array.
pub(super) async fn attach_status_pins(
    conn: &mut sqlx::SqliteConnection,
    runs: &mut [Run],
    run_ids: &str,
) -> Result<(), StoreError> {
    let found = sqlx::query(
        "SELECT run_id, status, at FROM run_status_pins \
         WHERE run_id IN (SELECT value FROM json_each(?1))",
    )
    .bind(run_ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(corrupt)?;
    for row in &found {
        let run_id = text(row, "run_id")?;
        if let Some(run) = runs.iter_mut().find(|run| run.id == run_id) {
            run.status_pin = Some(StatusPin {
                status: RunStatus::parse(&text(row, "status")?).map_err(corrupt)?,
                at: read_instant(row, "at")?,
            });
        }
    }
    Ok(())
}

/// One member's recorded attendance on done runs, newest first, at most
/// `per_timing` per weekly timing (one-off runs share one group).
pub(super) async fn member_history(
    conn: &mut sqlx::SqliteConnection,
    member: &str,
    per_timing: usize,
) -> Result<Vec<PastRun>, StoreError> {
    let limit = i64::try_from(per_timing).unwrap_or(i64::MAX);
    let found = sqlx::query(
        "SELECT run_id, fixed_run_id, datetime, attended, state FROM ( \
             SELECT r.id AS run_id, r.fixed_run_id, r.datetime, a.attended, v.state, \
                    ROW_NUMBER() OVER ( \
                        PARTITION BY r.fixed_run_id ORDER BY r.datetime DESC, r.id DESC \
                    ) AS recent \
             FROM run_attendance a \
             JOIN runs r ON r.id = a.run_id \
             LEFT JOIN rsvps v ON v.run_id = a.run_id AND v.user_id = a.user_id \
             WHERE a.user_id = ?1 AND r.status = 'done' \
         ) WHERE recent <= ?2 ORDER BY datetime DESC, run_id DESC",
    )
    .bind(member)
    .bind(limit)
    .fetch_all(&mut *conn)
    .await
    .map_err(corrupt)?;
    found
        .iter()
        .map(|row| {
            let attended: i64 = row.try_get("attended").map_err(corrupt)?;
            Ok(PastRun {
                run_id: text(row, "run_id")?,
                fixed_run_id: optional_text(row, "fixed_run_id")?,
                at: read_instant(row, "datetime")?,
                explicit: optional_text(row, "state")?
                    .map(|state| RsvpState::parse(&state).map_err(corrupt))
                    .transpose()?,
                attended: attended != 0,
            })
        })
        .collect()
}

pub(super) fn reminder(row: &SqliteRow) -> Result<Reminder, StoreError> {
    Ok(Reminder {
        id: text(row, "id")?,
        run_id: text(row, "run_id")?,
        kind: text(row, "kind")?,
        fire_at: read_instant(row, "fire_at")?,
        sent_at: read_optional_instant(row, "sent_at")?,
        message_id: optional_text(row, "message_id")?,
    })
}

pub(super) fn rsvp(row: &SqliteRow) -> Result<Rsvp, StoreError> {
    Ok(Rsvp {
        run_id: text(row, "run_id")?,
        user_id: text(row, "user_id")?,
        state: RsvpState::parse(&text(row, "state")?).map_err(corrupt)?,
        source: RsvpSource::parse(&text(row, "source")?).map_err(corrupt)?,
        at: read_instant(row, "at")?,
    })
}
