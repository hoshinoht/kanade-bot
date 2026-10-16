//! Per-boss character picks over the seeded store of `reads.rs`, with a
//! two-boss weekly timing `f-two` (XKalos + HMaleficStar, Alice 1001 and
//! Bob 1002), its live run `r-two` (Wed 30 Sep) and an ended two-boss run
//! `r-two-ended` (Mon 28 Sep, still planned, so frozen). A later boss's
//! pick saves with its own version, History row (key with the boss) and
//! replay; runs read the timing's per-boss pick live under their own; "same
//! for every boss" clears in one write; readiness judges each boss for the
//! character brought to it. Invented IGNs.

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::domain::{
    attendance::AttendanceDefault,
    characters::{CharacterStore, PickScope},
    schedule::{Change, FixedRun, Run, RunSource, RunStatus},
    settings::{SettingsChange, SettingsChangeQuery, SettingsStore},
};
use serde_json::{Value, json};

use crate::{
    readiness::{On, character, on, slot},
    reads::{Reads, commit_as},
    run_characters::pick,
};

const SLOT: &str = "characters.json#/$defs/RunCharacterSlot";
const TIMING: &str = "characters.json#/$defs/TimingCharacters";
const RUN: &str = "characters.json#/$defs/RunCharacters";
const READY: &str = "character_data.json#/$defs/RunReadiness";

fn utc(month: u32, day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, 0, 0).unwrap()
}

fn two_boss_run(id: &str, fixed: Option<&str>, at: DateTime<Utc>) -> Run {
    Run {
        id: id.into(),
        fixed_run_id: fixed.map(str::to_owned),
        channel_id: Some("kalos-four".into()),
        // This boss week: Thu 24 Sep 00:00 KL.
        week_start: utc(9, 23, 16),
        datetime: at,
        bosses: vec!["XKalos".into(), "HMaleficStar".into()],
        participants: vec!["1001".into(), "1002".into()],
        status: RunStatus::Planned,
        source: RunSource::Fixed,
        attendance: Vec::new(),
        status_pin: None,
    }
}

fn timing_with(bosses: &[&str]) -> FixedRun {
    FixedRun {
        owner_pinned: false,
        id: "f-two".into(),
        owner_id: "1001".into(),
        channel_id: Some("kalos-four".into()),
        bosses: bosses.iter().map(|boss| (*boss).to_owned()).collect(),
        weekday: Weekday::Wed,
        time: NaiveTime::from_hms_opt(21, 0, 0).unwrap(),
        participants: vec!["1001".into(), "1002".into()],
        note: None,
        attendance_default: AttendanceDefault::default(),
        standing: Vec::new(),
    }
}

async fn add_two_boss(reads: &Reads) {
    let changes = vec![
        Change::PutFixedRun(timing_with(&["XKalos", "HMaleficStar"])),
        Change::PutRun(two_boss_run("r-two", Some("f-two"), utc(9, 30, 13))),
        Change::PutRun(two_boss_run("r-two-ended", None, utc(9, 28, 13))),
    ];
    commit_as(&reads.store, "seed", utc(9, 28, 0), changes).await;
}

fn boss_path(scope: &str, target: &str, member: &str, boss: &str) -> String {
    format!("/api/admin/{scope}/{target}/characters/{member}/bosses/{boss}")
}

fn same_path(scope: &str, target: &str, member: &str) -> String {
    format!("/api/admin/{scope}/{target}/characters/{member}/same")
}

async fn rows(reads: &Reads, key: &str) -> Vec<SettingsChange> {
    reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|change| change.values.contains_key(key))
        .collect()
}

/// `(brings, basis)` of each boss of a slot.
fn per_boss(slot: &Value) -> Vec<(Value, Value)> {
    slot["bosses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|boss| (boss["brings"].clone(), boss["basis"].clone()))
        .collect()
}

struct Two {
    reads: Reads,
    lilac: String,
    wisteria: String,
    hydrangea: String,
}

async fn two() -> Two {
    let On { reads, .. } = on().await;
    add_two_boss(&reads).await;
    let lilac = character(&reads, "1001", "Lilac", 280).await;
    let wisteria = character(&reads, "1001", "Wisteria", 270).await;
    let hydrangea = character(&reads, "1001", "Hydrangea", 290).await;
    Two {
        reads,
        lilac,
        wisteria,
        hydrangea,
    }
}

#[tokio::test]
async fn a_timing_pick_for_a_later_boss_saves_replays_and_records_by_boss() {
    let Two {
        mut reads,
        lilac,
        wisteria,
        ..
    } = two().await;
    let answer = reads
        .read("/api/admin/fixed/f-two/characters", TIMING)
        .await;
    let alice = slot(&answer, "1001");
    assert_eq!(
        per_boss(alice),
        [
            (json!(lilac), json!("main")),
            (json!(lilac), json!("same_as_first"))
        ]
    );
    assert_eq!(
        (&alice["bosses"][1]["boss"], &alice["bosses"][1]["key"]),
        (&json!("HMaleficStar"), &json!("MaleficStar"))
    );
    assert_eq!(alice["per_boss"], false);

    let head = reads.version().await;
    let path = boss_path("fixed", "f-two", "1001", "MaleficStar");
    let key = [("Idempotency-Key", "boss-pick-k")];
    let first = reads
        .call("PUT", &path, pick(&*wisteria, 0), &key)
        .await
        .expect(200, SLOT);
    assert_eq!(
        per_boss(&first),
        [
            (json!(lilac), json!("main")),
            (json!(wisteria), json!("timing_pick"))
        ]
    );
    assert_eq!(first["per_boss"], true);
    assert_eq!(
        (
            &first["bosses"][1]["picked"],
            &first["bosses"][1]["version"]
        ),
        (&json!(wisteria), &json!(1))
    );
    assert_eq!(
        (&first["picked"], &first["version"]),
        (&Value::Null, &json!(0)),
        "the member's own pick is untouched"
    );
    // The retry replays across a restart; a token names the same boss.
    reads.restart(chrono::TimeDelta::zero(), None).await;
    let again = reads.call("PUT", &path, pick(&*wisteria, 0), &key).await;
    assert_eq!((again.status, again.json()), (200, first.clone()));
    let token = boss_path("fixed", "f-two", "1001", "HMaleficStar");
    assert_eq!(
        reads.refused("PUT", &token, pick(&*lilac, 0)).await,
        (409, "stale".into()),
        "the token is the same per-boss slot, at version 1 now"
    );
    for (path, want) in [
        (
            boss_path("fixed", "f-two", "1001", "XKalos"),
            (422, "first_boss"),
        ),
        (
            boss_path("fixed", "f-two", "1001", "Kalos"),
            (422, "first_boss"),
        ),
        (
            boss_path("fixed", "f-two", "1001", "HLucid"),
            (422, "boss_not_on_timing"),
        ),
        (
            boss_path("fixed", "f-two", "1004", "MaleficStar"),
            (422, "not_on_timing"),
        ),
        (
            boss_path("fixed", "f-gone", "1001", "MaleficStar"),
            (404, "not_found"),
        ),
    ] {
        assert_eq!(
            reads.refused("PUT", &path, pick(&*lilac, 0)).await,
            (want.0, want.1.into()),
            "{path}"
        );
    }
    let no_csrf = reads.without_csrf("PUT", &path, &pick(&*lilac, 1)).await;
    assert_eq!(no_csrf.error(), (403, "csrf".into()));
    assert_eq!(reads.version().await, head, "outside the history chain");
    let history = rows(&reads, "fixedpick.f-two.1001.MaleficStar").await;
    assert_eq!(history.len(), 1, "one row; the replay and refusals none");
    let row = &history[0].values["fixedpick.f-two.1001.MaleficStar"];
    assert_eq!(row.from, "", "from the same as the first boss");
    let to: Value = serde_json::from_str(&row.to).unwrap();
    assert_eq!(
        (&to["boss"], &to["ign"], &to["fixed"]),
        (&json!("MaleficStar"), &json!("Wisteria"), &json!("f-two"))
    );

    // The run made from the timing reads it live.
    let run = reads.read("/api/admin/runs/r-two/characters", RUN).await;
    assert_eq!(
        per_boss(slot(&run, "1001")),
        [
            (json!(lilac), json!("main")),
            (json!(wisteria), json!("timing_pick"))
        ]
    );

    // Same for every boss: one write, one row, stale against other versions.
    let same = same_path("fixed", "f-two", "1001");
    assert_eq!(
        reads
            .refused("POST", &same, json!({"versions": {"MaleficStar": 0}}))
            .await,
        (409, "stale".into())
    );
    assert_eq!(
        reads.refused("POST", &same, json!({})).await,
        (422, "version_required".into())
    );
    let cleared = reads
        .ok("POST", &same, json!({"versions": {"MaleficStar": 1}}), SLOT)
        .await;
    assert_eq!(
        per_boss(&cleared),
        [
            (json!(lilac), json!("main")),
            (json!(lilac), json!("same_as_first"))
        ]
    );
    let all = rows(&reads, "fixedpick.f-two.1001.*").await;
    assert_eq!(all.len(), 1);
    let from: Value = serde_json::from_str(&all[0].values["fixedpick.f-two.1001.*"].from).unwrap();
    assert_eq!(from[0]["ign"], "Wisteria");
    let fixed = ["f-two".to_owned()];
    assert!(
        reads
            .store
            .picks(PickScope::Timing, &fixed)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_run_pick_for_a_boss_wins_at_its_level_and_ended_runs_refuse() {
    let Two {
        reads,
        lilac,
        wisteria,
        hydrangea,
    } = two().await;
    let timing = boss_path("fixed", "f-two", "1001", "MaleficStar");
    reads.ok("PUT", &timing, pick(&*wisteria, 0), SLOT).await;
    // The run's own pick for the first boss leaves the timing's Star pick.
    let own = "/api/admin/runs/r-two/characters/1001";
    let first = reads.ok("PUT", own, pick(&*hydrangea, 0), SLOT).await;
    assert_eq!(
        per_boss(&first),
        [
            (json!(hydrangea), json!("run_pick")),
            (json!(wisteria), json!("timing_pick"))
        ]
    );
    // The run's own Star pick beats the timing's.
    let star = boss_path("runs", "r-two", "1001", "MaleficStar");
    let picked = reads.ok("PUT", &star, pick(&*lilac, 0), SLOT).await;
    assert_eq!(
        per_boss(&picked),
        [
            (json!(hydrangea), json!("run_pick")),
            (json!(lilac), json!("run_pick"))
        ]
    );
    assert_eq!(rows(&reads, "pick.r-two.1001.MaleficStar").await.len(), 1);
    // Readiness judges each boss for the character it brings.
    let ready = reads.read("/api/admin/runs/r-two/readiness", READY).await;
    let alice = slot(&ready, "1001");
    assert_eq!(alice["character"]["id"], json!(hydrangea));
    assert_eq!(alice["bosses"][0]["character_id"], json!(hydrangea));
    assert_eq!(alice["bosses"][1]["character_id"], json!(lilac));
    assert_eq!(alice["characters"].as_array().unwrap().len(), 1);
    assert_eq!(alice["characters"][0]["character"]["id"], json!(lilac));
    assert_eq!(slot(&ready, "1002")["characters"], json!([]));
    // Clearing the run's Star pick falls back to the timing's.
    let back = reads.ok("PUT", &star, pick(Value::Null, 1), SLOT).await;
    assert_eq!(per_boss(&back)[1], (json!(wisteria), json!("timing_pick")));
    // Run refusals: ended, wrong boss, not in the party.
    for (path, body, want) in [
        (
            boss_path("runs", "r-two-ended", "1001", "MaleficStar"),
            pick(&*lilac, 0),
            (409, "run_ended"),
        ),
        (
            same_path("runs", "r-two-ended", "1001"),
            json!({"versions": {}}),
            (409, "run_ended"),
        ),
        (
            boss_path("runs", "r-two", "1001", "Lucid"),
            pick(&*lilac, 0),
            (422, "boss_not_on_run"),
        ),
        (
            boss_path("runs", "r-two", "1001", "XKalos"),
            pick(&*lilac, 0),
            (422, "first_boss"),
        ),
        (
            boss_path("runs", "r-two", "1004", "MaleficStar"),
            pick(&*lilac, 0),
            (422, "not_on_run"),
        ),
    ] {
        let method = if path.ends_with("/same") {
            "POST"
        } else {
            "PUT"
        };
        assert_eq!(
            reads.refused(method, &path, body).await,
            (want.0, want.1.into()),
            "{path}"
        );
    }
    // Nothing to clear on the run: answered, nothing recorded.
    let same = same_path("runs", "r-two", "1001");
    reads
        .ok("POST", &same, json!({"versions": {"MaleficStar": 0}}), SLOT)
        .await;
    assert!(rows(&reads, "pick.r-two.1001.*").await.is_empty());
}

#[tokio::test]
async fn per_boss_routes_are_the_generic_404_while_their_module_is_off() {
    let run_boss = boss_path("runs", "r-kalos", "1001", "Kalos");
    let run_same = same_path("runs", "r-kalos", "1001");
    let fixed_boss = boss_path("fixed", "f-kalos", "1001", "Kalos");
    let fixed_same = same_path("fixed", "f-kalos", "1001");
    let reads = Reads::new().await;
    for (method, path) in [
        ("PUT", run_boss.as_str()),
        ("POST", run_same.as_str()),
        ("PUT", fixed_boss.as_str()),
        ("POST", fixed_same.as_str()),
    ] {
        reads.assert_unmounted(method, path, json!({})).await;
    }
    // The roster alone: timing routes need character data too.
    let reads = Reads::with_characters().await;
    for (method, path) in [("PUT", fixed_boss.as_str()), ("POST", fixed_same.as_str())] {
        reads.assert_unmounted(method, path, json!({})).await;
    }
}

/// A "same for every boss" loaded before boss 2 left the timing or run
/// is stale: its nonzero version for that boss names a pick the form saw.
/// Sent at 0, or left out, it goes through (and the kept pick stays).
#[tokio::test]
async fn same_for_every_boss_loaded_before_a_boss_left_is_stale() {
    let Two {
        reads, wisteria, ..
    } = two().await;
    let timing = boss_path("fixed", "f-two", "1001", "MaleficStar");
    let run = boss_path("runs", "r-two", "1001", "MaleficStar");
    for path in [&timing, &run] {
        let slot = reads.ok("PUT", path, pick(&*wisteria, 0), SLOT).await;
        assert_eq!(slot["bosses"][1]["version"], 1, "{path}");
    }
    // HMaleficStar leaves both the timing and its run.
    let mut one = two_boss_run("r-two", Some("f-two"), utc(9, 30, 13));
    one.bosses = vec!["XKalos".into()];
    let changes = vec![
        Change::PutFixedRun(timing_with(&["XKalos"])),
        Change::PutRun(one),
    ];
    commit_as(&reads.store, "edit", utc(9, 29, 0), changes).await;

    for (scope, target) in [("fixed", "f-two"), ("runs", "r-two")] {
        let same = same_path(scope, target, "1001");
        assert_eq!(
            reads
                .refused("POST", &same, json!({"versions": {"MaleficStar": 1}}))
                .await,
            (409, "stale".into()),
            "{same}: loaded while Star was boss 2"
        );
        for body in [
            json!({"versions": {"MaleficStar": 0}}),
            json!({"versions": {}}),
        ] {
            let slot = reads.ok("POST", &same, body, SLOT).await;
            assert_eq!(slot["bosses"].as_array().unwrap().len(), 1, "{same}");
        }
    }
    // Nothing was cleared: the kept picks come back with Star.
    for scope in [PickScope::Timing, PickScope::Run] {
        let target = match scope {
            PickScope::Timing => "f-two",
            PickScope::Run => "r-two",
        };
        let picks = reads
            .store
            .picks(scope, &[target.to_owned()])
            .await
            .unwrap();
        assert_eq!(picks.len(), 1, "{target}");
        assert_eq!(picks[0].boss.as_deref(), Some("MaleficStar"));
    }
    assert!(rows(&reads, "fixedpick.f-two.1001.*").await.is_empty());
    assert!(rows(&reads, "pick.r-two.1001.*").await.is_empty());
}
