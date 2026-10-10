//! The member's own runs: answer, move within the current boss week, and the
//! run deep link Discord posts.

use chrono::{NaiveTime, TimeDelta};
use kanade::{
    api::auth::{
        audit::{AuditEvent, Realm},
        rate::MEMBER_WRITE_ROUTE,
    },
    domain::{
        history::{Actor, Surface},
        schedule::{Change, RunStatus},
        scheduler::{ScheduleStore, Scope},
    },
};
use serde_json::{Value, json};

use super::{ALICE, BOB, DAN, Portal, keys, refused, run, this_week, utc};
use crate::{reads::Reads, schemas::assert_valid, support::Reply};

const RUN_KEYS: [&str; 13] = [
    "bosses",
    "can_edit",
    "channel",
    "day",
    "fixed_id",
    "id",
    "mine",
    "minutes",
    "participants",
    "party",
    "status",
    "tally",
    "time",
];
const RESULT_KEYS: [&str; 2] = ["run", "version"];
const MOVE_KEYS: [&str; 3] = ["previous", "run", "version"];
const LINK_KEYS: [&str; 9] = [
    "generated_at",
    "removed",
    "run",
    "started",
    "this_week",
    "timing",
    "week",
    "week_ends_at",
    "week_starts",
];

fn answer_path(id: &str) -> String {
    format!("/api/public/runs/{id}/answer")
}

fn move_path(id: &str) -> String {
    format!("/api/public/runs/{id}/move")
}

fn link_path(id: &str) -> String {
    format!("/api/public/runs/{id}")
}

/// Last boss week: Thursday 17 Sep 00:00 KL.
fn last_week() -> chrono::DateTime<chrono::Utc> {
    utc(9, 16, 16, 0)
}

/// Besides the seed (r-kalos Tue 22:00 for Alice, Bob and Dan; r-star done;
/// next week's n-star for Dan and n-kalos): Dan's own-time run tonight,
/// Alice's run that started this morning, her cancelled one tomorrow and
/// last week's run of the Tuesday timing for Alice and Bob.
fn extra() -> Vec<Change> {
    let mut past = run(
        "p-kalos",
        last_week(),
        utc(9, 22, 14, 0),
        &[ALICE, BOB],
        RunStatus::Planned,
    );
    past.fixed_run_id = Some("f-kalos".into());
    past.channel_id = Some("kalos-four".into());
    vec![
        Change::PutRun(run(
            "r-own",
            this_week(),
            utc(9, 29, 12, 0),
            &[DAN],
            RunStatus::Otot,
        )),
        Change::PutRun(run(
            "r-early",
            this_week(),
            utc(9, 29, 2, 0),
            &[ALICE],
            RunStatus::Planned,
        )),
        Change::PutRun(run(
            "r-off",
            this_week(),
            utc(9, 30, 12, 0),
            &[ALICE],
            RunStatus::Cancelled,
        )),
        Change::PutRun(past),
    ]
}

async fn portal() -> Portal {
    let portal = Portal::new().await;
    portal.seed(extra()).await;
    portal
}

fn checked(reply: &Reply, target: &str, top: &[&str]) -> Value {
    assert_eq!(reply.status, 200, "{}", reply.text());
    let value = reply.json();
    assert_valid(target, "write", &value);
    assert_eq!(keys(&value), top.iter().copied().collect());
    assert_eq!(keys(&value["run"]), RUN_KEYS.into());
    value
}

fn answer_of(run: &Value, member: u64) -> &str {
    run["participants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"].as_str().and_then(|id| id.parse().ok()) == Some(member))
        .and_then(|p| p["answer"].as_str())
        .unwrap_or("not on the run")
}

impl Portal {
    async fn link(&self, browser: &super::Browser, id: &str) -> Value {
        let value = self
            .read(browser, &link_path(id), "public.json#/$defs/MemberRunLink")
            .await;
        assert_eq!(keys(&value), LINK_KEYS.into());
        assert_eq!(keys(&value["run"]), RUN_KEYS.into());
        value
    }

    async fn stored(&self, id: &str) -> kanade::domain::schedule::Run {
        self.reads
            .store
            .load(&Scope::Run(id.into()))
            .await
            .unwrap()
            .runs
            .into_iter()
            .find(|run| run.id == id)
            .unwrap()
    }
}

#[tokio::test]
async fn members_answer_their_own_runs_through_the_admin_path() {
    let portal = portal().await;
    let (alice, bob, dan) = (
        portal.sign_in(ALICE).await,
        portal.sign_in(BOB).await,
        portal.sign_in(DAN).await,
    );
    let v0 = portal.version().await;
    let maybe = json!({"answer": "maybe", "version": v0});
    let reply = portal
        .write(&dan, "PUT", &answer_path("r-kalos"), "a-1", Some(&maybe))
        .await;
    let first = checked(&reply, "public.json#/$defs/MemberRunResult", &RESULT_KEYS);
    assert_eq!(answer_of(&first["run"], DAN), "maybe");
    assert_eq!(
        (
            first["run"]["mine"].clone(),
            first["run"]["can_edit"].clone()
        ),
        (json!(true), json!(true))
    );
    let head = portal.head().await;
    assert_eq!(
        (
            head.origin.actor.clone(),
            head.origin.surface,
            head.origin.request_id.as_deref()
        ),
        (
            Actor::member("1004"),
            Surface::PublicPortal,
            Some("public:a-1")
        )
    );
    assert_eq!(first["version"], head.seq);

    // An exact retry answers the first result and records nothing.
    let again = portal
        .write(&dan, "PUT", &answer_path("r-kalos"), "a-1", Some(&maybe))
        .await;
    assert_eq!(again.status, 200);
    assert_eq!(again.json(), first);
    assert_eq!(portal.version().await, head.seq, "no second record");
    let other = json!({"answer": "yes", "version": v0});
    let mismatch = portal
        .write(&dan, "PUT", &answer_path("r-kalos"), "a-1", Some(&other))
        .await;
    assert_eq!(refused(&mismatch), (422, "idempotency_mismatch".into()));

    // Stale only against the member's own answer.
    let stale = portal
        .write(&dan, "PUT", &answer_path("r-kalos"), "a-2", Some(&other))
        .await;
    assert_eq!(refused(&stale), (409, "stale".into()));
    let v1 = portal.version().await;
    let alice_yes = json!({"answer": "no", "version": v1});
    let reply = portal
        .write(
            &alice,
            "PUT",
            &answer_path("r-kalos"),
            "a-3",
            Some(&alice_yes),
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let dan_yes = json!({"answer": "yes", "version": v1});
    let reply = portal
        .write(&dan, "PUT", &answer_path("r-kalos"), "a-4", Some(&dan_yes))
        .await;
    let run = checked(&reply, "public.json#/$defs/MemberRunResult", &RESULT_KEYS);
    assert_eq!(
        (answer_of(&run["run"], DAN), answer_of(&run["run"], ALICE)),
        ("yes", "no"),
        "Alice's later answer never made Dan's stale"
    );

    // Next week's runs take answers too.
    let next = json!({"answer": "yes", "version": portal.version().await});
    let reply = portal
        .write(&alice, "PUT", &answer_path("n-kalos"), "a-5", Some(&next))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());

    let v = portal.version().await;
    let yes = json!({"answer": "yes", "version": v});
    for (browser, id, expected) in [
        (&alice, "r-nope", (404, "not_found")),
        (&alice, "p-kalos", (404, "not_found")),
        (&bob, "n-star", (403, "not_in_run")),
        (&alice, "r-star", (409, "run_closed")),
        (&alice, "r-off", (409, "run_closed")),
    ] {
        let reply = portal
            .write(
                browser,
                "PUT",
                &answer_path(id),
                &format!("x-{id}"),
                Some(&yes),
            )
            .await;
        assert_eq!(refused(&reply), (expected.0, expected.1.into()), "{id}");
    }
    for (body, what) in [
        (json!({"answer": "clear", "version": v}), "no clear"),
        (json!({"answer": "yes"}), "no version"),
        (
            json!({"answer": "yes", "version": v, "member_id": "1001"}),
            "extra field",
        ),
    ] {
        let reply = portal
            .write(&dan, "PUT", &answer_path("r-kalos"), "b-1", Some(&body))
            .await;
        assert_eq!(refused(&reply), (422, "invalid_body".into()), "{what}");
    }
    assert_eq!(portal.version().await, v, "nothing refused was recorded");
}

#[tokio::test]
async fn members_move_their_own_runs_within_this_boss_week() {
    let portal = portal().await;
    let (alice, bob, dan) = (
        portal.sign_in(ALICE).await,
        portal.sign_in(BOB).await,
        portal.sign_in(DAN).await,
    );
    let v0 = portal.version().await;
    // Tue 22:00 → Wed 21:00 (day 6).
    let to_wed = json!({"day": 6, "time": "21:00", "version": v0});
    let reply = portal
        .write(&dan, "POST", &move_path("r-kalos"), "m-1", Some(&to_wed))
        .await;
    let first = checked(&reply, "public.json#/$defs/MemberMoveResult", &MOVE_KEYS);
    assert_eq!(first["previous"], json!({"day": 5, "time": "22:00"}));
    assert_eq!(
        (first["run"]["day"].clone(), first["run"]["time"].clone()),
        (json!(6), json!("21:00"))
    );
    let head = portal.head().await;
    assert_eq!(
        (head.origin.actor.clone(), head.origin.surface),
        (Actor::member("1004"), Surface::PublicPortal)
    );
    assert!(
        !head.notices.is_empty(),
        "the party is told, as for an admin move"
    );
    assert_eq!(portal.stored("r-kalos").await.datetime, utc(9, 30, 13, 0));

    let again = portal
        .write(&dan, "POST", &move_path("r-kalos"), "m-1", Some(&to_wed))
        .await;
    assert_eq!(again.status, 200);
    assert_eq!(again.json(), first, "the first result, `previous` included");
    assert_eq!(portal.version().await, head.seq, "no second record");
    let other = json!({"day": 6, "time": "20:00", "version": v0});
    let mismatch = portal
        .write(&dan, "POST", &move_path("r-kalos"), "m-1", Some(&other))
        .await;
    assert_eq!(refused(&mismatch), (422, "idempotency_mismatch".into()));
    let stale = portal
        .write(&alice, "POST", &move_path("r-kalos"), "m-2", Some(&other))
        .await;
    assert_eq!(refused(&stale), (409, "stale".into()));

    let v = portal.version().await;
    let at = |day: u8, time: Value| json!({"day": day, "time": time, "version": v});
    for (browser, id, body, expected) in [
        (&alice, "r-nope", at(6, json!("21:00")), (404, "not_found")),
        (&alice, "p-kalos", at(6, json!("21:00")), (404, "not_found")),
        (&bob, "n-star", at(6, json!("21:00")), (403, "not_in_run")),
        (&alice, "n-kalos", at(6, json!("21:00")), (409, "week_over")),
        (&alice, "r-off", at(6, json!("21:00")), (409, "run_closed")),
        (&alice, "r-star", at(6, json!("21:00")), (409, "run_closed")),
        // Started hours ago, so past its end too: frozen until it is settled.
        (&alice, "r-early", at(6, json!("21:00")), (409, "run_ended")),
        (&dan, "r-kalos", at(2, json!("21:00")), (422, "in_the_past")),
        (&dan, "r-kalos", at(5, json!("11:00")), (422, "in_the_past")),
        (
            &dan,
            "r-kalos",
            at(6, json!("25:00")),
            (422, "invalid_time"),
        ),
        (&dan, "r-kalos", at(6, json!("9:00")), (422, "invalid_time")),
        (&dan, "r-kalos", at(6, Value::Null), (422, "invalid_time")),
        (
            &dan,
            "r-kalos",
            at(7, json!("21:00")),
            (422, "invalid_body"),
        ),
    ] {
        let reply = portal
            .write(
                browser,
                "POST",
                &move_path(id),
                &format!("x-{id}"),
                Some(&body),
            )
            .await;
        assert_eq!(
            refused(&reply),
            (expected.0, expected.1.into()),
            "{id} {body}"
        );
    }
    assert_eq!(portal.version().await, v, "nothing refused was recorded");

    // An own-time run keeps its clock, or gets a time (this week only).
    let keep = json!({"day": 6, "time": null, "version": v});
    let reply = portal
        .write(&dan, "POST", &move_path("r-own"), "o-1", Some(&keep))
        .await;
    let kept = checked(&reply, "public.json#/$defs/MemberMoveResult", &MOVE_KEYS);
    assert_eq!(kept["previous"], json!({"day": 5, "time": null}));
    assert_eq!(kept["run"]["day"], 6);
    assert_eq!(portal.stored("r-own").await.datetime, utc(9, 30, 12, 0));
    let timed = json!({"day": 6, "time": "21:30", "version": kept["version"]});
    let reply = portal
        .write(&dan, "POST", &move_path("r-own"), "o-2", Some(&timed))
        .await;
    checked(&reply, "public.json#/$defs/MemberMoveResult", &MOVE_KEYS);
    let stored = portal.stored("r-own").await;
    assert_eq!(
        (stored.datetime, stored.status),
        (utc(9, 30, 13, 30), RunStatus::Otot)
    );
}

#[tokio::test]
async fn a_move_never_leaves_the_boss_week() {
    // Weeks reset Thursday 12:00, so Thursday morning is last week.
    let portal =
        Portal::over(Reads::with_reset(NaiveTime::from_hms_opt(12, 0, 0).unwrap()).await).await;
    let dan = portal.sign_in(DAN).await;
    let body = json!({"day": 0, "time": "11:00", "version": portal.version().await});
    let reply = portal
        .write(&dan, "POST", &move_path("r-kalos"), "w-1", Some(&body))
        .await;
    assert_eq!(refused(&reply), (422, "outside_week".into()));
}

#[tokio::test]
async fn run_links_show_the_week_the_slot_and_who_took_you_off() {
    let portal = portal().await;
    let (alice, bob, dan) = (
        portal.sign_in(ALICE).await,
        portal.sign_in(BOB).await,
        portal.sign_in(DAN).await,
    );
    let link = portal.link(&bob, "r-kalos").await;
    assert_eq!(
        (
            link["week"].clone(),
            link["week_starts"].clone(),
            link["week_ends_at"].clone(),
            link["started"].clone(),
            link["removed"].clone(),
            link["this_week"].clone(),
            link["timing"].clone(),
        ),
        (
            json!("current"),
            json!("2026-09-24"),
            json!("2026-09-30T16:00:00Z"),
            json!(false),
            Value::Null,
            Value::Null,
            json!({"day": 1, "time": "22:00"}),
        )
    );
    assert_eq!(link["run"]["mine"], true);
    let next = portal.link(&bob, "n-star").await;
    assert_eq!(
        (
            next["week"].clone(),
            next["timing"].clone(),
            next["run"]["mine"].clone(),
            next["run"]["can_edit"].clone()
        ),
        (json!("next"), Value::Null, json!(false), json!(false))
    );
    let started = portal.link(&alice, "r-early").await;
    assert_eq!(started["started"], true);

    // An admin takes Bob off: the link says so, and his writes are refused.
    let v = portal.version().await;
    portal
        .reads
        .ok(
            "PATCH",
            "/api/admin/runs/r-kalos/participants",
            json!({"remove": BOB.to_string(), "version": v}),
            "week.json#/$defs/RunResult",
        )
        .await;
    let gone = portal.link(&bob, "r-kalos").await;
    assert_eq!(
        gone["removed"],
        json!({"by": "an admin", "at": "2026-09-29T04:00:00Z"})
    );
    assert_eq!(
        (gone["run"]["mine"].clone(), gone["run"]["can_edit"].clone()),
        (json!(false), json!(false))
    );
    let late = json!({"answer": "yes", "version": v});
    let reply = portal
        .write(&bob, "PUT", &answer_path("r-kalos"), "late-1", Some(&late))
        .await;
    assert_eq!(refused(&reply), (403, "not_in_run".into()));

    // Last week's run: its party sees it, with this week's run of the timing.
    let past = portal.link(&alice, "p-kalos").await;
    assert_eq!(
        (
            past["week"].clone(),
            past["started"].clone(),
            past["run"]["can_edit"].clone(),
            past["this_week"]["id"].clone(),
            past["timing"].clone(),
        ),
        (
            json!("past"),
            json!(true),
            json!(false),
            json!("r-kalos"),
            json!({"day": 1, "time": "22:00"}),
        )
    );
    assert_eq!(keys(&past["this_week"]), RUN_KEYS.into());
    assert_eq!(portal.link(&bob, "p-kalos").await["week"], "past");
    for (browser, id) in [(&dan, "p-kalos"), (&dan, "r-nope")] {
        let reply = portal.get(Some(browser), &link_path(id)).await;
        assert_eq!(refused(&reply), (404, "not_found".into()), "{id}");
    }
    // Anyone may open this or next week's runs.
    assert_eq!(portal.link(&dan, "r-star").await["run"]["mine"], false);
}

#[tokio::test]
async fn member_run_writes_need_a_fresh_sign_in_csrf_a_key_and_a_session() {
    let portal = portal().await;
    let dan = portal.sign_in(DAN).await;
    let v = portal.version().await;
    let yes = json!({"answer": "yes", "version": v}).to_string();
    let csrf = ("X-Kanade-CSRF", dan.csrf.as_str());
    let key = ("Idempotency-Key", "k-1");
    let origin = super::PUB_ORIGIN;
    for (method, path) in [
        ("PUT", answer_path("r-kalos")),
        ("POST", move_path("r-kalos")),
    ] {
        let reply = portal
            .send(Some(&dan), method, &path, &[origin, csrf], Some(&yes))
            .await;
        assert_eq!(
            refused(&reply),
            (400, "invalid_idempotency_key".into()),
            "{path}"
        );
        let reply = portal
            .send(Some(&dan), method, &path, &[origin, key], Some(&yes))
            .await;
        assert_eq!(refused(&reply), (403, "csrf".into()), "{path}");
        let reply = portal
            .send(None, method, &path, &[origin, key], Some(&yes))
            .await;
        assert_eq!(refused(&reply), (401, "unauthenticated".into()), "{path}");
    }
    let reply = portal.get(None, &link_path("r-kalos")).await;
    assert_eq!(refused(&reply), (401, "unauthenticated".into()));

    // Past the fresh-write window: sign in again, then it goes through.
    portal.harness.advance(TimeDelta::minutes(16));
    let answer = json!({"answer": "yes", "version": v});
    let moved = json!({"day": 6, "time": "21:00", "version": v});
    let reply = portal
        .write(&dan, "PUT", &answer_path("r-kalos"), "f-1", Some(&answer))
        .await;
    assert_eq!(refused(&reply), (401, "reauth_required".into()));
    let reply = portal
        .write(&dan, "POST", &move_path("r-kalos"), "f-2", Some(&moved))
        .await;
    assert_eq!(refused(&reply), (401, "reauth_required".into()));
    portal.link(&dan, "r-kalos").await;
    assert_eq!(portal.version().await, v);
    let dan = portal.sign_in(DAN).await;
    let reply = portal
        .write(&dan, "PUT", &answer_path("r-kalos"), "f-3", Some(&answer))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());

    // Losing the role ends the session.
    portal.harness.member.member_changed("1004", false).await;
    let reply = portal
        .write(&dan, "PUT", &answer_path("r-kalos"), "f-4", Some(&answer))
        .await;
    assert_eq!(refused(&reply), (401, "unauthenticated".into()));

    let alice = portal.sign_in(ALICE).await;
    portal.harness.set_open(false);
    for (method, path) in [
        ("PUT", answer_path("r-kalos")),
        ("POST", move_path("r-kalos")),
        ("GET", link_path("r-kalos")),
    ] {
        let reply = portal
            .write(&alice, method, &path, "c-1", Some(&answer))
            .await;
        assert_eq!(refused(&reply), (503, "closed".into()), "{method} {path}");
    }
}

#[tokio::test]
async fn member_run_writes_take_the_member_write_tokens() {
    let portal = portal().await;
    let dan = portal.sign_in(DAN).await;
    let body = json!({"answer": "yes", "version": portal.version().await});
    for n in 0..20 {
        let reply = portal
            .write(
                &dan,
                "PUT",
                &answer_path("r-nope"),
                &format!("r-{n}"),
                Some(&body),
            )
            .await;
        assert_eq!(refused(&reply), (404, "not_found".into()), "write {n}");
    }
    let limited = portal
        .write(&dan, "PUT", &answer_path("r-kalos"), "r-20", Some(&body))
        .await;
    assert_eq!(refused(&limited), (429, "rate_limited".into()));
    assert!(
        portal
            .harness
            .audit
            .events()
            .contains(&AuditEvent::RateLimited {
                route: MEMBER_WRITE_ROUTE
            })
    );
}

/// Every refused member write is a log line naming the member, the route
/// and the reason: CSRF, a stale sign-in, and the 403/404/409 codes. None of
/// them is a stored sign-in row; a refusal without a member (no session) and
/// an invalid body are not write refusals.
#[tokio::test]
async fn refused_member_writes_are_audited_with_route_and_reason() {
    let portal = portal().await;
    let alice = portal.sign_in(ALICE).await;
    let bob = portal.sign_in(BOB).await;
    let v = portal.version().await;
    let yes = json!({"answer": "yes", "version": v});
    for (browser, id, key) in [
        (&alice, "r-nope", "w-1"),
        (&bob, "n-star", "w-2"),
        (&alice, "r-star", "w-3"),
    ] {
        let reply = portal
            .write(browser, "PUT", &answer_path(id), key, Some(&yes))
            .await;
        assert!(matches!(reply.status, 403 | 404 | 409), "{}", reply.text());
    }
    let reply = portal
        .write(
            &alice,
            "PUT",
            &answer_path("r-kalos"),
            "w-4",
            Some(&json!({"answer": "?"})),
        )
        .await;
    assert_eq!(refused(&reply), (422, "invalid_body".into()));
    let body = yes.to_string();
    let reply = portal
        .send(
            Some(&alice),
            "POST",
            &move_path("r-kalos"),
            &[super::PUB_ORIGIN, ("Idempotency-Key", "w-5")],
            Some(&body),
        )
        .await;
    assert_eq!(refused(&reply), (403, "csrf".into()));
    let reply = portal
        .send(
            None,
            "PUT",
            &answer_path("r-kalos"),
            &[super::PUB_ORIGIN],
            Some(&body),
        )
        .await;
    assert_eq!(refused(&reply), (401, "unauthenticated".into()));
    portal.harness.advance(TimeDelta::minutes(16));
    let reply = portal
        .write(&alice, "PUT", &answer_path("r-kalos"), "w-6", Some(&yes))
        .await;
    assert_eq!(refused(&reply), (401, "reauth_required".into()));

    let refusals: Vec<(String, String, String)> = portal
        .harness
        .audit
        .records()
        .into_iter()
        .filter_map(|record| {
            let stored = record.row().is_some();
            match record.event {
                AuditEvent::WriteRefused {
                    actor,
                    route,
                    reason,
                } => {
                    assert_eq!(record.realm, Realm::Member);
                    assert!(!stored, "never a stored row");
                    Some((actor, route, reason))
                }
                _ => None,
            }
        })
        .collect();
    let row = |member: u64, method: &str, path: String, reason: &str| {
        (
            format!("discord:{member}"),
            format!("{method} {path}"),
            reason.to_owned(),
        )
    };
    assert_eq!(
        refusals,
        [
            row(ALICE, "PUT", answer_path("r-nope"), "not_found"),
            row(BOB, "PUT", answer_path("n-star"), "not_in_run"),
            row(ALICE, "PUT", answer_path("r-star"), "run_closed"),
            row(ALICE, "POST", move_path("r-kalos"), "csrf"),
            row(ALICE, "PUT", answer_path("r-kalos"), "reauth_required"),
        ]
    );
}

/// The writer re-checks a member's write inside the commit: a member taken
/// off the run after the handler's checks is refused and nothing is written.
#[tokio::test]
async fn the_writer_refuses_a_member_taken_off_after_the_checks() {
    use kanade::{
        api::write::{RunWrite, WriteContext},
        domain::{
            history::{Expect, Origin},
            members::Roster,
            schedule::{MemberRunRefusal, RsvpState, ScheduleError},
            scheduler::{DeclineNoticeContext, SchedulerError},
        },
    };
    let portal = portal().await;
    let v = portal.version().await;
    portal
        .reads
        .ok(
            "PATCH",
            "/api/admin/runs/r-kalos/participants",
            json!({"remove": BOB.to_string(), "version": v}),
            "week.json#/$defs/RunResult",
        )
        .await;
    let state = portal.reads.site.state.clone().unwrap();
    let ctx = WriteContext {
        policy: state.policy.clone(),
        directory: Roster::new(),
    };
    let bob = || {
        Origin::new(Actor::member(BOB.to_string()), Surface::PublicPortal)
            .with_request_id("public:race-1".to_owned())
    };
    let refused = Err::<(), _>(SchedulerError::Schedule(ScheduleError::MemberRun(
        MemberRunRefusal::NotInRun,
    )));
    let moved = state
        .writer
        .run(
            bob(),
            Expect::default(),
            "r-kalos",
            RunWrite::MemberMove {
                to: utc(9, 30, 13, 0),
            },
            &ctx,
        )
        .await;
    assert_eq!(moved, refused);
    let decline = DeclineNoticeContext {
        channel_id: None,
        reference_id: None,
        display_name: "Bob".into(),
    };
    let answered = state
        .writer
        .member_answer(
            bob(),
            Expect::default(),
            "r-kalos",
            RsvpState::Yes,
            decline,
            &ctx,
        )
        .await
        .map(drop);
    assert_eq!(answered, refused);
    // Next week's run is not his to move: refused in the commit too.
    let dan = Origin::new(Actor::member(DAN.to_string()), Surface::PublicPortal);
    let star = state
        .writer
        .run(
            dan,
            Expect::default(),
            "n-star",
            RunWrite::MemberMove {
                to: utc(10, 3, 13, 0),
            },
            &ctx,
        )
        .await;
    assert_eq!(
        star,
        Err(SchedulerError::Schedule(ScheduleError::MemberRun(
            MemberRunRefusal::WeekOver
        )))
    );
    assert_eq!(portal.version().await, v + 1, "only the admin's removal");
}

/// A retry answers before the live checks: the first result after the run
/// changed under it, and a changed body is still a mismatch.
#[tokio::test]
async fn retries_answer_before_the_live_checks() {
    let portal = portal().await;
    let dan = portal.sign_in(DAN).await;
    let v = portal.version().await;
    let answer = json!({"answer": "yes", "version": v});
    let to_wed = json!({"day": 6, "time": "21:00", "version": v});
    let reply = portal
        .write(&dan, "PUT", &answer_path("r-kalos"), "a-1", Some(&answer))
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let moved = portal
        .write(&dan, "POST", &move_path("r-kalos"), "m-1", Some(&to_wed))
        .await;
    let first = checked(&moved, "public.json#/$defs/MemberMoveResult", &MOVE_KEYS);
    let w = portal.version().await;
    portal
        .reads
        .ok(
            "PATCH",
            "/api/admin/runs/r-kalos/participants",
            json!({"remove": DAN.to_string(), "version": w}),
            "week.json#/$defs/RunResult",
        )
        .await;
    let removed = portal.version().await;
    let again = portal
        .write(&dan, "PUT", &answer_path("r-kalos"), "a-1", Some(&answer))
        .await;
    assert_eq!(again.status, 200, "{}", again.text());
    let again = portal
        .write(&dan, "POST", &move_path("r-kalos"), "m-1", Some(&to_wed))
        .await;
    let again = checked(&again, "public.json#/$defs/MemberMoveResult", &MOVE_KEYS);
    assert_eq!(again["previous"], first["previous"]);
    for (method, path, key, body) in [
        (
            "PUT",
            answer_path("r-kalos"),
            "a-1",
            json!({"answer": "no", "version": v}),
        ),
        (
            "POST",
            move_path("r-kalos"),
            "m-1",
            json!({"day": 6, "time": "20:00", "version": v}),
        ),
    ] {
        let reply = portal.write(&dan, method, &path, key, Some(&body)).await;
        assert_eq!(
            refused(&reply),
            (422, "idempotency_mismatch".into()),
            "{path}"
        );
    }
    assert_eq!(portal.version().await, removed, "no retry wrote anything");
}

/// A move to where the run already is writes nothing and tells nobody.
#[tokio::test]
async fn a_move_to_the_current_slot_writes_nothing() {
    let portal = portal().await;
    let dan = portal.sign_in(DAN).await;
    let v = portal.version().await;
    let same = json!({"day": 5, "time": "22:00", "version": v});
    let reply = portal
        .write(&dan, "POST", &move_path("r-kalos"), "s-1", Some(&same))
        .await;
    let value = checked(&reply, "public.json#/$defs/MemberMoveResult", &MOVE_KEYS);
    assert_eq!(value["previous"], json!({"day": 5, "time": "22:00"}));
    assert_eq!(value["version"], v);
    assert_eq!(portal.version().await, v);
}
