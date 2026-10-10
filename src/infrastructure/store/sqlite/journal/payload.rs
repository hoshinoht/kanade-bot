//! `notice_outbox.payload`: a [`Notice`] as versioned JSON. Existing kinds
//! retain their v1 bytes; only fixed-timing add/remove use v2.

use chrono::{DateTime, NaiveTime, Timelike, Utc, Weekday};
use serde_json::{Value, json};

use crate::domain::schedule::{FixedField, Notice, NoticeChange, RequestDecision, RunStatus};
use crate::domain::time::{from_iso, to_iso};

const V1: i64 = 1;
const V2: i64 = 2;

const FIELDS: &[FixedField] = &[
    FixedField::OwnerId,
    FixedField::ChannelId,
    FixedField::Bosses,
    FixedField::Weekday,
    FixedField::Time,
    FixedField::Participants,
    FixedField::Note,
];

const WEEKDAYS: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

const DECISIONS: &[RequestDecision] = &[
    RequestDecision::Approved,
    RequestDecision::Rejected,
    RequestDecision::Expired,
];

fn instant(at: &DateTime<Utc>) -> Result<String, String> {
    to_iso(at).map_err(|error| error.to_string())
}

/// # Errors
/// An instant outside v4's representable years.
pub(super) fn encode(notice: &Notice) -> Result<String, String> {
    let version = match &notice.change {
        NoticeChange::FixedAdded { .. } | NoticeChange::FixedRemoved { .. } => V2,
        _ => V1,
    };
    let change = match &notice.change {
        NoticeChange::RunStatus { run_id, from, to } => json!({
            "type": "run_status", "run_id": run_id, "from": from.as_str(), "to": to.as_str(),
        }),
        NoticeChange::RunMoved { run_id, from, to } => json!({
            "type": "run_moved", "run_id": run_id, "from": instant(from)?, "to": instant(to)?,
        }),
        NoticeChange::RunReset { run_id, from, to } => json!({
            "type": "run_reset", "run_id": run_id, "from": instant(from)?, "to": instant(to)?,
        }),
        NoticeChange::RunSwapped {
            run_id,
            participants,
            leaving,
            joining,
        } => json!({
            "type": "run_swapped", "run_id": run_id, "participants": participants,
            "leaving": leaving, "joining": joining,
        }),
        NoticeChange::Rollback {
            reverted,
            cancelled_runs,
            run_ids,
            checkpoint,
        } => json!({
            "type": "rollback", "reverted": reverted, "cancelled_runs": cancelled_runs,
            "run_ids": run_ids, "checkpoint": checkpoint,
        }),
        NoticeChange::Merged {
            draft,
            version,
            title,
            run_ids,
            fixed_ids,
        } => json!({
            "type": "merged", "draft": draft, "version": version, "title": title,
            "run_ids": run_ids, "fixed_ids": fixed_ids,
        }),
        NoticeChange::RequestDecided {
            request,
            decision,
            reason,
        } => json!({
            "type": "request_decided", "request": request, "decision": decision.as_str(),
            "reason": reason,
        }),
        NoticeChange::FixedChanged {
            fixed_id,
            fields,
            weekday,
            time,
            participants,
        } => json!({
            "type": "fixed_changed", "fixed_id": fixed_id,
            "fields": fields.iter().map(|field| field.as_str()).collect::<Vec<_>>(),
            "weekday": weekday.num_days_from_monday(),
            "seconds": time.num_seconds_from_midnight(),
            "nanos": time.nanosecond(),
            "participants": participants,
        }),
        NoticeChange::FixedAdded {
            fixed_id,
            bosses,
            weekday,
            time,
            participants,
        } => json!({
            "type": "fixed_added", "fixed_id": fixed_id, "bosses": bosses,
            "weekday": weekday.num_days_from_monday(),
            "seconds": time.num_seconds_from_midnight(),
            "nanos": time.nanosecond(), "participants": participants,
        }),
        NoticeChange::FixedRemoved {
            fixed_id,
            bosses,
            weekday,
            time,
            participants,
            cancelled_runs,
        } => json!({
            "type": "fixed_removed", "fixed_id": fixed_id, "bosses": bosses,
            "weekday": weekday.num_days_from_monday(),
            "seconds": time.num_seconds_from_midnight(),
            "nanos": time.nanosecond(), "participants": participants,
            "cancelled_runs": cancelled_runs,
        }),
    };
    Ok(json!({
        "v": version,
        "change": change,
        "channel_id": notice.channel_id,
        "listed": notice.listed,
        "via_portal": notice.via_portal,
    })
    .to_string())
}

fn text(value: &Value, key: &str) -> Result<String, String> {
    value[key]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{key} is not text"))
}

fn optional_text(value: &Value, key: &str) -> Result<Option<String>, String> {
    match &value[key] {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text.clone())),
        _ => Err(format!("{key} is not text")),
    }
}

fn texts(value: &Value, key: &str) -> Result<Vec<String>, String> {
    value[key]
        .as_array()
        .ok_or_else(|| format!("{key} is not a list"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{key} holds a non-text item"))
        })
        .collect()
}

fn number(value: &Value, key: &str) -> Result<u64, String> {
    value[key]
        .as_u64()
        .ok_or_else(|| format!("{key} is not a number"))
}

fn at(value: &Value, key: &str) -> Result<DateTime<Utc>, String> {
    from_iso(&text(value, key)?).map_err(|error| error.to_string())
}

fn status(value: &Value, key: &str) -> Result<RunStatus, String> {
    RunStatus::parse(&text(value, key)?).map_err(|error| error.to_string())
}

fn decode_change(change: &Value) -> Result<NoticeChange, String> {
    Ok(match text(change, "type")?.as_str() {
        "run_status" => NoticeChange::RunStatus {
            run_id: text(change, "run_id")?,
            from: status(change, "from")?,
            to: status(change, "to")?,
        },
        "run_moved" => NoticeChange::RunMoved {
            run_id: text(change, "run_id")?,
            from: at(change, "from")?,
            to: at(change, "to")?,
        },
        "run_reset" => NoticeChange::RunReset {
            run_id: text(change, "run_id")?,
            from: at(change, "from")?,
            to: at(change, "to")?,
        },
        "run_swapped" => NoticeChange::RunSwapped {
            run_id: text(change, "run_id")?,
            participants: texts(change, "participants")?,
            leaving: texts(change, "leaving")?,
            joining: texts(change, "joining")?,
        },
        "rollback" => NoticeChange::Rollback {
            reverted: change["reverted"]
                .as_array()
                .ok_or("reverted is not a list")?
                .iter()
                .map(|seq| seq.as_u64().ok_or("reverted holds a non-number"))
                .collect::<Result<_, _>>()?,
            cancelled_runs: texts(change, "cancelled_runs")?,
            run_ids: texts(change, "run_ids")?,
            checkpoint: optional_text(change, "checkpoint")?,
        },
        "merged" => NoticeChange::Merged {
            draft: text(change, "draft")?,
            version: number(change, "version")?,
            title: text(change, "title")?,
            run_ids: texts(change, "run_ids")?,
            fixed_ids: texts(change, "fixed_ids")?,
        },
        "request_decided" => {
            let decision = text(change, "decision")?;
            NoticeChange::RequestDecided {
                request: text(change, "request")?,
                decision: *DECISIONS
                    .iter()
                    .find(|known| known.as_str() == decision)
                    .ok_or_else(|| format!("unknown decision {decision}"))?,
                reason: optional_text(change, "reason")?,
            }
        }
        "fixed_changed" => NoticeChange::FixedChanged {
            fixed_id: text(change, "fixed_id")?,
            fields: texts(change, "fields")?
                .iter()
                .map(|name| {
                    FIELDS
                        .iter()
                        .copied()
                        .find(|field| field.as_str() == name)
                        .ok_or_else(|| format!("unknown field {name}"))
                })
                .collect::<Result<_, _>>()?,
            weekday: usize::try_from(number(change, "weekday")?)
                .ok()
                .and_then(|day| WEEKDAYS.get(day).copied())
                .ok_or("weekday is out of range")?,
            time: u32::try_from(number(change, "seconds")?)
                .ok()
                .zip(u32::try_from(number(change, "nanos")?).ok())
                .and_then(|(secs, nanos)| {
                    NaiveTime::from_num_seconds_from_midnight_opt(secs, nanos)
                })
                .ok_or("time is out of range")?,
            participants: texts(change, "participants")?,
        },
        "fixed_added" => NoticeChange::FixedAdded {
            fixed_id: text(change, "fixed_id")?,
            bosses: texts(change, "bosses")?,
            weekday: weekday(change)?,
            time: time(change)?,
            participants: texts(change, "participants")?,
        },
        "fixed_removed" => NoticeChange::FixedRemoved {
            fixed_id: text(change, "fixed_id")?,
            bosses: texts(change, "bosses")?,
            weekday: weekday(change)?,
            time: time(change)?,
            participants: texts(change, "participants")?,
            cancelled_runs: usize::try_from(number(change, "cancelled_runs")?)
                .map_err(|_| "cancelled_runs is out of range".to_owned())?,
        },
        other => return Err(format!("unknown notice type {other}")),
    })
}

fn weekday(change: &Value) -> Result<Weekday, String> {
    usize::try_from(number(change, "weekday")?)
        .ok()
        .and_then(|day| WEEKDAYS.get(day).copied())
        .ok_or("weekday is out of range".into())
}

fn time(change: &Value) -> Result<NaiveTime, String> {
    u32::try_from(number(change, "seconds")?)
        .ok()
        .zip(u32::try_from(number(change, "nanos")?).ok())
        .and_then(|(secs, nanos)| NaiveTime::from_num_seconds_from_midnight_opt(secs, nanos))
        .ok_or("time is out of range".into())
}

/// # Errors
/// Text that is not a known notice version.
pub(super) fn decode(payload: &str) -> Result<Notice, String> {
    let value: Value = serde_json::from_str(payload).map_err(|error| error.to_string())?;
    if !matches!(value["v"].as_i64(), Some(V1 | V2)) {
        return Err(format!("unknown payload version {}", value["v"]));
    }
    Ok(Notice {
        change: decode_change(&value["change"])?,
        channel_id: optional_text(&value, "channel_id")?,
        listed: texts(&value, "listed")?,
        via_portal: value["via_portal"]
            .as_bool()
            .ok_or("via_portal is not a flag")?,
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn notice(change: NoticeChange) -> Notice {
        Notice {
            change,
            channel_id: Some("900".into()),
            listed: vec!["1".into(), "2".into()],
            via_portal: true,
        }
    }

    #[test]
    fn every_notice_kind_round_trips() {
        let at = Utc.with_ymd_and_hms(2026, 9, 23, 12, 30, 0).unwrap();
        let later = at + chrono::TimeDelta::microseconds(1_500_001);
        let changes = vec![
            NoticeChange::RunStatus {
                run_id: "r".into(),
                from: RunStatus::Planned,
                to: RunStatus::Cancelled,
            },
            NoticeChange::RunMoved {
                run_id: "r".into(),
                from: at,
                to: later,
            },
            NoticeChange::RunReset {
                run_id: "r".into(),
                from: later,
                to: at,
            },
            NoticeChange::RunSwapped {
                run_id: "r".into(),
                participants: vec!["1".into()],
                leaving: vec!["2".into()],
                joining: Vec::new(),
            },
            NoticeChange::Rollback {
                reverted: vec![7, 3],
                cancelled_runs: vec!["c".into()],
                run_ids: vec!["r".into()],
                checkpoint: Some("week-1".into()),
            },
            NoticeChange::Rollback {
                reverted: Vec::new(),
                cancelled_runs: Vec::new(),
                run_ids: Vec::new(),
                checkpoint: None,
            },
            NoticeChange::Merged {
                draft: "d".into(),
                version: 4,
                title: "ü \"quoted\"".into(),
                run_ids: vec!["r".into()],
                fixed_ids: vec!["f".into()],
            },
            NoticeChange::RequestDecided {
                request: "q".into(),
                decision: RequestDecision::Rejected,
                reason: Some("no".into()),
            },
            NoticeChange::RequestDecided {
                request: "q".into(),
                decision: RequestDecision::Expired,
                reason: None,
            },
            NoticeChange::FixedChanged {
                fixed_id: "f".into(),
                fields: FIELDS.to_vec(),
                weekday: Weekday::Sun,
                time: NaiveTime::from_hms_opt(21, 5, 0).unwrap(),
                participants: vec!["3".into()],
            },
            NoticeChange::FixedAdded {
                fixed_id: "f".into(),
                bosses: vec!["HFA".into()],
                weekday: Weekday::Mon,
                time: NaiveTime::from_hms_opt(20, 30, 0).unwrap(),
                participants: vec!["1".into()],
            },
            NoticeChange::FixedRemoved {
                fixed_id: "f".into(),
                bosses: vec!["HFA".into()],
                weekday: Weekday::Tue,
                time: NaiveTime::from_hms_opt(20, 30, 0).unwrap(),
                participants: vec!["2".into()],
                cancelled_runs: 3,
            },
        ];
        for change in changes {
            let mut original = notice(change);
            let payload = encode(&original).unwrap();
            let decoded = decode(&payload).unwrap();
            assert_eq!(decoded, original);
            if !matches!(
                &original.change,
                NoticeChange::FixedAdded { .. } | NoticeChange::FixedRemoved { .. }
            ) {
                assert_eq!(decode_v1_only(&payload).unwrap(), original);
            }
            original.channel_id = None;
            original.via_portal = false;
            assert_eq!(decode(&encode(&original).unwrap()).unwrap(), original);
        }
    }

    #[test]
    fn unknown_versions_and_kinds_are_refused() {
        assert!(decode(r#"{"v":3}"#).is_err());
        assert!(
            decode(r#"{"v":2,"change":{"type":"other"},"listed":[],"via_portal":false}"#).is_err()
        );
    }

    #[test]
    fn existing_kinds_keep_v1_bytes_and_a_v1_reader_accepts_them() {
        let notice = notice(NoticeChange::RunStatus {
            run_id: "r".into(),
            from: RunStatus::Planned,
            to: RunStatus::Cancelled,
        });
        let payload = encode(&notice).unwrap();
        assert_eq!(
            payload,
            r#"{"change":{"from":"planned","run_id":"r","to":"cancelled","type":"run_status"},"channel_id":"900","listed":["1","2"],"v":1,"via_portal":true}"#
        );
        assert_eq!(decode_v1_only(&payload).unwrap(), notice);
    }

    #[test]
    fn timing_add_and_remove_write_v2() {
        for change in [
            NoticeChange::FixedAdded {
                fixed_id: "f".into(),
                bosses: vec!["HFA".into()],
                weekday: Weekday::Mon,
                time: NaiveTime::from_hms_opt(21, 0, 0).unwrap(),
                participants: vec!["1".into()],
            },
            NoticeChange::FixedRemoved {
                fixed_id: "f".into(),
                bosses: vec!["HFA".into()],
                weekday: Weekday::Mon,
                time: NaiveTime::from_hms_opt(21, 0, 0).unwrap(),
                participants: vec!["1".into()],
                cancelled_runs: 1,
            },
        ] {
            let value: Value = serde_json::from_str(&encode(&notice(change)).unwrap()).unwrap();
            assert_eq!(value["v"], V2);
        }
    }

    /// The pre-v2 reader's version gate over the unchanged v1 shape.
    fn decode_v1_only(payload: &str) -> Result<Notice, String> {
        let value: Value = serde_json::from_str(payload).map_err(|error| error.to_string())?;
        if value["v"].as_i64() != Some(V1) {
            return Err("unknown payload version".into());
        }
        Ok(Notice {
            change: decode_change(&value["change"])?,
            channel_id: optional_text(&value, "channel_id")?,
            listed: texts(&value, "listed")?,
            via_portal: value["via_portal"]
                .as_bool()
                .ok_or("via_portal is not a flag")?,
        })
    }
}
