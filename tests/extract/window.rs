use chrono::{DateTime, NaiveTime, TimeDelta, Utc};
use kanade::domain::time::{ZonedDateTime, isoformat};
use kanade::domain::weeks;
use kanade::extract::window::{self, WindowError};
use serde_json::{Value, json};

use crate::support::{Outcome, flag, instant, replay_family, text, unknown_op, zone};

#[derive(Clone)]
struct Row {
    id: String,
    created_at: DateTime<Utc>,
}

fn rows(value: &Value) -> Vec<Row> {
    value
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| Row {
            id: text(&row["id"]).to_owned(),
            created_at: instant(&row["created_at"]).with_timezone(&Utc),
        })
        .collect()
}

fn created(row: &Row) -> DateTime<Utc> {
    row.created_at
}

fn ids(groups: Vec<Vec<Row>>) -> Value {
    json!(
        groups
            .into_iter()
            .map(|group| group.into_iter().map(|row| row.id).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    )
}

fn count(value: &Value) -> usize {
    usize::try_from(value.as_u64().expect("count")).expect("usize")
}

fn value_error(error: WindowError) -> (&'static str, String) {
    match error {
        WindowError::Unknown { .. } => ("ValueError", error.to_string()),
        WindowError::OutOfRange => panic!("unexpected overflow"),
    }
}

fn replay(input: &Value, step: &Value) -> Outcome {
    let tz = zone(input);
    let reset_weekday =
        weeks::weekday_from_index(input["reset_weekday"].as_i64().expect("weekday"))
            .expect("0..=6");
    let reset_time = text(&input["reset_time"])
        .parse::<NaiveTime>()
        .expect("reset clock");
    let value = match text(&step["op"]) {
        "constants" => json!({
            "burst_gap_seconds": window::BURST_GAP.num_seconds(),
            "max_burst_messages": window::MAX_BURST_MESSAGES,
            "windows": window::WINDOWS,
            "default_window": window::DEFAULT_WINDOW,
            "automated_window": window::AUTOMATED_WINDOW,
        }),
        "group_bursts" => {
            let gap = match step["gap_seconds"].as_i64() {
                Some(seconds) => TimeDelta::seconds(seconds),
                None => window::BURST_GAP,
            };
            ids(window::group_bursts(&rows(&step["rows"]), gap, created))
        }
        "group_for_rescan" => {
            let cap = match &step["cap"] {
                Value::Null => window::MAX_BURST_MESSAGES,
                cap => count(cap),
            };
            ids(window::group_for_rescan(
                &rows(&step["rows"]),
                tz,
                window::BURST_GAP,
                cap,
                created,
            ))
        }
        "split_until" => {
            let limit = count(&step["max_messages"]);
            ids(window::split_until(
                &rows(&step["rows"]),
                |chunk| chunk.len() <= limit,
                created,
            ))
        }
        "clamp_window" => json!(
            window::clamp_window(text(&step["window"]), flag(&step["automated"]))
                .map_err(value_error)?
        ),
        "window_since" => {
            let since = window::window_since(
                text(&step["window"]),
                tz,
                reset_weekday,
                reset_time,
                &instant(&step["now"]),
            )
            .map_err(value_error)?;
            json!(isoformat(&since))
        }
        "previous_week_start" => {
            // The v4 caller passes a guild-zone week start; the fixed-offset
            // vector instant is viewed in the guild zone first.
            let this_week =
                ZonedDateTime::from_instant(&instant(&step["this_week"]), tz).expect("in range");
            let previous = window::previous_week_start(&this_week, tz, reset_weekday, reset_time)
                .expect("in range");
            json!(previous.isoformat())
        }
        "should_widen" => json!(window::should_widen(
            text(&step["window"]),
            count(&step["gated"]),
            flag(&step["automated"])
        )),
        other => unknown_op("window", other),
    };
    Ok(value)
}

#[test]
fn window_vectors_replay_exactly() {
    assert_eq!(replay_family("window", replay), (3, 32));
}
