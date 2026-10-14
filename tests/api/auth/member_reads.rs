//! Member reads (`member-reads`) on the public origin: the boss week, the
//! member's own chat allowance and boss art, behind the member session, over
//! the admin reads' seeded store and pinned clock (Tue 29 Sep 2026 12:00 in
//! Kuala Lumpur; boss weeks reset Thursday).

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, NaiveTime, TimeDelta, Utc};
use kanade::{
    api::auth::{audit::AuditEvent, rate::MEMBER_READ_ROUTE},
    domain::{
        members::MemberStore,
        schedule::{Change, Run, RunSource, RunStatus},
    },
    infrastructure::llm::governor::{CallKind, GroupSnapshot, QueuedCall},
};
use serde_json::{Value, json};

use super::{
    member_support::Browser,
    member_writes::{ALICE, BOB, CARA, DAN, IP, Portal, all_keys, keys, set, utc},
};
use crate::{
    reads::{Reads, commit_as, snapshot},
    support::{ADMIN_HOST, find, items, request},
};

const WEEK: &str = "/api/public/week";
const ALLOWANCE: &str = "/api/public/me/allowance";
const ART: &str = "/art/portraits/MaleficStar";

// Key sets (see `set`).
const WEEK_KEYS: &str = "days generated_at reset runs starts timezone version";
const RUN_KEYS: &str =
    "bosses can_edit channel day fixed_id id mine minutes participants party status tally time";
const ALLOWANCE_KEYS: &str = "allowance bot_busy generated_at queue_position resets_at used";
/// Admin-only fields that must never reach a member, at any depth.
const FORBIDDEN: &str =
    "short_id channel_id cards amended roster_change role roles role_id role_ids";
/// `MemberRun` fields that are the admin run's own.
const SHARED: &str = "id day time minutes status bosses tally participants party channel fixed_id";

impl Portal {
    async fn week(&self, browser: &Browser, query: &str) -> Value {
        let path = format!("{WEEK}{query}");
        self.read(browser, &path, "public.json#/$defs/MemberWeek")
            .await
    }

    async fn allowance(&self, browser: &Browser) -> Value {
        self.read(browser, ALLOWANCE, "public.json#/$defs/MemberAllowance")
            .await
    }
}

fn runs(week: &Value) -> &Vec<Value> {
    items(&week["runs"])
}

fn run<'a>(week: &'a Value, id: &str) -> &'a Value {
    find(&week["runs"], "id", id)
}

fn flags(week: &Value, id: &str) -> (bool, bool) {
    let run = run(week, id);
    (
        run["mine"].as_bool().unwrap(),
        run["can_edit"].as_bool().unwrap(),
    )
}

/// Exactly the frozen keys, nothing admin-only at any depth.
fn assert_member_week_shape(week: &Value) {
    assert_eq!(keys(week), set(WEEK_KEYS));
    for run in runs(week) {
        assert_eq!(keys(run), set(RUN_KEYS), "{run}");
    }
    let mut seen = BTreeSet::new();
    all_keys(week, &mut seen);
    for key in set(FORBIDDEN) {
        assert!(!seen.contains(key), "{key} leaked: {week:#}");
    }
}

/// The member week is the admin week's frame, run set and order, and each
/// run's shared fields are the admin run's.
fn assert_matches_admin(member: &Value, admin: &Value) {
    for key in set("starts timezone reset days generated_at version") {
        assert_eq!(member[key], admin[key], "{key}");
    }
    let ids =
        |week: &Value| -> Vec<Value> { runs(week).iter().map(|run| run["id"].clone()).collect() };
    assert_eq!(ids(member), ids(admin));
    for (mine, theirs) in runs(member).iter().zip(runs(admin)) {
        for key in set(SHARED) {
            assert_eq!(mine[key], theirs[key], "{} {key}", mine["id"]);
        }
    }
}

async fn commit(reads: &Reads, changes: Vec<Change>) {
    commit_as(&reads.store, "test", utc(9, 29, 3, 0), changes).await;
}

fn put_run(
    id: &str,
    week: DateTime<Utc>,
    at: DateTime<Utc>,
    party: &[&str],
    status: RunStatus,
) -> Change {
    Change::PutRun(Run {
        id: id.into(),
        fixed_run_id: None,
        channel_id: Some("star".into()),
        week_start: week,
        datetime: at,
        bosses: vec!["NMaleficStar".into()],
        participants: party.iter().map(|p| (*p).to_owned()).collect(),
        status,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    })
}

#[tokio::test]
async fn the_member_week_is_the_admin_week_with_only_member_fields() {
    let portal = Portal::new().await;
    // A cancelled run of Alice's this week (Fri 25 Sep 21:00 KL).
    commit(
        &portal.reads,
        vec![put_run(
            "r-off",
            utc(9, 23, 16, 0),
            utc(9, 25, 13, 0),
            &["1001"],
            RunStatus::Cancelled,
        )],
    )
    .await;
    let alice = portal.sign_in(ALICE).await;

    let week = portal.week(&alice, "").await;
    assert_member_week_shape(&week);
    let admin = portal
        .reads
        .read("/api/admin/week", "week.json#/$defs/Week")
        .await;
    assert_matches_admin(&week, &admin);
    assert_eq!(week["starts"], "2026-09-24");
    assert_eq!(week["version"], 2, "the seed and the cancelled run");
    let kalos = run(&week, "r-kalos");
    assert_eq!(kalos["channel"], "#kalos-four");
    assert_eq!(kalos["party"], "#kalos-four");
    assert_eq!(kalos["fixed_id"], "f-kalos");
    assert_eq!(
        kalos["participants"][1],
        json!({"id": "1002", "name": "Bobby", "answer": "no"}),
        "names and answers on every run"
    );
    assert_eq!(flags(&week, "r-kalos"), (true, true), "planned, hers");
    assert_eq!(flags(&week, "r-star"), (true, false), "done");
    assert_eq!(flags(&week, "r-off"), (true, false), "cancelled");

    let next = portal.week(&alice, "?week=next").await;
    assert_member_week_shape(&next);
    let admin_next = portal
        .reads
        .read("/api/admin/week?week=next", "week.json#/$defs/Week")
        .await;
    assert_matches_admin(&next, &admin_next);
    assert_eq!(next["starts"], "2026-10-01");
    assert_eq!(
        flags(&next, "n-kalos"),
        (true, true),
        "next week takes writes"
    );
    assert_eq!(
        flags(&next, "n-star"),
        (false, false),
        "Dan's, sent in full"
    );
    assert_eq!(run(&next, "n-star")["participants"][0]["name"], "Dan");
    assert_eq!(
        portal.week(&alice, "?week=this").await["starts"],
        "2026-09-24"
    );

    // Someone else's view of the same week.
    let dan = portal.sign_in(DAN).await;
    let theirs = portal.week(&dan, "").await;
    assert_eq!(flags(&theirs, "r-kalos"), (true, true));
    assert_eq!(flags(&theirs, "r-star"), (false, false));
    assert_eq!(flags(&theirs, "r-off"), (false, false));

    for query in ["?week=later", "?week=", "?week=NEXT"] {
        let reply = portal.get(Some(&alice), &format!("{WEEK}{query}")).await;
        assert_eq!(
            reply.error(),
            (422, "invalid_query".into()),
            "{query}: as the admin week"
        );
    }
}

/// A run in a channel the bot does not list reads `#unknown` to members,
/// never the raw channel id; the admin week still shows the id.
#[tokio::test]
async fn an_unlisted_channel_reads_unknown_never_its_id() {
    const GONE: &str = "424242424242424242";
    let portal = Portal::new().await;
    let Change::PutRun(mut gone) = put_run(
        "r-gone",
        utc(9, 23, 16, 0),
        utc(9, 25, 13, 0),
        &["1001"],
        RunStatus::Planned,
    ) else {
        unreachable!("put_run puts a run")
    };
    gone.channel_id = Some(GONE.into());
    commit(&portal.reads, vec![Change::PutRun(gone)]).await;
    let alice = portal.sign_in(ALICE).await;

    let week = portal.week(&alice, "").await;
    let view = run(&week, "r-gone");
    assert_eq!(view["channel"], "#unknown");
    assert_eq!(view["party"], "#unknown");
    assert!(!week.to_string().contains(GONE), "{week:#}");
    assert_eq!(run(&week, "r-kalos")["channel"], "#kalos-four");

    let admin = portal
        .reads
        .read("/api/admin/week", "week.json#/$defs/Week")
        .await;
    assert_eq!(run(&admin, "r-gone")["channel"], GONE);
}

/// With the reset at 22:00, Thursday evening belongs to two boss weeks: the
/// member week places runs on the same day and time as the admin week.
#[tokio::test]
async fn boss_week_boundaries_match_the_admin_week() {
    let reads = Reads::with_reset(NaiveTime::from_hms_opt(22, 0, 0).unwrap()).await;
    let start = utc(9, 24, 14, 0);
    commit(
        &reads,
        vec![
            // Thu 24 Sep 22:30 KL, just after the reset.
            put_run(
                "r-first",
                start,
                utc(9, 24, 14, 30),
                &["1001"],
                RunStatus::Planned,
            ),
            // Thu 1 Oct 21:30 KL, just before the next one.
            put_run(
                "r-last",
                start,
                utc(10, 1, 13, 30),
                &["1001"],
                RunStatus::Planned,
            ),
        ],
    )
    .await;
    let portal = Portal::over(reads).await;
    let alice = portal.sign_in(ALICE).await;
    let week = portal.week(&alice, "").await;
    let admin = portal
        .reads
        .read("/api/admin/week", "week.json#/$defs/Week")
        .await;
    assert_matches_admin(&week, &admin);
    assert_eq!(week["reset"], "Thu 22:00");
    assert_eq!(week["starts"], "2026-09-24");
    let at = |id: &str| {
        let run = run(&week, id);
        (run["day"].clone(), run["time"].clone())
    };
    assert_eq!(at("r-first"), (json!(0), json!("22:30")));
    assert_eq!(at("r-last"), (json!(6), json!("21:30")));
}

#[tokio::test]
async fn reads_need_an_open_portal_and_an_eligible_session() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    for path in [WEEK, ALLOWANCE, ART] {
        let reply = portal.get(None, path).await;
        assert_eq!(reply.error(), (401, "unauthenticated".into()), "{path}");
        assert_eq!(reply.header("cache-control"), Some("no-store"));
    }

    portal.harness.set_open(false);
    for browser in [None, Some(&alice)] {
        for path in [WEEK, ALLOWANCE, ART] {
            let reply = portal.get(browser, path).await;
            assert_eq!(reply.error(), (503, "closed".into()), "{path}");
        }
    }
    portal.harness.set_open(true);
    assert_eq!(portal.get(Some(&alice), WEEK).await.status, 200);

    // An admin session means nothing here.
    let admin_cookie = portal.reads.cookie.clone();
    let reply = portal
        .harness
        .get(WEEK, &[("Cookie", &admin_cookie), ("CF-Connecting-IP", IP)])
        .await;
    assert_eq!(reply.error(), (401, "unauthenticated".into()));

    // Losing the bossing role ends the session, and with it the reads.
    portal.harness.roster(ALICE, false).await;
    assert_eq!(
        portal
            .harness
            .member
            .member_changed(&ALICE.to_string(), false)
            .await,
        1
    );
    for path in [WEEK, ALLOWANCE, ART] {
        let reply = portal.get(Some(&alice), path).await;
        assert_eq!(reply.error(), (401, "unauthenticated".into()), "{path}");
    }
}

#[tokio::test]
async fn art_is_served_to_signed_in_members_and_unchanged_on_admin() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let art = portal.get(Some(&alice), ART).await;
    assert_eq!(art.status, 200, "{}", art.text());
    assert_eq!(art.header("content-type"), Some("image/png"));
    assert_eq!(art.body, b"art");
    // Behind the session: never in a shared cache; the browser keeps it a day.
    assert_eq!(art.header("cache-control"), Some("private, max-age=86400"));
    // The public site now holds the live state; the bot's id still stays admin-only.
    let identity = portal.get(None, "/api/identity").await.json();
    crate::schemas::assert_valid("identity.json#/$defs/Identity", "identity", &identity);
    assert_eq!(identity["bot_user_id"], Value::Null);
    let admin_identity = request(portal.reads.admin, "GET", ADMIN_HOST, "/api/identity", &[]).await;
    assert_eq!(admin_identity.json()["bot_user_id"], "42");
    let clip = portal.get(Some(&alice), "/art/animated/MaleficStar").await;
    assert_eq!(clip.status, 200);
    assert_eq!(clip.header("content-type"), Some("video/mp4"));
    for path in ["/art/portraits/Missing", "/art/bogus/MaleficStar"] {
        let reply = portal.get(Some(&alice), path).await;
        assert_eq!(reply.error(), (404, "not_found".into()), "{path}");
    }
    // Other shapes and paths stay unmounted.
    for path in ["/art/portraits/a/b", "/api/public/anything"] {
        let reply = portal.get(Some(&alice), path).await;
        assert_eq!(reply.status, 404, "{path}");
    }

    // The admin listener serves art without a session, open or closed.
    portal.harness.set_open(false);
    for admin in [portal.reads.admin, portal.harness.admin] {
        let reply = request(admin, "GET", ADMIN_HOST, ART, &[]).await;
        assert_eq!(reply.status, 200);
        assert_eq!(reply.body, b"art");
    }
}

#[tokio::test]
async fn data_reads_take_the_members_read_bucket_and_art_does_not() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    for n in 0..120 {
        let reply = portal.get(Some(&alice), ALLOWANCE).await;
        assert_eq!(reply.status, 200, "read {n}: {}", reply.text());
    }
    for path in [WEEK, ALLOWANCE, "/api/public/bosses"] {
        let reply = portal.get(Some(&alice), path).await;
        assert_eq!(reply.error(), (429, "rate_limited".into()), "{path}");
    }
    assert!(
        portal
            .harness
            .audit
            .events()
            .contains(&AuditEvent::RateLimited {
                route: MEMBER_READ_ROUTE
            })
    );
    assert_eq!(portal.get(Some(&alice), ART).await.status, 200, "art");
    assert_eq!(
        portal.get(Some(&alice), "/api/public/session").await.status,
        200,
        "the session read"
    );
    let bob = portal.sign_in(BOB).await;
    assert_eq!(portal.week(&bob, "").await["starts"], "2026-09-24");
    // Two a second.
    portal.harness.advance(TimeDelta::seconds(1));
    assert_eq!(portal.get(Some(&alice), ALLOWANCE).await.status, 200);
}

fn group(name: &str, busy: bool, queue: Vec<QueuedCall>) -> GroupSnapshot {
    let in_use = if busy { 2 } else { 1 };
    GroupSnapshot {
        queue,
        ..snapshot(name, in_use, 2)
    }
}

fn queued(position: u32, kind: CallKind, who: &str) -> QueuedCall {
    QueuedCall {
        position,
        kind,
        who: who.into(),
        waiting_s: 4,
    }
}

async fn set_roles(reads: &Reads, id: u64, roles: &[&str]) {
    let id = id.to_string();
    let mut profile = reads.store.load_member(&id).await.unwrap().unwrap();
    profile.roles = roles.iter().map(|role| (*role).to_owned()).collect();
    reads.store.put_member(profile).await.unwrap();
}

#[tokio::test]
async fn the_allowance_is_the_callers_own() {
    let busy = Arc::new(Mutex::new(false));
    let limits: kanade::api::state::ModelLimits = {
        let busy = busy.clone();
        Arc::new(move |_| {
            vec![
                group(
                    "gateway",
                    *busy.lock().unwrap(),
                    vec![
                        queued(1, CallKind::Chat, "1002"),
                        queued(2, CallKind::Extraction, "1001"),
                        queued(3, CallKind::Chat, "1001"),
                    ],
                ),
                group("local", false, vec![queued(1, CallKind::Chat, "1002")]),
            ]
        })
    };
    let reads = Reads::with_model_limits(limits).await;
    // Alice and Bob hold the chat pilot role; Cara is staff; Dan has neither.
    set_roles(&reads, ALICE, &["30"]).await;
    set_roles(&reads, BOB, &["30"]).await;
    reads.chat.spend_at("1001", -100.0);
    reads.chat.spend("1002");
    reads.chat.spend("1002");
    reads.chat.spend_at("1003", -10.0);
    let portal = Portal::over(reads).await;
    // Raw: this fixture's breaker carries a null `retry_at`, which the Limits
    // schema's own fixtures never send.
    let limits = request(
        portal.reads.admin,
        "GET",
        ADMIN_HOST,
        "/api/admin/limits",
        &[("Cookie", &portal.reads.cookie)],
    )
    .await
    .json();
    let admin_row = items(&limits["allowances"])
        .iter()
        .find(|row| row["member"]["id"] == "1001")
        .unwrap();

    let alice = portal.sign_in(ALICE).await;
    let reply = portal.get(Some(&alice), ALLOWANCE).await;
    let own = portal.allowance(&alice).await;
    assert_eq!(keys(&own), set(ALLOWANCE_KEYS));
    assert_eq!(own["allowance"], admin_row["allowance"]);
    assert!(own["allowance"]["count"].as_u64().unwrap() > 0);
    assert_eq!(own["used"], 1);
    assert_eq!(own["resets_at"], "2026-09-29T04:03:20Z", "-100 + 300");
    assert_eq!(own["resets_at"], admin_row["resets_at"]);
    assert_eq!(
        own["queue_position"], 3,
        "her own chat call, not extraction"
    );
    assert_eq!(own["bot_busy"], false);
    assert_eq!(own["generated_at"], "2026-09-29T04:00:00Z");
    // Nobody else's usage, queue entry or name.
    for other in ["1002", "1003", "Bob", "Cara"] {
        assert!(!reply.text().contains(other), "{other}: {}", reply.text());
    }

    let bob = portal.sign_in(BOB).await;
    let theirs = portal.allowance(&bob).await;
    assert_eq!(
        (theirs["used"].clone(), theirs["queue_position"].clone()),
        (json!(2), json!(1))
    );

    let cara = portal.sign_in(CARA).await;
    assert_eq!(
        portal.allowance(&cara).await,
        json!({
            "allowance": null, "used": 0, "resets_at": null, "queue_position": null,
            "bot_busy": false, "generated_at": "2026-09-29T04:00:00Z",
        }),
        "staff: no limit"
    );

    let dan = portal.sign_in(DAN).await;
    let none = portal.allowance(&dan).await;
    assert_eq!(
        none["allowance"],
        json!({"count": 0, "per_s": own["allowance"]["per_s"]}),
        "no chatbot access: nothing allowed over the default window"
    );
    assert_eq!(
        (none["used"].clone(), none["resets_at"].clone()),
        (json!(0), json!(null))
    );

    *busy.lock().unwrap() = true;
    assert_eq!(portal.allowance(&alice).await["bot_busy"], true);
}
