//! The stored form of a draft operation, format `kanade.draft_op.v1`: one
//! JSON object, keys sorted (`serde_json` maps), instants as UTC ISO text
//! (`domain::time::to_iso`), weekly times `HH:MM:SS`, weekdays 0 = Monday,
//! enums by their stored v4 spelling, absent optionals as `null`. Targets
//! are `{"existing": id}` or `{"created": ord}`. Any change to the encoding
//! is a new format version.

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, NaiveTime, Timelike, Utc, Weekday};
use serde_json::{Map, Value, json};

use super::op::{DraftOp, Target};
use crate::domain::schedule::{
    AmendedRunChoice, FixedEdit, FixedEditChoices, NewFixedRun, RsvpSource, RsvpState, RunSource,
    RunStatus, StatusChange,
};
use crate::domain::time::{from_iso, to_iso};

pub const DRAFT_OP_FORMAT: &str = "kanade.draft_op.v1";

/// A stored operation that cannot be read, or one that cannot be written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecError(pub String);

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "draft operation codec: {}", self.0)
    }
}

impl std::error::Error for CodecError {}

fn bad(detail: impl fmt::Display) -> CodecError {
    CodecError(detail.to_string())
}

fn iso(at: &DateTime<Utc>) -> Result<String, CodecError> {
    to_iso(at).map_err(bad)
}

fn wall(time: NaiveTime) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        time.hour(),
        time.minute(),
        time.second()
    )
}

fn target(target: &Target) -> Value {
    match target {
        Target::Existing(id) => json!({"existing": id}),
        Target::Created(ord) => json!({"created": ord}),
    }
}

fn choice(choice: AmendedRunChoice) -> &'static str {
    match choice {
        AmendedRunChoice::UpdateToFixed => "update_to_fixed",
        AmendedRunChoice::KeepForThisWeek => "keep_for_this_week",
    }
}

/// Drafts never change a timing's owner (only admin edits do), so the
/// stored `kanade.draft_op.v1` edit has no `owner_id` and refuses one.
fn fixed_edit(edit: &FixedEdit) -> Result<Value, CodecError> {
    if edit.owner_id.is_some() {
        return Err(bad("a draft edit cannot change the owner"));
    }
    Ok(json!({
        "bosses": edit.bosses,
        "weekday": edit.weekday.map(|day| day.num_days_from_monday()),
        "time": edit.time.map(wall),
        "participants": edit.participants,
        "channel_id": edit.channel_id,
        "note": edit.note,
    }))
}

/// Encode one operation.
///
/// # Errors
/// [`CodecError`] for an instant outside the representable years, or a
/// timing edit that changes the owner.
pub fn encode(op: &DraftOp) -> Result<String, CodecError> {
    let mut body = match op {
        DraftOp::AddFixedRun(new) => json!({
            "fixed_run": {
                "owner_id": new.owner_id,
                "channel_id": new.channel_id,
                "bosses": new.bosses,
                "weekday": new.weekday.num_days_from_monday(),
                "time": wall(new.time),
                "participants": new.participants,
                "note": new.note,
            },
        }),
        DraftOp::ApplyFixedEdit {
            fixed,
            edit,
            choices,
        } => json!({
            "fixed": target(fixed),
            "edit": fixed_edit(edit)?,
            "choices": match choices {
                FixedEditChoices::UpdateAll => json!("update_all"),
                FixedEditChoices::PerRun(per_run) => json!({
                    "per_run": per_run
                        .iter()
                        .map(|(run, pick)| (run.clone(), Value::from(choice(*pick))))
                        .collect::<Map<_, _>>(),
                }),
            },
        }),
        DraftOp::FixedParticipants { fixed, add, remove } => json!({
            "fixed": target(fixed),
            "add": add,
            "remove": remove,
        }),
        DraftOp::RetireFixedRun { fixed, weeks } => json!({
            "fixed": target(fixed),
            "weeks": weeks.iter().map(iso).collect::<Result<Vec<_>, _>>()?,
        }),
        DraftOp::CreateRun {
            fixed,
            channel_id,
            week_start,
            datetime,
            bosses,
            participants,
            status,
            source,
        } => json!({
            "fixed": fixed.as_ref().map(target),
            "channel_id": channel_id,
            "week_start": iso(week_start)?,
            "datetime": iso(datetime)?,
            "bosses": bosses,
            "participants": participants,
            "status": status.as_str(),
            "source": source.as_str(),
        }),
        DraftOp::AmendRun { run, to } => json!({"run": target(run), "to": iso(to)?}),
        DraftOp::SetStatus { run, change } => json!({
            "run": target(run),
            "change": {
                "status": change.status.as_str(),
                "announce": change.announce,
                "via_portal": change.via_portal,
            },
        }),
        DraftOp::SwapParticipants {
            run,
            remove,
            add,
            via_portal,
        } => json!({
            "run": target(run),
            "remove": remove,
            "add": add,
            "via_portal": via_portal,
        }),
        DraftOp::SetRsvp {
            run,
            user_id,
            state,
            source,
        } => json!({
            "run": target(run),
            "user_id": user_id,
            "state": state.as_str(),
            "source": source.as_str(),
        }),
        DraftOp::ResetToFixed { run }
        | DraftOp::EnsureReminders { run }
        | DraftOp::RecountRun { run }
        | DraftOp::ReviveRun { run } => json!({"run": target(run)}),
        DraftOp::SetRunBosses { run, bosses } => json!({"run": target(run), "bosses": bosses}),
    };
    if let Value::Object(map) = &mut body {
        map.insert("format".into(), DRAFT_OP_FORMAT.into());
        map.insert("op".into(), op.kind().into());
    }
    Ok(body.to_string())
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, CodecError> {
    value
        .get(name)
        .ok_or_else(|| bad(format!("missing {name}")))
}

fn text(value: &Value, name: &str) -> Result<String, CodecError> {
    field(value, name)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| bad(format!("{name} is not text")))
}

fn optional_text(value: &Value, name: &str) -> Result<Option<String>, CodecError> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(bad(format!("{name} is not text"))),
    }
}

fn texts(value: &Value) -> Result<Vec<String>, CodecError> {
    value
        .as_array()
        .ok_or_else(|| bad("not a list"))?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| bad("list holds non-text"))
        })
        .collect()
}

fn optional_texts(value: &Value, name: &str) -> Result<Option<Vec<String>>, CodecError> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(list) => texts(list).map(Some),
    }
}

fn boolean(value: &Value, name: &str) -> Result<bool, CodecError> {
    field(value, name)?
        .as_bool()
        .ok_or_else(|| bad(format!("{name} is not a boolean")))
}

/// Exactly the encoder's form: another spelling of the same instant is refused.
fn parse_instant(text: &str) -> Result<DateTime<Utc>, CodecError> {
    let at = from_iso(text).map_err(bad)?;
    if iso(&at)? != text {
        return Err(bad(format!("instant {text}")));
    }
    Ok(at)
}

fn instant(value: &Value, name: &str) -> Result<DateTime<Utc>, CodecError> {
    parse_instant(&text(value, name)?)
}

/// An object holding exactly `names` (optionals are written as `null`).
fn exact_keys(value: &Value, names: &[&str]) -> Result<(), CodecError> {
    let object = value.as_object().ok_or_else(|| bad("not an object"))?;
    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
    let mut expected = names.to_vec();
    keys.sort_unstable();
    expected.sort_unstable();
    if keys == expected {
        Ok(())
    } else {
        Err(bad(format!("keys {keys:?}, expected {expected:?}")))
    }
}

fn weekday(value: &Value) -> Result<Weekday, CodecError> {
    value
        .as_u64()
        .and_then(|day| u8::try_from(day).ok())
        .and_then(|day| Weekday::try_from(day).ok())
        .ok_or_else(|| bad("weekday"))
}

/// Exactly `HH:MM:SS`.
fn time(text: &str) -> Result<NaiveTime, CodecError> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == 8
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            2 | 5 => *byte == b':',
            _ => byte.is_ascii_digit(),
        });
    let number = |range: std::ops::Range<usize>| text[range].parse::<u32>().ok();
    shaped
        .then(|| NaiveTime::from_hms_opt(number(0..2)?, number(3..5)?, number(6..8)?))
        .flatten()
        .ok_or_else(|| bad(format!("time {text}")))
}

fn read_target(value: &Value) -> Result<Target, CodecError> {
    if exact_keys(value, &["existing"]).is_ok() {
        return text(value, "existing").map(Target::Existing);
    }
    exact_keys(value, &["created"])?;
    value
        .get("created")
        .and_then(Value::as_u64)
        .and_then(|ord| usize::try_from(ord).ok())
        .map(Target::Created)
        .ok_or_else(|| bad("target"))
}

fn read_choices(value: &Value) -> Result<FixedEditChoices, CodecError> {
    if value.as_str() == Some("update_all") {
        return Ok(FixedEditChoices::UpdateAll);
    }
    exact_keys(value, &["per_run"])?;
    let per_run = field(value, "per_run")?
        .as_object()
        .ok_or_else(|| bad("per_run"))?
        .iter()
        .map(|(run, pick)| {
            let pick = match pick.as_str() {
                Some("update_to_fixed") => AmendedRunChoice::UpdateToFixed,
                Some("keep_for_this_week") => AmendedRunChoice::KeepForThisWeek,
                _ => return Err(bad("choice")),
            };
            Ok((run.clone(), pick))
        })
        .collect::<Result<BTreeMap<_, _>, CodecError>>()?;
    Ok(FixedEditChoices::PerRun(per_run))
}

/// Decode one stored operation.
///
/// # Errors
/// [`CodecError`] for malformed text or another format.
pub fn decode(stored: &str) -> Result<DraftOp, CodecError> {
    let value: Value = serde_json::from_str(stored).map_err(bad)?;
    if value.get("format").and_then(Value::as_str) != Some(DRAFT_OP_FORMAT) {
        return Err(bad("unknown format"));
    }
    let run = || read_target(field(&value, "run")?);
    let op = text(&value, "op")?;
    let body: &[&str] = match op.as_str() {
        "add_fixed_run" => &["fixed_run"],
        "apply_fixed_edit" => &["fixed", "edit", "choices"],
        "fixed_participants" => &["fixed", "add", "remove"],
        "retire_fixed_run" => &["fixed", "weeks"],
        "create_run" => &[
            "fixed",
            "channel_id",
            "week_start",
            "datetime",
            "bosses",
            "participants",
            "status",
            "source",
        ],
        "amend_run" => &["run", "to"],
        "set_status" => &["run", "change"],
        "swap_participants" => &["run", "remove", "add", "via_portal"],
        "set_rsvp" => &["run", "user_id", "state", "source"],
        "reset_to_fixed" | "ensure_reminders" | "recount_run" | "revive_run" => &["run"],
        "set_run_bosses" => &["run", "bosses"],
        other => return Err(bad(format!("unknown operation {other}"))),
    };
    exact_keys(&value, &[&["format", "op"][..], body].concat())?;
    Ok(match op.as_str() {
        "add_fixed_run" => {
            let fixed = field(&value, "fixed_run")?;
            exact_keys(
                fixed,
                &[
                    "owner_id",
                    "channel_id",
                    "bosses",
                    "weekday",
                    "time",
                    "participants",
                    "note",
                ],
            )?;
            DraftOp::AddFixedRun(NewFixedRun {
                owner_id: text(fixed, "owner_id")?,
                channel_id: optional_text(fixed, "channel_id")?,
                bosses: texts(field(fixed, "bosses")?)?,
                weekday: weekday(field(fixed, "weekday")?)?,
                time: time(&text(fixed, "time")?)?,
                participants: texts(field(fixed, "participants")?)?,
                note: optional_text(fixed, "note")?,
            })
        }
        "apply_fixed_edit" => {
            let edit = field(&value, "edit")?;
            exact_keys(
                edit,
                &[
                    "bosses",
                    "weekday",
                    "time",
                    "participants",
                    "channel_id",
                    "note",
                ],
            )?;
            DraftOp::ApplyFixedEdit {
                fixed: read_target(field(&value, "fixed")?)?,
                edit: FixedEdit {
                    bosses: optional_texts(edit, "bosses")?,
                    weekday: match edit.get("weekday") {
                        None | Some(Value::Null) => None,
                        Some(day) => Some(weekday(day)?),
                    },
                    time: optional_text(edit, "time")?
                        .map(|text| time(&text))
                        .transpose()?,
                    participants: optional_texts(edit, "participants")?,
                    channel_id: optional_text(edit, "channel_id")?,
                    note: optional_text(edit, "note")?,
                    owner_id: None,
                },
                choices: read_choices(field(&value, "choices")?)?,
            }
        }
        "fixed_participants" => DraftOp::FixedParticipants {
            fixed: read_target(field(&value, "fixed")?)?,
            add: texts(field(&value, "add")?)?,
            remove: texts(field(&value, "remove")?)?,
        },
        "retire_fixed_run" => DraftOp::RetireFixedRun {
            fixed: read_target(field(&value, "fixed")?)?,
            weeks: texts(field(&value, "weeks")?)?
                .iter()
                .map(|week| parse_instant(week))
                .collect::<Result<_, _>>()?,
        },
        "create_run" => DraftOp::CreateRun {
            fixed: match value.get("fixed") {
                None | Some(Value::Null) => None,
                Some(fixed) => Some(read_target(fixed)?),
            },
            channel_id: optional_text(&value, "channel_id")?,
            week_start: instant(&value, "week_start")?,
            datetime: instant(&value, "datetime")?,
            bosses: texts(field(&value, "bosses")?)?,
            participants: texts(field(&value, "participants")?)?,
            status: RunStatus::parse(&text(&value, "status")?).map_err(bad)?,
            source: RunSource::parse(&text(&value, "source")?).map_err(bad)?,
        },
        "amend_run" => DraftOp::AmendRun {
            run: run()?,
            to: instant(&value, "to")?,
        },
        "set_status" => {
            let change = field(&value, "change")?;
            exact_keys(change, &["status", "announce", "via_portal"])?;
            DraftOp::SetStatus {
                run: run()?,
                change: StatusChange {
                    status: RunStatus::parse(&text(change, "status")?).map_err(bad)?,
                    announce: boolean(change, "announce")?,
                    via_portal: boolean(change, "via_portal")?,
                },
            }
        }
        "swap_participants" => DraftOp::SwapParticipants {
            run: run()?,
            remove: texts(field(&value, "remove")?)?,
            add: texts(field(&value, "add")?)?,
            via_portal: boolean(&value, "via_portal")?,
        },
        "set_rsvp" => DraftOp::SetRsvp {
            run: run()?,
            user_id: text(&value, "user_id")?,
            state: RsvpState::parse(&text(&value, "state")?).map_err(bad)?,
            source: RsvpSource::parse(&text(&value, "source")?).map_err(bad)?,
        },
        "reset_to_fixed" => DraftOp::ResetToFixed { run: run()? },
        "ensure_reminders" => DraftOp::EnsureReminders { run: run()? },
        "recount_run" => DraftOp::RecountRun { run: run()? },
        "revive_run" => DraftOp::ReviveRun { run: run()? },
        "set_run_bosses" => DraftOp::SetRunBosses {
            run: run()?,
            bosses: texts(field(&value, "bosses")?)?,
        },
        other => return Err(bad(format!("unknown operation {other}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_edit_cannot_carry_an_owner() {
        let op = DraftOp::ApplyFixedEdit {
            fixed: Target::Existing("fixed-1".into()),
            edit: FixedEdit {
                owner_id: Some("1002".into()),
                ..FixedEdit::default()
            },
            choices: FixedEditChoices::UpdateAll,
        };
        assert!(encode(&op).is_err());
    }
}
