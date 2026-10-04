//! Change records and their canonical, hash-chained encoding.
//!
//! Canonical encoding (format `kanade.change.v1`): the record without its
//! `hash` as one JSON object, UTF-8, no insignificant whitespace, object keys
//! in ascending byte order at every level, strings escaped as `serde_json`
//! writes them, integers in decimal, instants as UTC ISO text
//! (`domain::time::to_iso`), absent optionals as `null`. The fields are
//! `actor {id, kind}`, `at`, `format`, `id`, `notices`, `prev_hash`,
//! `request_id`, `revision`, `rows [{after, before, key}]`, `seq`, `surface`
//! and `weeks`. `hash` is the lowercase hex SHA-256 of that text, and each
//! record's `prev_hash` is the previous record's `hash` (64 zeros for the
//! genesis record, `seq` 0).

use crate::domain::attendance::{AttendanceDefault, AttendanceRecord, StandingAnswer, StatusPin};
use std::fmt;

use chrono::{DateTime, NaiveTime, Timelike, Utc, Weekday};
use ring::digest::{SHA256, digest};
use serde_json::{Value, json};

use super::blame::{BlameTarget, changed_fields};
use super::origin::{Actor, ChangeMeta, Origin, Surface};
use crate::domain::schedule::{
    FixedRun, Reminder, Rsvp, RsvpSource, RsvpState, Run, RunSource, RunStatus,
};
use crate::domain::time::{from_iso, to_iso};

pub const CHANGE_FORMAT: &str = "kanade.change.v1";
pub const GENESIS_PREV_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

pub fn sha256_hex(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A stored record that cannot be read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordError(pub String);

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unreadable change record: {}", self.0)
    }
}

impl std::error::Error for RecordError {}

fn bad(detail: impl fmt::Display) -> RecordError {
    RecordError(detail.to_string())
}

/// Which row a change touched.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RowKey {
    FixedRun(String),
    Run(String),
    Reminder(String),
    Rsvp { run_id: String, user_id: String },
}

impl RowKey {
    /// The key as the record encodes it (`{table, id}` or the RSVP pair).
    pub fn to_json(&self) -> Value {
        match self {
            Self::FixedRun(id) => json!({"table": "fixed_runs", "id": id}),
            Self::Run(id) => json!({"table": "runs", "id": id}),
            Self::Reminder(id) => json!({"table": "reminders", "id": id}),
            Self::Rsvp { run_id, user_id } => {
                json!({"table": "rsvps", "run_id": run_id, "user_id": user_id})
            }
        }
    }

    fn from_json(value: &Value) -> Result<Self, RecordError> {
        let id = || text(value, "id");
        match text(value, "table")?.as_str() {
            "fixed_runs" => Ok(Self::FixedRun(id()?)),
            "runs" => Ok(Self::Run(id()?)),
            "reminders" => Ok(Self::Reminder(id()?)),
            "rsvps" => Ok(Self::Rsvp {
                run_id: text(value, "run_id")?,
                user_id: text(value, "user_id")?,
            }),
            other => Err(bad(format!("unknown table {other}"))),
        }
    }
}

/// One row's full value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowValue {
    FixedRun(FixedRun),
    Run(Run),
    Reminder(Reminder),
    Rsvp(Rsvp),
}

fn iso(at: &DateTime<Utc>) -> Result<String, RecordError> {
    to_iso(at).map_err(|error| bad(format!("{at:?}: {error}")))
}

fn wall(time: NaiveTime) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        time.hour(),
        time.minute(),
        time.second()
    )
}

fn parse_wall(value: &str) -> Result<NaiveTime, RecordError> {
    let parts: Vec<u32> = value
        .split(':')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(bad)?;
    match parts[..] {
        [hour, minute, second] => NaiveTime::from_hms_opt(hour, minute, second),
        _ => None,
    }
    .ok_or_else(|| bad(format!("time {value}")))
}

fn text(value: &Value, field: &str) -> Result<String, RecordError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| bad(format!("missing {field}")))
}

fn optional_text(value: &Value, field: &str) -> Result<Option<String>, RecordError> {
    match value.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(bad(format!("{field} is not text"))),
    }
}

fn texts(value: &Value, field: &str) -> Result<Vec<String>, RecordError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| bad(format!("missing {field}")))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| bad(format!("{field} holds non-text")))
        })
        .collect()
}

fn instant(value: &Value, field: &str) -> Result<DateTime<Utc>, RecordError> {
    from_iso(&text(value, field)?).map_err(bad)
}

fn optional_instant(value: &Value, field: &str) -> Result<Option<DateTime<Utc>>, RecordError> {
    optional_text(value, field)?
        .map(|text| from_iso(&text).map_err(bad))
        .transpose()
}

fn unsigned(value: &Value, field: &str) -> Result<u64, RecordError> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| bad(format!("missing {field}")))
}

impl RowValue {
    pub fn key(&self) -> RowKey {
        match self {
            Self::FixedRun(row) => RowKey::FixedRun(row.id.clone()),
            Self::Run(row) => RowKey::Run(row.id.clone()),
            Self::Reminder(row) => RowKey::Reminder(row.id.clone()),
            Self::Rsvp(row) => RowKey::Rsvp {
                run_id: row.run_id.clone(),
                user_id: row.user_id.clone(),
            },
        }
    }

    /// The full row as the record encodes it.
    ///
    /// # Errors
    /// [`RecordError`] when an instant is outside the representable years.
    pub fn to_json(&self) -> Result<Value, RecordError> {
        Ok(match self {
            Self::FixedRun(row) => {
                let mut value = json!({
                    "id": row.id,
                    "owner_id": row.owner_id,
                    "channel_id": row.channel_id,
                    "bosses": row.bosses,
                    "weekday": row.weekday.num_days_from_monday(),
                    "time": wall(row.time),
                    "participants": row.participants,
                    "note": row.note,
                });
                // v5 attendance keys appear only when set, so every v4-shaped
                // row (and the golden vector) encodes byte for byte as before.
                if let Value::Object(map) = &mut value {
                    if row.attendance_default != AttendanceDefault::OptIn {
                        map.insert(
                            "attendance_default".into(),
                            row.attendance_default.as_str().into(),
                        );
                    }
                    if !row.standing.is_empty() {
                        let standing = row
                            .standing
                            .iter()
                            .map(|answer| {
                                Ok(json!({
                                    "user_id": answer.user_id,
                                    "set_by": answer.set_by,
                                    "at": iso(&answer.at)?,
                                }))
                            })
                            .collect::<Result<Vec<_>, RecordError>>()?;
                        map.insert("standing".into(), Value::Array(standing));
                    }
                }
                value
            }
            Self::Run(row) => {
                let mut value = json!({
                    "id": row.id,
                    "fixed_run_id": row.fixed_run_id,
                    "channel_id": row.channel_id,
                    "week_start": iso(&row.week_start)?,
                    "datetime": iso(&row.datetime)?,
                    "bosses": row.bosses,
                    "participants": row.participants,
                    "status": row.status.as_str(),
                    "source": row.source.as_str(),
                });
                // Only when recorded, so v4-shaped rows encode as before.
                if let Value::Object(map) = &mut value
                    && !row.attendance.is_empty()
                {
                    let attendance = row
                        .attendance
                        .iter()
                        .map(|record| {
                            Ok(json!({
                                "user_id": record.user_id,
                                "attended": record.attended,
                                "recorded_by": record.recorded_by,
                                "at": iso(&record.at)?,
                            }))
                        })
                        .collect::<Result<Vec<_>, RecordError>>()?;
                    map.insert("attendance".into(), Value::Array(attendance));
                }
                if let Value::Object(map) = &mut value
                    && let Some(pin) = &row.status_pin
                {
                    map.insert(
                        "status_pin".into(),
                        json!({"status": pin.status.as_str(), "at": iso(&pin.at)?}),
                    );
                }
                value
            }
            Self::Reminder(row) => json!({
                "id": row.id,
                "run_id": row.run_id,
                "kind": row.kind,
                "fire_at": iso(&row.fire_at)?,
                "sent_at": row.sent_at.as_ref().map(iso).transpose()?,
                "message_id": row.message_id,
            }),
            Self::Rsvp(row) => json!({
                "run_id": row.run_id,
                "user_id": row.user_id,
                "state": row.state.as_str(),
                "source": row.source.as_str(),
                "at": iso(&row.at)?,
            }),
        })
    }

    fn from_json(key: &RowKey, value: &Value) -> Result<Self, RecordError> {
        Ok(match key {
            RowKey::FixedRun(_) => {
                let day = value
                    .get("weekday")
                    .and_then(Value::as_u64)
                    .and_then(|day| u8::try_from(day).ok())
                    .and_then(|day| Weekday::try_from(day).ok())
                    .ok_or_else(|| bad("weekday"))?;
                let time = parse_wall(&text(value, "time")?)?;
                Self::FixedRun(FixedRun {
                    id: text(value, "id")?,
                    owner_id: text(value, "owner_id")?,
                    channel_id: optional_text(value, "channel_id")?,
                    bosses: texts(value, "bosses")?,
                    weekday: day,
                    time,
                    participants: texts(value, "participants")?,
                    note: optional_text(value, "note")?,
                    attendance_default: match value.get("attendance_default") {
                        None => AttendanceDefault::OptIn,
                        Some(text) => text
                            .as_str()
                            .and_then(AttendanceDefault::parse)
                            .ok_or_else(|| bad("attendance_default"))?,
                    },
                    standing: match value.get("standing") {
                        None => Vec::new(),
                        Some(Value::Array(items)) => items
                            .iter()
                            .map(|item| {
                                Ok(StandingAnswer {
                                    user_id: text(item, "user_id")?,
                                    set_by: text(item, "set_by")?,
                                    at: instant(item, "at")?,
                                })
                            })
                            .collect::<Result<_, RecordError>>()?,
                        Some(_) => return Err(bad("standing")),
                    },
                })
            }
            RowKey::Run(_) => Self::Run(Run {
                id: text(value, "id")?,
                fixed_run_id: optional_text(value, "fixed_run_id")?,
                channel_id: optional_text(value, "channel_id")?,
                week_start: instant(value, "week_start")?,
                datetime: instant(value, "datetime")?,
                bosses: texts(value, "bosses")?,
                participants: texts(value, "participants")?,
                status: RunStatus::parse(&text(value, "status")?).map_err(bad)?,
                source: RunSource::parse(&text(value, "source")?).map_err(bad)?,
                attendance: match value.get("attendance") {
                    None => Vec::new(),
                    Some(Value::Array(items)) => items
                        .iter()
                        .map(|item| {
                            Ok(AttendanceRecord {
                                user_id: text(item, "user_id")?,
                                attended: item
                                    .get("attended")
                                    .and_then(Value::as_bool)
                                    .ok_or_else(|| bad("attended"))?,
                                recorded_by: text(item, "recorded_by")?,
                                at: instant(item, "at")?,
                            })
                        })
                        .collect::<Result<_, RecordError>>()?,
                    Some(_) => return Err(bad("attendance")),
                },
                status_pin: match value.get("status_pin") {
                    None => None,
                    Some(pin) => Some(StatusPin {
                        status: RunStatus::parse(&text(pin, "status")?).map_err(bad)?,
                        at: instant(pin, "at")?,
                    }),
                },
            }),
            RowKey::Reminder(_) => Self::Reminder(Reminder {
                id: text(value, "id")?,
                run_id: text(value, "run_id")?,
                kind: text(value, "kind")?,
                fire_at: instant(value, "fire_at")?,
                sent_at: optional_instant(value, "sent_at")?,
                message_id: optional_text(value, "message_id")?,
            }),
            RowKey::Rsvp { .. } => Self::Rsvp(Rsvp {
                run_id: text(value, "run_id")?,
                user_id: text(value, "user_id")?,
                state: RsvpState::parse(&text(value, "state")?).map_err(bad)?,
                source: RsvpSource::parse(&text(value, "source")?).map_err(bad)?,
                at: instant(value, "at")?,
            }),
        })
    }

    /// The boss week of a run row.
    pub fn run_week(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::Run(row) => Some(row.week_start),
            _ => None,
        }
    }

    /// The run a reminder or RSVP belongs to.
    pub fn owning_run(&self) -> Option<&str> {
        match self {
            Self::Reminder(row) => Some(&row.run_id),
            Self::Rsvp(row) => Some(&row.run_id),
            Self::Run(row) => Some(&row.id),
            Self::FixedRun(_) => None,
        }
    }
}

/// One touched row: `None` means absent before or after.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowChange {
    pub key: RowKey,
    pub before: Option<RowValue>,
    pub after: Option<RowValue>,
}

/// A reference to another record by position and hash (the records a revert
/// undid; reusable for cherry-picks and merges).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChangeRef {
    pub seq: u64,
    pub hash: String,
}

/// One appended history record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeRecord {
    /// Position in the chain; 0 is the genesis record.
    pub seq: u64,
    pub id: String,
    /// The store revision right after this change committed.
    pub revision: u64,
    pub at: DateTime<Utc>,
    pub origin: Origin,
    /// Boss weeks of every run the change touched (or whose rows it touched).
    pub weeks: Vec<DateTime<Utc>>,
    /// Touched rows in key order.
    pub rows: Vec<RowChange>,
    /// Effect kinds of the notices emitted.
    pub notices: Vec<String>,
    /// Records this change refers to (e.g. the ones it reverted).
    pub refs: Vec<ChangeRef>,
    pub prev_hash: String,
    pub hash: String,
}

impl ChangeRecord {
    /// Build and hash the record following the one with `prev` = `(seq,
    /// hash)` (`None` for genesis).
    ///
    /// # Errors
    /// [`RecordError`] when an instant is outside the representable years.
    pub fn seal(
        prev: Option<(u64, &str)>,
        id: String,
        revision: u64,
        meta: ChangeMeta,
        mut weeks: Vec<DateTime<Utc>>,
        mut rows: Vec<RowChange>,
    ) -> Result<Self, RecordError> {
        weeks.sort();
        weeks.dedup();
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        let mut record = Self {
            seq: prev.map_or(0, |(seq, _)| seq + 1),
            id,
            revision,
            at: meta.at,
            origin: meta.origin,
            weeks,
            rows,
            notices: meta.notices,
            refs: meta.refs,
            prev_hash: prev.map_or(GENESIS_PREV_HASH, |(_, hash)| hash).to_owned(),
            hash: String::new(),
        };
        record.hash = record.computed_hash()?;
        Ok(record)
    }

    /// The genesis record of a store at `revision`. It has no clock reading,
    /// so its instant is the Unix epoch.
    ///
    /// # Errors
    /// None in practice: the epoch is representable.
    pub fn genesis(revision: u64) -> Result<Self, RecordError> {
        let meta = ChangeMeta {
            origin: Origin::new(Actor::system("store"), Surface::Import),
            at: DateTime::UNIX_EPOCH,
            notices: Vec::new(),
            refs: Vec::new(),
            request_digest: None,
            expect: Default::default(),
            outbox: Vec::new(),
        };
        Self::seal(
            None,
            "genesis".into(),
            revision,
            meta,
            Vec::new(),
            Vec::new(),
        )
    }

    /// The canonical body (everything but `hash`) as a JSON value.
    ///
    /// # Errors
    /// [`RecordError`] when an instant is outside the representable years.
    pub fn body(&self) -> Result<Value, RecordError> {
        let rows = self
            .rows
            .iter()
            .map(|row| {
                Ok(json!({
                    "key": row.key.to_json(),
                    "before": row.before.as_ref().map(RowValue::to_json).transpose()?,
                    "after": row.after.as_ref().map(RowValue::to_json).transpose()?,
                }))
            })
            .collect::<Result<Vec<Value>, RecordError>>()?;
        let refs: Vec<Value> = self
            .refs
            .iter()
            .map(|r| json!({"seq": r.seq, "hash": r.hash}))
            .collect();
        Ok(json!({
            "format": CHANGE_FORMAT,
            "seq": self.seq,
            "id": self.id,
            "revision": self.revision,
            "at": iso(&self.at)?,
            "actor": {"kind": self.origin.actor.kind(), "id": self.origin.actor.id()},
            "surface": self.origin.surface.as_str(),
            "request_id": self.origin.request_id,
            "weeks": self.weeks.iter().map(iso).collect::<Result<Vec<_>, _>>()?,
            "rows": rows,
            "notices": self.notices,
            "refs": refs,
            "prev_hash": self.prev_hash,
        }))
    }

    /// The canonical text that is hashed and stored.
    ///
    /// # Errors
    /// As [`Self::body`].
    pub fn canonical(&self) -> Result<String, RecordError> {
        Ok(self.body()?.to_string())
    }

    /// # Errors
    /// As [`Self::body`].
    pub fn computed_hash(&self) -> Result<String, RecordError> {
        Ok(sha256_hex(self.canonical()?.as_bytes()))
    }

    /// Whether `body` is the stored form of this record: its bytes hash to
    /// `hash` and it is exactly this record's canonical encoding.
    pub fn check_stored(&self, body: &str) -> Result<(), String> {
        if sha256_hex(body.as_bytes()) != self.hash {
            return Err("stored bytes do not match the hash".into());
        }
        match self.canonical() {
            Ok(canonical) if canonical == body => Ok(()),
            Ok(_) => Err("stored body is not canonical".into()),
            Err(error) => Err(error.to_string()),
        }
    }

    /// This record as a reference.
    pub fn reference(&self) -> ChangeRef {
        ChangeRef {
            seq: self.seq,
            hash: self.hash.clone(),
        }
    }

    /// Read a stored canonical body back; `hash` is the stored hash.
    ///
    /// # Errors
    /// [`RecordError`] for a malformed body.
    pub fn parse(body: &str, hash: &str) -> Result<Self, RecordError> {
        let value: Value = serde_json::from_str(body).map_err(bad)?;
        if value.get("format").and_then(Value::as_str) != Some(CHANGE_FORMAT) {
            return Err(bad("unknown format"));
        }
        let actor = value.get("actor").ok_or_else(|| bad("missing actor"))?;
        let actor = Actor::from_parts(&text(actor, "kind")?, &text(actor, "id")?)
            .ok_or_else(|| bad("unknown actor kind"))?;
        let surface = Surface::parse(&text(&value, "surface")?).ok_or_else(|| bad("surface"))?;
        let weeks = texts(&value, "weeks")?
            .iter()
            .map(|week| from_iso(week).map_err(bad))
            .collect::<Result<_, _>>()?;
        let rows = value
            .get("rows")
            .and_then(Value::as_array)
            .ok_or_else(|| bad("missing rows"))?
            .iter()
            .map(|row| {
                let key = RowKey::from_json(row.get("key").ok_or_else(|| bad("missing key"))?)?;
                let side = |field| match row.get(field) {
                    None | Some(Value::Null) => Ok(None),
                    Some(value) => RowValue::from_json(&key, value).map(Some),
                };
                Ok(RowChange {
                    before: side("before")?,
                    after: side("after")?,
                    key,
                })
            })
            .collect::<Result<_, RecordError>>()?;
        Ok(Self {
            seq: unsigned(&value, "seq")?,
            id: text(&value, "id")?,
            revision: unsigned(&value, "revision")?,
            at: instant(&value, "at")?,
            origin: Origin {
                actor,
                surface,
                request_id: optional_text(&value, "request_id")?,
            },
            weeks,
            rows,
            notices: texts(&value, "notices")?,
            refs: value
                .get("refs")
                .and_then(Value::as_array)
                .ok_or_else(|| bad("missing refs"))?
                .iter()
                .map(|item| {
                    Ok(ChangeRef {
                        seq: unsigned(item, "seq")?,
                        hash: text(item, "hash")?,
                    })
                })
                .collect::<Result<_, RecordError>>()?,
            prev_hash: text(&value, "prev_hash")?,
            hash: hash.to_owned(),
        })
    }

    /// Whether the change touched any row of a run in `week`.
    pub fn touches_week(&self, week: DateTime<Utc>) -> bool {
        self.weeks.contains(&week)
    }

    /// Whether the change set a blamed field of the run (its row or one of
    /// its RSVPs): the records blame's field index names for it, so stores
    /// answer from that index. Reminder-only changes and a run row's removal
    /// (no writer removes runs; reverting a creation cancels) do not count.
    pub fn touches_run(&self, run_id: &str) -> bool {
        changed_fields(self)
            .iter()
            .any(|(target, _)| matches!(target, BlameTarget::Run(id) if id == run_id))
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};
    use serde_json::Map;

    use super::*;

    fn run() -> Run {
        Run {
            id: "r-1".into(),
            fixed_run_id: None,
            channel_id: Some("900".into()),
            week_start: Utc.with_ymd_and_hms(2026, 8, 26, 16, 0, 0).unwrap(),
            datetime: Utc.with_ymd_and_hms(2026, 8, 30, 12, 0, 0).unwrap(),
            bosses: vec!["HFA".into()],
            participants: vec!["1".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
            attendance: Vec::new(),
            status_pin: None,
        }
    }

    /// The golden record: non-ASCII, control characters, nulls and
    /// sub-second instants.
    fn golden_records() -> [ChangeRecord; 2] {
        let genesis = ChangeRecord::genesis(0).expect("genesis");
        let mut moved = run();
        moved.datetime += TimeDelta::microseconds(90 * 60 * 1_000_000 + 250);
        moved.status = RunStatus::Confirmed;
        let rsvp = Rsvp {
            run_id: "r-1".into(),
            user_id: "42".into(),
            state: RsvpState::Maybe,
            source: RsvpSource::Chat,
            at: Utc.with_ymd_and_hms(2026, 8, 29, 1, 2, 3).unwrap()
                + TimeDelta::microseconds(456_789),
        };
        let timing = FixedRun {
            id: "f-ü".into(),
            owner_id: "42".into(),
            channel_id: None,
            bosses: vec!["Kalos ✓".into()],
            weekday: Weekday::Sun,
            time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
            participants: vec![],
            note: Some("tab\there \"quoted\"\u{1}\nline\u{7f}".into()),
            attendance_default: Default::default(),
            standing: Vec::new(),
        };
        let meta = ChangeMeta {
            origin: Origin::new(Actor::member("42"), Surface::PublicPortal)
                .with_request_id("req-ü\u{0}"),
            at: Utc.with_ymd_and_hms(2026, 8, 29, 1, 2, 3).unwrap()
                + TimeDelta::microseconds(456_789),
            notices: vec!["notice.run.move.moved".into()],
            refs: vec![genesis.reference()],
            request_digest: None,
            expect: Default::default(),
            outbox: Vec::new(),
        };
        let record = ChangeRecord::seal(
            Some((genesis.seq, genesis.hash.as_str())),
            "c-1".into(),
            7,
            meta,
            vec![run().week_start],
            vec![
                RowChange {
                    key: RowKey::Run("r-1".into()),
                    before: Some(RowValue::Run(run())),
                    after: Some(RowValue::Run(moved)),
                },
                RowChange {
                    key: RowKey::Rsvp {
                        run_id: "r-1".into(),
                        user_id: "42".into(),
                    },
                    before: None,
                    after: Some(RowValue::Rsvp(rsvp)),
                },
                RowChange {
                    key: RowKey::FixedRun("f-ü".into()),
                    before: Some(RowValue::FixedRun(timing)),
                    after: None,
                },
            ],
        )
        .expect("seal");
        [genesis, record]
    }

    #[test]
    fn golden_records_match_the_pinned_vector() {
        let vector: Value =
            serde_json::from_str(include_str!("../../../docs/v5/vectors/history/golden.json"))
                .expect("vector");
        let pinned = vector["records"].as_array().expect("records");
        let records = golden_records();
        assert_eq!(pinned.len(), records.len());
        for (record, pinned) in records.iter().zip(pinned) {
            let body = pinned["body"].as_str().expect("body");
            let hash = pinned["hash"].as_str().expect("hash");
            assert_eq!(record.canonical().expect("canonical"), body);
            assert_eq!(record.hash, hash);
            assert_eq!(&ChangeRecord::parse(body, hash).expect("parse"), record);
            record.check_stored(body).expect("stored bytes");
        }
        assert_eq!(records[1].prev_hash, records[0].hash);
    }

    #[test]
    fn recorded_attendance_is_encoded_only_when_present_and_round_trips() {
        let plain = RowValue::Run(run()).to_json().expect("json");
        assert!(plain.get("attendance").is_none(), "v4-shaped run row");
        assert!(plain.get("status_pin").is_none(), "v4-shaped run row");
        let mut done = run();
        done.status = RunStatus::Done;
        done.attendance = vec![AttendanceRecord {
            user_id: "1".into(),
            attended: false,
            recorded_by: "admin:root".into(),
            at: Utc.with_ymd_and_hms(2026, 8, 30, 14, 0, 0).unwrap(),
        }];
        done.status_pin = Some(StatusPin {
            status: RunStatus::Confirmed,
            at: Utc.with_ymd_and_hms(2026, 8, 30, 10, 0, 0).unwrap(),
        });
        let value = RowValue::Run(done.clone());
        let json = value.to_json().expect("json");
        assert_eq!(json["attendance"][0]["attended"], Value::Bool(false));
        assert_eq!(
            RowValue::from_json(&RowKey::Run("r-1".into()), &json).expect("decode"),
            value
        );
    }

    #[test]
    fn stored_bytes_must_hash_and_be_canonical() {
        let [_, record] = golden_records();
        let body = record.canonical().expect("canonical");
        let spaced = body.replacen(',', ", ", 1);
        assert_eq!(
            record.check_stored(&spaced).unwrap_err(),
            "stored bytes do not match the hash"
        );
        let mut forged = record.clone();
        forged.hash = sha256_hex(spaced.as_bytes());
        assert_eq!(
            forged.check_stored(&spaced).unwrap_err(),
            "stored body is not canonical"
        );
    }

    /// The canonical encoding needs `serde_json` maps sorted by key; the
    /// `preserve_order` feature would silently change every hash.
    #[test]
    fn serde_json_maps_are_sorted() {
        let mut map = Map::new();
        map.insert("b".into(), Value::Null);
        map.insert("a".into(), Value::Null);
        assert_eq!(Value::Object(map).to_string(), r#"{"a":null,"b":null}"#);
    }

    #[test]
    fn out_of_range_instants_are_refused() {
        let meta = ChangeMeta {
            origin: Origin::new(Actor::admin("root"), Surface::Cli),
            at: DateTime::<Utc>::MAX_UTC,
            notices: Vec::new(),
            refs: Vec::new(),
            request_digest: None,
            expect: Default::default(),
            outbox: Vec::new(),
        };
        let sealed = ChangeRecord::seal(None, "x".into(), 1, meta, Vec::new(), Vec::new());
        assert!(sealed.is_err(), "{sealed:?}");
    }

    #[test]
    #[ignore = "prints the golden vector; run with --ignored --nocapture"]
    fn print_golden_vector() {
        let records: Vec<Value> = golden_records()
            .iter()
            .map(|record| json!({"body": record.canonical().unwrap(), "hash": record.hash}))
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"records": records})).unwrap()
        );
    }
}
