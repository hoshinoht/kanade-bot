//! Vector steps the scheduler, reminder and mutation families replay alike.

use std::collections::BTreeMap;

use kanade::domain::history::Origin;
use kanade::domain::schedule::{NewFixedRun, NewRun, RunSource, RunStatus};
use serde_json::{Value, json};

use crate::common::{Service, clock_time, strings, text, utc, weekday};

/// `add_fixed`: register the new timing under its `fixed_key`.
pub async fn add_fixed(
    service: &mut Service,
    step: &Value,
    fixed_refs: &mut BTreeMap<String, String>,
) -> Value {
    let id = service
        .as_origin(Origin::for_tests())
        .add_fixed_run(NewFixedRun {
            owner_pinned: false,
            owner_id: text(&step["owner_id"]).into(),
            channel_id: Some(text(&step["channel_id"]).into()),
            bosses: strings(&step["bosses"]),
            weekday: weekday(&step["weekday"]),
            time: clock_time(&step["time"]),
            participants: strings(&step["participants"]),
            note: None,
        })
        .await
        .expect("add_fixed");
    fixed_refs.insert(text(&step["fixed_key"]).to_owned(), id.clone());
    json!(id)
}

/// `create_run` in `week_start`: register the one-off under its `run_key`.
pub async fn create_run(
    service: &mut Service,
    step: &Value,
    week_start: &Value,
    run_refs: &mut BTreeMap<String, String>,
) -> Value {
    let id = service
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(text(&step["channel_id"]).into()),
            week_start: utc(week_start),
            datetime: utc(&step["at"]),
            bosses: strings(&step["bosses"]),
            participants: strings(&step["participants"]),
            status: RunStatus::parse(text(&step["status"])).unwrap(),
            source: RunSource::parse(text(&step["source"])).unwrap(),
        })
        .await
        .expect("create_run");
    run_refs.insert(text(&step["run_key"]).to_owned(), id.clone());
    json!(id)
}
