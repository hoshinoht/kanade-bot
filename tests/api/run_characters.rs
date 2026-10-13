//! The character each party member brings to a run (module `roster`) over
//! the seeded store of `reads.rs` (pinned clock Tue 29 Sep 12:00 KL; boss
//! weeks reset Thursday 00:00; `r-kalos` XKalos Tue 22:00 with Alice, Bob
//! and Dan; `r-star` a done Star run this week; `n-kalos` XKalos next
//! week). Off, both routes are the generic 404; on, reads resolve what each
//! member brings (the main by default) without writing, warnings name runs
//! of a shared boss this boss week, and picks save directly with CSRF, a
//! slot version, History rows and replays.
//! Invented IGNs only.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use kanade::domain::{
    characters::{CharacterStore, PickScope},
    history::{Actor, ChangeMeta, Origin, Surface},
    schedule::{Change, ChangeSet, Run, RunSource, RunStatus},
    scheduler::{ScheduleStore, Scope},
    settings::{SettingsChange, SettingsChangeQuery, SettingsStore},
};
use serde_json::{Value, json};

use crate::{
    readiness::slot,
    reads::Reads,
    support::{ADMIN_HOST, request, strs},
};

const RUN: &str = "characters.json#/$defs/RunCharacters";
const SLOT: &str = "characters.json#/$defs/RunCharacterSlot";
const ROW: &str = "characters.json#/$defs/CharacterRow";
const KALOS: &str = "/api/admin/runs/r-kalos/characters";

fn pick_path(run: &str, member: &str) -> String {
    format!("/api/admin/runs/{run}/characters/{member}")
}

/// A pick of `character` (`null` clears) from slot `version`.
pub(crate) fn pick(character: impl Into<Value>, version: u64) -> Value {
    json!({"character_id": character.into(), "version": version})
}

/// `(picked, brings, basis, version)` of a slot.
pub(crate) fn picked(slot: &Value) -> (&Value, &Value, &Value, &Value) {
    (
        &slot["picked"],
        &slot["brings"],
        &slot["basis"],
        &slot["version"],
    )
}

/// The refusals a run pick and a timing pick share, plus the route's own
/// `scoped` cases (each labelled by its path and body), then the CSRF
/// check. `wisteria` is retired and `heather` is another member's.
pub(crate) async fn assert_pick_refusals(
    reads: &Reads,
    alice: &str,
    [lilac, heather, wisteria]: [&str; 3],
    stale: u64,
    scoped: Vec<(String, Value, (u16, &str))>,
) {
    let shared = [
        (json!({"character_id": lilac}), (422, "version_required")),
        (json!({"version": 0}), (422, "invalid")),
        (
            json!({"character_id": lilac, "version": 0, "x": 1}),
            (400, "invalid_body"),
        ),
        (pick("c-missing", 0), (404, "not_found")),
        (pick(heather, 0), (422, "not_their_character")),
        (pick(wisteria, 0), (422, "character_retired")),
        (pick(lilac, stale), (409, "stale")),
    ];
    let cases = shared
        .into_iter()
        .map(|(body, want)| (alice.to_owned(), body, want))
        .chain(scoped);
    for (path, body, (status, code)) in cases {
        assert_eq!(
            reads.refused("PUT", &path, body.clone()).await,
            (status, code.into()),
            "{path} {body}"
        );
    }
    let no_csrf = reads.without_csrf("PUT", alice, &pick(lilac, 0)).await;
    assert_eq!(no_csrf.error(), (403, "csrf".into()));
}

/// Retires `character` (at version 1).
pub(crate) async fn retire(reads: &Reads, character: &str) {
    let path = format!("/api/admin/characters/{character}");
    let body = json!({"version": 1, "retired": true});
    reads.ok("PATCH", &path, body, ROW).await;
}

fn utc(month: u32, day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, 0, 0).unwrap()
}

fn run(id: &str, at: DateTime<Utc>, bosses: &[&str], party: &[&str]) -> Run {
    Run {
        id: id.into(),
        fixed_run_id: None,
        channel_id: Some("star".into()),
        // This boss week: Thu 24 Sep 00:00 KL.
        week_start: utc(9, 23, 16),
        datetime: at,
        bosses: bosses.iter().map(|b| (*b).to_owned()).collect(),
        participants: party.iter().map(|p| (*p).to_owned()).collect(),
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

/// More runs this boss week: Chaos Kalos on Wed 30 Sep 21:00 KL with Alice
/// and Cara, and a Star run on Mon 28 Sep 21:00 KL with Alice, still
/// planned and so frozen (past its end).
async fn add_runs(reads: &Reads) {
    let revision = reads.store.load(&Scope::All).await.unwrap().revision;
    let changes = vec![
        Change::PutRun(run(
            "r-ckalos",
            utc(9, 30, 13),
            &["CKalos"],
            &["1001", "1003"],
        )),
        Change::PutRun(run("r-ended", utc(9, 28, 13), &["NMaleficStar"], &["1001"])),
    ];
    let meta = ChangeMeta {
        origin: Origin::new(Actor::admin("seed"), Surface::AdminPortal),
        at: utc(9, 28, 0),
        notices: Vec::new(),
        refs: Vec::new(),
        request_digest: None,
        expect: Default::default(),
        outbox: Vec::new(),
    };
    let set = ChangeSet { changes };
    reads.store.commit(revision, set, meta).await.unwrap();
}

async fn created(reads: &Reads, member: &str, body: Value) -> String {
    let path = format!("/api/admin/members/{member}/characters");
    let reply = reads.call("POST", &path, body, &[]).await;
    let row = reply.expect(201, "characters.json#/$defs/CharacterCreated");
    row["id"].as_str().unwrap().to_owned()
}

async fn records(reads: &Reads) -> Vec<SettingsChange> {
    reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|change| change.section == "characters")
        .collect()
}

async fn stored_picks(reads: &Reads, runs: &[&str]) -> usize {
    let runs: Vec<String> = runs.iter().map(|run| (*run).to_owned()).collect();
    reads
        .store
        .picks(PickScope::Run, &runs)
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn both_routes_are_the_generic_404_while_the_module_is_off() {
    let reads = Reads::new().await;
    let put = pick_path("r-kalos", "1001");
    for (method, path) in [("GET", KALOS), ("PUT", put.as_str())] {
        reads
            .assert_unmounted(method, path, pick(Value::Null, 0))
            .await;
    }
}

#[tokio::test]
async fn the_default_is_recomputed_on_every_read_and_reads_write_nothing() {
    let reads = Reads::with_characters().await;
    let lilac = created(&reads, "1001", json!({"ign": "Lilac", "level": 280})).await;
    let answer = reads.read(KALOS, RUN).await;
    assert_eq!(answer["run_id"], "r-kalos");
    assert_eq!(
        strs(&answer["slots"], "member_id"),
        ["1001", "1002", "1004"],
        "party order"
    );
    let alice = slot(&answer, "1001");
    assert_eq!(
        (&alice["brings"], &alice["basis"]),
        (&json!(lilac), &json!("main")),
        "her first character is her main"
    );
    assert_eq!(alice["picked"], Value::Null);
    assert_eq!(alice["version"], 0);
    assert_eq!(alice["candidates"][0]["ign"], "Lilac");
    let bob = slot(&answer, "1002");
    assert_eq!(bob["candidates"], json!([]), "no characters: an empty slot");
    assert_eq!(
        (&bob["brings"], &bob["basis"]),
        (&Value::Null, &Value::Null)
    );

    // A second active character: the main is still the default.
    let wisteria = created(&reads, "1001", json!({"ign": "Wisteria"})).await;
    let answer = reads.read(KALOS, RUN).await;
    let alice = slot(&answer, "1001");
    assert_eq!(
        (&alice["brings"], &alice["basis"]),
        (&json!(lilac), &json!("main"))
    );
    assert_eq!(alice["candidates"].as_array().unwrap().len(), 2);

    // Retiring it leaves Lilac; the retired one is no candidate but counted.
    retire(&reads, &wisteria).await;
    let answer = reads.read(KALOS, RUN).await;
    let alice = slot(&answer, "1001");
    assert_eq!(alice["brings"], lilac.as_str());
    assert_eq!(alice["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(alice["retired"], 1);

    assert_eq!(
        stored_picks(&reads, &["r-kalos"]).await,
        0,
        "the default is never stored"
    );
    assert_eq!(
        records(&reads).await.len(),
        3,
        "the three character writes only: reads record nothing"
    );
}

#[tokio::test]
async fn the_same_character_on_a_shared_boss_this_boss_week_warns() {
    let reads = Reads::with_characters().await;
    add_runs(&reads).await;
    let lilac = created(&reads, "1001", json!({"ign": "Lilac"})).await;
    // Inferred on r-kalos (XKalos) and on r-ckalos (CKalos), the same boss;
    // not on r-star or r-ended (another boss) or n-kalos (next boss week).
    let answer = reads.read(KALOS, RUN).await;
    assert_eq!(
        slot(&answer, "1001")["warnings"],
        json!([{
            "character_id": lilac,
            "run_id": "r-ckalos",
            "short_id": "rckalos",
            "day": 6,
            "time": "21:00",
            "bosses": ["CKalos"],
            "basis": "main",
        }])
    );
    let next = reads.read("/api/admin/runs/n-kalos/characters", RUN).await;
    assert_eq!(slot(&next, "1001")["warnings"], json!([]));
    // A done run used its clear: r-star (Hard) and r-ended (Normal) share
    // Malefic Star and warn each other.
    let star = reads.read("/api/admin/runs/r-star/characters", RUN).await;
    let warned = &slot(&star, "1001")["warnings"];
    assert_eq!(warned[0]["run_id"], "r-ended");
    assert_eq!(warned[0]["bosses"], json!(["NMaleficStar"]));
    let ended = reads.read("/api/admin/runs/r-ended/characters", RUN).await;
    assert_eq!(slot(&ended, "1001")["warnings"][0]["run_id"], "r-star");

    // Two characters, another picked here: no warning. The warning never
    // blocks a save: picking the same one on r-ckalos goes through.
    let wisteria = created(&reads, "1001", json!({"ign": "Wisteria"})).await;
    let path = pick_path("r-kalos", "1001");
    let mine = reads.ok("PUT", &path, pick(&*wisteria, 0), SLOT).await;
    assert_eq!(mine["picked"], wisteria.as_str());
    assert_eq!(mine["version"], 1);
    assert_eq!(mine["warnings"], json!([]));
    let path = pick_path("r-ckalos", "1001");
    let theirs = reads.ok("PUT", &path, pick(&*wisteria, 0), SLOT).await;
    assert_eq!(theirs["warnings"][0]["run_id"], "r-kalos");
    assert_eq!(theirs["warnings"][0]["basis"], "run_pick");
    let answer = reads.read(KALOS, RUN).await;
    assert_eq!(slot(&answer, "1001")["warnings"][0]["run_id"], "r-ckalos");
}

#[tokio::test]
async fn refusals_write_nothing() {
    let reads = Reads::with_characters().await;
    add_runs(&reads).await;
    let lilac = created(&reads, "1001", json!({"ign": "Lilac"})).await;
    let wisteria = created(&reads, "1001", json!({"ign": "Wisteria"})).await;
    let heather = created(&reads, "1002", json!({"ign": "Heather"})).await;
    retire(&reads, &wisteria).await;
    let alice = pick_path("r-kalos", "1001");
    let scoped = vec![
        (
            pick_path("r-gone", "1001"),
            pick(&*lilac, 0),
            (404, "not_found"),
        ),
        (
            pick_path("r-ended", "1001"),
            pick(&*lilac, 0),
            (409, "run_ended"),
        ),
        (
            pick_path("r-kalos", "1003"),
            pick(Value::Null, 0),
            (422, "not_on_run"),
        ),
    ];
    let ids = [lilac.as_str(), heather.as_str(), wisteria.as_str()];
    assert_pick_refusals(&reads, &alice, ids, 3, scoped).await;
    assert_eq!(
        reads
            .refused("GET", "/api/admin/runs/r-gone/characters", json!({}))
            .await,
        (404, "not_found".into())
    );
    let signed_out = request(reads.admin, "GET", ADMIN_HOST, KALOS, &[]).await;
    assert_eq!(signed_out.status, 401);
    assert_eq!(
        stored_picks(&reads, &["r-kalos", "r-ended"]).await,
        0,
        "nothing written"
    );
    assert_eq!(records(&reads).await.len(), 4, "the character writes only");
}

#[tokio::test]
async fn picks_save_directly_and_replay_across_a_restart() {
    let mut reads = Reads::with_characters().await;
    let lilac = created(&reads, "1001", json!({"ign": "Lilac"})).await;
    let wisteria = created(&reads, "1001", json!({"ign": "Wisteria"})).await;
    let head = reads.version().await;
    let path = pick_path("r-kalos", "1001");
    let body = pick(&*wisteria, 0);
    let key = [("Idempotency-Key", "pick-k")];
    let first = reads.call("PUT", &path, body.clone(), &key).await;
    assert_eq!(first.expect(200, SLOT)["picked"], wisteria.as_str());

    reads.restart(TimeDelta::zero(), None).await;
    let again = reads.call("PUT", &path, body, &key).await;
    assert_eq!(
        (again.status, again.json()),
        (200, first.json()),
        "the first answer"
    );
    let other = reads.call("PUT", &path, pick(&*lilac, 1), &key).await;
    assert_eq!(other.error(), (422, "idempotency_mismatch".into()));
    assert_eq!(
        reads.version().await,
        head,
        "a pick is outside the history chain"
    );
    let picks: Vec<SettingsChange> = records(&reads)
        .await
        .into_iter()
        .filter(|change| change.values.contains_key("pick.r-kalos.1001"))
        .collect();
    assert_eq!(picks.len(), 1, "one History row, the replay none");
    assert!(picks[0].values["pick.r-kalos.1001"].to.contains("Wisteria"));

    // The same character again is a no-op; a clear frees the slot.
    let same = reads.ok("PUT", &path, pick(&*wisteria, 1), SLOT).await;
    assert_eq!(same["version"], 1);
    let cleared = reads.ok("PUT", &path, pick(Value::Null, 1), SLOT).await;
    assert_eq!(
        picked(&cleared),
        (&Value::Null, &json!(lilac), &json!("main"), &json!(0)),
        "back to the main"
    );
    assert_eq!(stored_picks(&reads, &["r-kalos"]).await, 0);
    assert_eq!(
        records(&reads)
            .await
            .iter()
            .filter(|change| change.values.contains_key("pick.r-kalos.1001"))
            .count(),
        2,
        "the pick and the clear"
    );
}

#[tokio::test]
async fn deleting_a_character_drops_its_picks_and_party_leavers_drop_out() {
    let reads = Reads::with_characters().await;
    let lilac = created(&reads, "1001", json!({"ign": "Lilac"})).await;
    let wisteria = created(&reads, "1001", json!({"ign": "Wisteria"})).await;
    let heather = created(&reads, "1004", json!({"ign": "Heather"})).await;
    for (member, character) in [("1001", &wisteria), ("1004", &heather)] {
        let path = pick_path("r-kalos", member);
        reads.ok("PUT", &path, pick(&**character, 0), SLOT).await;
    }
    assert_eq!(stored_picks(&reads, &["r-kalos"]).await, 2);
    reads
        .ok(
            "DELETE",
            &format!("/api/admin/characters/{wisteria}"),
            json!({}),
            "common.json#/$defs/Message",
        )
        .await;
    assert_eq!(stored_picks(&reads, &["r-kalos"]).await, 1);
    let answer = reads.read(KALOS, RUN).await;
    assert_eq!(
        picked(slot(&answer, "1001")),
        (&Value::Null, &json!(lilac), &json!("main"), &json!(0))
    );

    // Dan leaves the party: his slot is gone and his pick is not returned.
    let version = reads.version().await;
    reads
        .ok(
            "PATCH",
            "/api/admin/runs/r-kalos/participants",
            json!({"remove": "1004", "version": version}),
            "week.json#/$defs/RunResult",
        )
        .await;
    let answer = reads.read(KALOS, RUN).await;
    assert!(!strs(&answer["slots"], "member_id").contains(&"1004"));
    assert_eq!(
        reads
            .refused("PUT", &pick_path("r-kalos", "1004"), pick(Value::Null, 1))
            .await,
        (422, "not_on_run".into())
    );
}
