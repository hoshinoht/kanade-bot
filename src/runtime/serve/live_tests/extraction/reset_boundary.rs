use chrono::TimeZone;
use chrono_tz::Asia::Kuala_Lumpur;
use serde_json::Value;

use crate::{
    domain::schedule::Run,
    extract::{matching::reachable, resolve::resolve},
};

use super::{ALICE, HOME_A, RunSource, RunStatus, Utc, moved};

#[test]
fn tomorrow_move_fixture_reaches_next_boss_week_on_reset_eve() {
    let zone = Kuala_Lumpur;
    let anchor = zone
        .with_ymd_and_hms(2026, 10, 7, 12, 0, 0)
        .unwrap()
        .with_timezone(&Utc);
    let slot = zone
        .with_ymd_and_hms(2026, 10, 8, 20, 0, 0)
        .unwrap()
        .with_timezone(&Utc);
    let week_start = zone
        .with_ymd_and_hms(2026, 10, 8, 8, 0, 0)
        .unwrap()
        .with_timezone(&Utc);
    let run = Run {
        id: "reset-boundary-run".into(),
        fixed_run_id: None,
        channel_id: Some(HOME_A.to_string()),
        week_start,
        datetime: slot,
        bosses: vec!["NKalos".into()],
        participants: vec![ALICE.to_string()],
        status: RunStatus::Planned,
        source: RunSource::Fixed,
        attendance: Vec::new(),
        status_pin: None,
    };
    let bare = resolve(None, Some("10pm"), &anchor, zone).unwrap();
    assert!(reachable(&[&run], bare.day, zone, None).is_empty());

    let reply: Value = serde_json::from_str(&moved(100)).unwrap();
    let amendment = &reply["amendments"][0];
    assert_eq!(amendment["day_ref"], "tomorrow");
    let explicit = resolve(
        amendment["day_ref"].as_str(),
        amendment["time_ref"].as_str(),
        &anchor,
        zone,
    )
    .unwrap();
    assert_eq!(explicit.day, Some(slot.with_timezone(&zone).date_naive()));
    assert_eq!(reachable(&[&run], explicit.day, zone, None), [&run]);
}
