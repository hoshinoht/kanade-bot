use kanade::domain::{
    time::DateOutOfRange,
    weeks::{self, WeekParseError},
};
use serde_json::{Value, json};

use crate::support::{
    Outcome, clock, clock_text, instant, integer, replay_family, text, unknown_op, zone,
};

fn replay(op: &str, input: &Value, _fixtures: &Value) -> Outcome {
    let reset = || {
        let weekday =
            weeks::weekday_from_index(integer(input, "reset_weekday")).map_err(parse_error)?;
        Ok::<_, (&'static str, String)>((weekday, clock(input, "reset_time")))
    };
    match op {
        "week_start" => {
            let (weekday, time) = reset()?;
            let start = weeks::week_start(&instant(input, "at"), zone(input), weekday, time);
            Ok(json!(start.map_err(overflow)?.isoformat()))
        }
        "calendar_week_bounds" => {
            let (weekday, time) = reset()?;
            let (at, tz) = (instant(input, "at"), zone(input));
            let start = weeks::calendar_week_start(&at, tz).map_err(overflow)?;
            Ok(json!({
                "start": start.isoformat(),
                "end": weeks::calendar_week_end(&start, tz).map_err(overflow)?.isoformat(),
                "boss_start": weeks::week_start(&at, tz, weekday, time).map_err(overflow)?.isoformat(),
            }))
        }
        "materialised_week_starts" => {
            let (weekday, time) = reset()?;
            let starts =
                weeks::materialised_week_starts(zone(input), weekday, time, &instant(input, "at"))
                    .map_err(overflow)?;
            Ok(json!(
                starts
                    .iter()
                    .map(|start| start.isoformat())
                    .collect::<Vec<_>>()
            ))
        }
        "slot_in_week" => {
            let weekday =
                weeks::weekday_from_index(integer(input, "weekday")).map_err(parse_error)?;
            let slot = weeks::slot_in_week(
                &instant(input, "week_start"),
                zone(input),
                weekday,
                clock(input, "time"),
            );
            Ok(json!(slot.map_err(overflow)?.isoformat()))
        }
        "week_end" => {
            let end = weeks::week_end(&instant(input, "week_start"), zone(input));
            Ok(json!(end.map_err(overflow)?.isoformat()))
        }
        "parse_weekday" => {
            let weekday = weeks::parse_weekday(text(input, "value")).map_err(parse_error)?;
            Ok(json!(weekday.num_days_from_monday()))
        }
        "parse_hhmm" => {
            let time = weeks::parse_hhmm(text(input, "value")).map_err(parse_error)?;
            Ok(json!(clock_text(time)))
        }
        other => unknown_op("weeks", other),
    }
}

fn parse_error(error: WeekParseError) -> (&'static str, String) {
    ("ValueError", error.to_string())
}

fn overflow(error: DateOutOfRange) -> (&'static str, String) {
    ("OverflowError", error.to_string())
}

#[test]
fn weeks_vectors_replay_exactly() {
    replay_family("weeks", replay);
}
