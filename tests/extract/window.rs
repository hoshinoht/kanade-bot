use chrono::{DateTime, TimeDelta, Utc};
use kanade::extract::window;

#[test]
fn bursts_split_at_pauses_longer_than_the_gap() {
    let rows: Vec<(&str, DateTime<Utc>)> = [
        ("1", "2026-08-30T11:50:00+08:00"),
        ("2", "2026-08-30T12:40:00+08:00"),
        ("3", "2026-08-30T13:15:00+08:00"),
        ("4", "2026-08-30T16:20:00+08:00"),
        ("5", "2026-08-30T16:21:00+08:00"),
        ("6", "2026-08-30T23:59:00+08:00"),
        ("7", "2026-08-30T16:00:00+00:00"),
    ]
    .into_iter()
    .map(|(id, at)| (id, DateTime::parse_from_rfc3339(at).unwrap().to_utc()))
    .collect();
    let ids = |gap: TimeDelta| {
        window::group_bursts(&rows, gap, |row| row.1)
            .into_iter()
            .map(|group| group.into_iter().map(|row| row.0).collect::<Vec<_>>())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(window::BURST_GAP),
        [vec!["1", "2", "3"], vec!["4", "5"], vec!["6", "7"]]
    );
    assert_eq!(
        ids(TimeDelta::seconds(900)),
        [
            vec!["1"],
            vec!["2"],
            vec!["3"],
            vec!["4", "5"],
            vec!["6", "7"]
        ]
    );
}
