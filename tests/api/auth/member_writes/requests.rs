//! The member's own requests: submit, list and withdraw, the limits, and an
//! admin's decision in the Inbox reaching the requester's list.

use std::sync::Arc;

use chrono::{NaiveTime, TimeDelta, Weekday};
use kanade::{
    api::write::ApiClock,
    domain::{
        drafts::DraftStore,
        ids::RandomIds,
        members::{MemberStore, Roster},
        requests::{NoFreezes, RequestSpec, Subject},
        schedule::{Change, ReminderPolicy, RunStatus, SchedulePolicy},
        scheduler::SchedulerService,
    },
};
use serde_json::{Value, json};

use super::{ALICE, BOB, CARA, DAN, Portal, keys, refused, run, utc};
use crate::{schemas::assert_valid, support::Reply};

const MINE: &str = "/api/public/requests/mine";
const SUBMIT: &str = "/api/public/requests";
const REQUESTS_KEYS: [&str; 7] = [
    "generated_at",
    "max_open",
    "max_today",
    "open",
    "options",
    "requests",
    "today",
];
const REQUEST_KEYS: [&str; 16] = [
    "bosses",
    "channel",
    "decided_at",
    "decided_by",
    "expires_at",
    "fixed_id",
    "id",
    "kind",
    "note",
    "proposed",
    "reason",
    "run",
    "sent_at",
    "state",
    "summary",
    "with",
];
const PROPOSED_KEYS: [&str; 4] = ["channel", "day", "party", "time"];
const OPTIONS_KEYS: [&str; 2] = ["channels", "members"];
const LIMIT_KEYS: [&str; 3] = ["error", "limit", "message"];

fn withdraw_path(id: &str) -> String {
    format!("/api/public/requests/{id}/withdraw")
}

fn join(run: &str) -> Value {
    json!({"kind": "join", "run_id": run})
}

/// Last boss week (Thursday 17 Sep 00:00 KL): Alice's run there.
fn last_week_run() -> Change {
    Change::PutRun(run(
        "p-old",
        utc(9, 16, 16, 0),
        utc(9, 20, 13, 0),
        &[ALICE],
        RunStatus::Planned,
    ))
}

fn request(reply: &Reply, status: u16) -> Value {
    assert_eq!(reply.status, status, "{}", reply.text());
    let value = reply.json();
    assert_valid("public.json#/$defs/MemberRequest", "request", &value);
    assert_eq!(keys(&value), REQUEST_KEYS.into());
    if !value["proposed"].is_null() {
        assert_eq!(keys(&value["proposed"]), PROPOSED_KEYS.into());
    }
    value
}

fn limited(reply: &Reply) -> String {
    assert_eq!(reply.status, 429, "{}", reply.text());
    let value = reply.json();
    assert_valid("public.json#/$defs/MemberRequestLimit", "limit", &value);
    assert_eq!(keys(&value), LIMIT_KEYS.into());
    assert_eq!(value["error"], "request_limit");
    value["limit"].as_str().unwrap().to_owned()
}

impl Portal {
    async fn mine(&self, browser: &super::Browser) -> Value {
        let value = self
            .read(browser, MINE, "public.json#/$defs/MemberRequests")
            .await;
        assert_eq!(keys(&value), REQUESTS_KEYS.into());
        assert_eq!(keys(&value["options"]), OPTIONS_KEYS.into());
        for item in value["requests"].as_array().unwrap() {
            assert_eq!(keys(item), REQUEST_KEYS.into());
        }
        value
    }

    /// A submission whose body the contract accepts.
    async fn submit(&self, browser: &super::Browser, key: &str, body: &Value) -> Reply {
        assert_valid("public.json#/$defs/MemberRequestBody", "body", body);
        self.write(browser, "POST", SUBMIT, key, Some(body)).await
    }

    async fn withdraw(&self, browser: &super::Browser, id: &str, key: &str) -> Reply {
        self.write(browser, "POST", &withdraw_path(id), key, None)
            .await
    }
}

fn counts(mine: &Value) -> (u64, u64) {
    (
        mine["open"].as_u64().unwrap(),
        mine["today"].as_u64().unwrap(),
    )
}

fn id(value: &Value) -> String {
    value["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn members_send_list_and_withdraw_their_own_requests() {
    let portal = Portal::new().await;
    let (alice, bob) = (portal.sign_in(ALICE).await, portal.sign_in(BOB).await);
    let body = json!({"kind": "join", "run_id": "n-star", "note": "  let me in  "});
    let sent = request(&portal.submit(&alice, "j-1", &body).await, 201);
    assert_eq!(
        sent,
        json!({
            "id": sent["id"].clone(),
            "kind": "join",
            "state": "waiting",
            "summary": "member request: join run:n-star",
            "note": "let me in",
            "run": sent["run"].clone(),
            "fixed_id": null,
            "bosses": sent["bosses"].clone(),
            "channel": "#star",
            "with": null,
            "proposed": null,
            "sent_at": "2026-09-29T04:00:00Z",
            "decided_at": null,
            "decided_by": null,
            "reason": null,
            "expires_at": "2026-10-07T16:00:00Z",
        })
    );
    assert_eq!(
        (sent["run"]["id"].clone(), sent["run"]["mine"].clone()),
        (json!("n-star"), json!(false))
    );
    assert_eq!(sent["bosses"].as_array().unwrap().len(), 1);

    // A retry answers the same request; the key for another body is refused.
    let retry = portal.submit(&alice, "j-1", &body).await;
    assert_eq!(request(&retry, 200), sent);
    let other = json!({"kind": "join", "run_id": "n-star", "note": "now"});
    let reply = portal.submit(&alice, "j-1", &other).await;
    assert_eq!(refused(&reply), (422, "idempotency_mismatch".into()));

    let mine = portal.mine(&alice).await;
    assert_eq!(mine["requests"], json!([sent.clone()]));
    assert_eq!(counts(&mine), (1, 1));
    assert_eq!(
        (mine["max_open"].clone(), mine["max_today"].clone()),
        (json!(3), json!(6))
    );
    assert_eq!(
        mine["options"],
        json!({
            "channels": [{"id": "kalos-four", "name": "#kalos-four"}],
            "members": [
                {"id": "1001", "name": "Alice"},
                {"id": "1002", "name": "Bobby"},
                {"id": "1004", "name": "Dan"},
            ],
        }),
        "watched channels in use; bossing-role members only"
    );
    assert_eq!(portal.mine(&bob).await["requests"], json!([]));

    // Only the requester withdraws; anyone else gets the unknown-id answer.
    let sent_id = id(&sent);
    for (browser, target) in [(&bob, sent_id.as_str()), (&alice, "q-nope")] {
        let reply = portal.withdraw(browser, target, "w-1").await;
        assert_eq!(refused(&reply), (404, "not_found".into()), "{target}");
    }
    let withdrawn = request(&portal.withdraw(&alice, &sent_id, "w-2").await, 200);
    assert_eq!(
        (
            withdrawn["state"].clone(),
            withdrawn["decided_by"].clone(),
            withdrawn["decided_at"].clone()
        ),
        (
            json!("withdrawn"),
            json!("Alice"),
            json!("2026-09-29T04:00:00Z")
        )
    );
    let again = request(&portal.withdraw(&alice, &sent_id, "w-3").await, 200);
    assert_eq!(again, withdrawn, "withdrawing again answers it as it is");
    let reply = portal
        .send(
            Some(&alice),
            "POST",
            &withdraw_path(&sent_id),
            &[
                super::PUB_ORIGIN,
                ("X-Kanade-CSRF", &alice.csrf),
                ("Idempotency-Key", "w-4"),
            ],
            Some(r#"{"reason":"x"}"#),
        )
        .await;
    assert_eq!(refused(&reply), (422, "invalid_body".into()));
    assert_eq!(
        counts(&portal.mine(&alice).await),
        (0, 1),
        "withdrawn still counts today"
    );
}

#[tokio::test]
async fn every_request_kind_and_its_refusals() {
    let portal = Portal::new().await;
    portal.seed(vec![last_week_run()]).await;
    let (alice, bob, cara, dan) = (
        portal.sign_in(ALICE).await,
        portal.sign_in(BOB).await,
        portal.sign_in(CARA).await,
        portal.sign_in(DAN).await,
    );
    let swap = json!({"kind": "swap", "run_id": "n-kalos", "with": "1004"});
    let swapped = request(&portal.submit(&bob, "s-1", &swap).await, 201);
    assert_eq!(swapped["with"], json!({"id": "1004", "name": "Dan"}));
    assert_eq!(swapped["run"]["mine"], true);

    let change =
        json!({"kind": "change_fixed", "fixed_id": "f-kalos", "party": ["1001", "1002", "1004"]});
    let changed = request(&portal.submit(&bob, "c-1", &change).await, 201);
    assert_eq!(
        (
            changed["fixed_id"].clone(),
            changed["run"].clone(),
            changed["channel"].clone(),
            changed["proposed"].clone()
        ),
        (
            json!("f-kalos"),
            Value::Null,
            json!("#kalos-four"),
            json!({"day": null, "time": null, "channel": null, "party": [
                {"id": "1001", "name": "Alice"},
                {"id": "1002", "name": "Bobby"},
                {"id": "1004", "name": "Dan"},
            ]}),
        )
    );

    let new = json!({
        "kind": "new_fixed", "day": 4, "time": "21:00", "channel_id": "kalos-four",
        "bosses": ["XKalos"], "party": ["1001", "1004"], "note": "Friday Kalos?",
    });
    let created = request(&portal.submit(&dan, "n-1", &new).await, 201);
    assert_eq!(
        created["proposed"],
        json!({
            "day": 4, "time": "21:00", "channel": "#kalos-four",
            "party": [{"id": "1004", "name": "Dan"}, {"id": "1001", "name": "Alice"}],
        }),
        "the requester leads the party"
    );
    assert_eq!(
        (
            created["kind"].clone(),
            created["fixed_id"].clone(),
            created["bosses"][0]["token"].clone(),
            created["note"].clone()
        ),
        (
            json!("new_fixed"),
            Value::Null,
            json!("XKalos"),
            json!("Friday Kalos?")
        )
    );

    // The Tuesday timing's dry run cannot apply Alice leaving it (its party
    // keeps Cara, who has no bossing role): 409, nothing stored.
    let stranded = json!({"kind": "leave", "fixed_id": "f-kalos"});
    let reply = portal.submit(&alice, "l-0", &stranded).await;
    assert_eq!(refused(&reply), (409, "no_effect".into()));
    let leave = json!({"kind": "leave", "run_id": "r-kalos"});
    let left = request(&portal.submit(&alice, "l-1", &leave).await, 201);
    assert_eq!(
        (left["kind"].clone(), left["note"].clone()),
        (json!("leave"), Value::Null)
    );

    for (browser, body, expected) in [
        (&alice, join("r-kalos"), (409, "already_in_party")),
        (
            &alice,
            json!({"kind": "leave", "run_id": "n-star"}),
            (404, "not_found"),
        ),
        (&alice, join("r-nope"), (404, "not_found")),
        (&alice, join("p-old"), (404, "not_found")),
        (
            &alice,
            json!({"kind": "join", "fixed_id": "f-nope"}),
            (404, "not_found"),
        ),
        (&cara, join("n-star"), (404, "not_found")),
        (
            &bob,
            json!({"kind": "change_fixed", "fixed_id": "f-kalos", "bosses": ["XKalos"]}),
            (422, "field_not_allowed"),
        ),
        (
            &alice,
            json!({"kind": "join", "run_id": "n-star", "day": 3}),
            (422, "field_not_allowed"),
        ),
        (
            &dan,
            json!({"kind": "new_fixed", "run_id": "n-star", "day": 4, "time": "21:00",
                   "channel_id": "kalos-four", "bosses": ["XKalos"]}),
            (422, "field_not_allowed"),
        ),
        (
            &bob,
            json!({"kind": "swap", "run_id": "n-kalos", "with": "1003"}),
            (422, "invalid_body"),
        ),
        (
            &dan,
            json!({"kind": "new_fixed", "day": 4, "time": "21:00", "channel_id": "star",
                   "bosses": ["XKalos"]}),
            (422, "invalid_body"),
        ),
        (
            &dan,
            json!({"kind": "new_fixed", "day": 4, "time": "21:00", "channel_id": "kalos-four",
                   "bosses": ["Nobody"]}),
            (422, "invalid_body"),
        ),
        (
            &bob,
            json!({"kind": "change_fixed", "fixed_id": "f-kalos"}),
            (422, "invalid_body"),
        ),
        (
            &alice,
            json!({"kind": "join", "run_id": "n-star", "fixed_id": "f-kalos"}),
            (422, "invalid_body"),
        ),
        (&alice, json!({"kind": "join"}), (422, "invalid_body")),
    ] {
        let reply = portal.submit(browser, "x-1", &body).await;
        assert_eq!(refused(&reply), (expected.0, expected.1.into()), "{body}");
    }
    for body in [
        json!({"kind": "move", "run_id": "n-star"}),
        json!({"kind": "join", "run_id": "n-star", "note": "x".repeat(201)}),
        json!({"kind": "join", "run_id": "n-star", "note": "line\nbreak"}),
        json!({"kind": "join", "run_id": "n-star", "title": "x"}),
    ] {
        let reply = portal
            .write(&alice, "POST", SUBMIT, "x-2", Some(&body))
            .await;
        assert_eq!(refused(&reply), (422, "invalid_body".into()), "{body}");
    }
}

#[tokio::test]
async fn requests_are_capped_at_three_waiting_and_six_a_day() {
    let portal = Portal::new().await;
    let bob = portal.sign_in(BOB).await;
    let ask = join("n-star");
    let mut ids = Vec::new();
    for n in 1..=3 {
        ids.push(id(&request(
            &portal.submit(&bob, &format!("l-{n}"), &ask).await,
            201,
        )));
    }
    assert_eq!(limited(&portal.submit(&bob, "l-4", &ask).await), "open");
    assert_eq!(
        counts(&portal.mine(&bob).await),
        (3, 3),
        "a refused one is not counted"
    );

    for n in [0, 1, 2] {
        request(
            &portal.withdraw(&bob, &ids[n], &format!("w-{n}")).await,
            200,
        );
        request(&portal.submit(&bob, &format!("m-{n}"), &ask).await, 201);
    }
    assert_eq!(counts(&portal.mine(&bob).await), (3, 6));
    // The pinned clock gives every request the same `sent_at`: pick a waiting one.
    let mine = portal.mine(&bob).await;
    let waiting = mine["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["state"] == "waiting")
        .map(id)
        .unwrap();
    request(&portal.withdraw(&bob, &waiting, "w-3").await, 200);
    assert_eq!(limited(&portal.submit(&bob, "m-3", &ask).await), "today");
    assert_eq!(counts(&portal.mine(&bob).await), (2, 6));
}

#[tokio::test]
async fn an_admin_approval_reaches_the_requester() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let sent = request(&portal.submit(&alice, "j-1", &join("n-star")).await, 201);
    assert_eq!(sent["note"], Value::Null, "no note: only the summary");
    let sent_id = id(&sent);
    let version = portal
        .reads
        .store
        .load_draft(&sent_id)
        .await
        .unwrap()
        .unwrap()
        .draft
        .version;
    let reply = portal
        .reads
        .call(
            "POST",
            &format!("/api/admin/inbox/{sent_id}/approve"),
            json!({"version": version}),
            &[],
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());

    let mine = portal.mine(&alice).await;
    let approved = &mine["requests"][0];
    assert_eq!(
        (
            approved["state"].clone(),
            approved["decided_by"].clone(),
            approved["decided_at"].clone(),
            approved["run"]["mine"].clone()
        ),
        (
            json!("approved"),
            json!("an admin"),
            json!("2026-09-29T04:00:00Z"),
            json!(true)
        ),
        "Alice is on the run now"
    );
    assert_eq!(counts(&mine), (0, 1));
    let reply = portal.withdraw(&alice, &sent_id, "w-1").await;
    assert_eq!(refused(&reply), (409, "request_closed".into()));
}

#[tokio::test]
async fn a_request_whose_boss_week_passed_shows_expired() {
    let portal = Portal::new().await;
    portal.seed(vec![last_week_run()]).await;
    let store = portal.reads.store.clone();
    let mut directory = Roster::new();
    for profile in store.list_members().await.unwrap() {
        directory.upsert(profile.member);
    }
    let policy = SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::Asia::Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    );
    // Sent last week, before the reset.
    let at = utc(9, 19, 4, 0);
    let mut past = SchedulerService::new(store.clone(), RandomIds, ApiClock(Arc::new(move || at)));
    let old = past
        .submit_request(
            "1004",
            "let me in",
            RequestSpec::Join(Subject::Run("p-old".into())),
            None,
            &policy,
            &directory,
            &NoFreezes,
        )
        .await
        .unwrap();

    let dan = portal.sign_in(DAN).await;
    let mine = portal.mine(&dan).await;
    let expired = &mine["requests"][0];
    assert_eq!(
        (
            expired["id"].clone(),
            expired["state"].clone(),
            expired["expires_at"].clone(),
            expired["run"]["id"].clone(),
            expired["run"]["can_edit"].clone(),
            expired["decided_at"].clone()
        ),
        (
            json!(old.id),
            json!("expired"),
            json!("2026-09-23T16:00:00Z"),
            json!("p-old"),
            json!(false),
            Value::Null
        )
    );
    let reply = portal.withdraw(&dan, &old.id, "w-1").await;
    assert_eq!(refused(&reply), (409, "request_closed".into()));
}

#[tokio::test]
async fn sending_needs_a_fresh_sign_in_and_withdrawing_does_not() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let sent = request(&portal.submit(&alice, "j-1", &join("n-star")).await, 201);
    let body = join("n-star").to_string();
    let csrf = ("X-Kanade-CSRF", alice.csrf.as_str());
    let key = ("Idempotency-Key", "k-1");
    let origin = super::PUB_ORIGIN;
    for path in [SUBMIT.to_owned(), withdraw_path(&id(&sent))] {
        let reply = portal
            .send(Some(&alice), "POST", &path, &[origin, csrf], Some(&body))
            .await;
        assert_eq!(
            refused(&reply),
            (400, "invalid_idempotency_key".into()),
            "{path}"
        );
        let reply = portal
            .send(Some(&alice), "POST", &path, &[origin, key], Some(&body))
            .await;
        assert_eq!(refused(&reply), (403, "csrf".into()), "{path}");
        let reply = portal
            .send(None, "POST", &path, &[origin, key], Some(&body))
            .await;
        assert_eq!(refused(&reply), (401, "unauthenticated".into()), "{path}");
    }
    let reply = portal.get(None, MINE).await;
    assert_eq!(refused(&reply), (401, "unauthenticated".into()));

    portal.harness.advance(TimeDelta::minutes(16));
    let reply = portal.submit(&alice, "j-2", &join("n-star")).await;
    assert_eq!(refused(&reply), (401, "reauth_required".into()));
    let withdrawn = request(&portal.withdraw(&alice, &id(&sent), "w-1").await, 200);
    assert_eq!(withdrawn["state"], "withdrawn");
    let alice = portal.sign_in(ALICE).await;
    request(&portal.submit(&alice, "j-2", &join("n-star")).await, 201);

    portal.harness.set_open(false);
    for (method, path) in [("GET", MINE.to_owned()), ("POST", SUBMIT.to_owned())] {
        let reply = portal
            .write(&alice, method, &path, "c-1", Some(&join("n-star")))
            .await;
        assert_eq!(refused(&reply), (503, "closed".into()), "{method} {path}");
    }
}

#[tokio::test]
async fn a_changed_request_retry_is_a_mismatch_before_the_live_checks() {
    let portal = Portal::new().await;
    let alice = portal.sign_in(ALICE).await;
    let sent = request(&portal.submit(&alice, "j-1", &join("n-star")).await, 201);
    // Not a run members may ask about, but the key's request decides first.
    let reply = portal.submit(&alice, "j-1", &join("r-nope")).await;
    assert_eq!(refused(&reply), (422, "idempotency_mismatch".into()));
    assert_eq!(
        request(&portal.submit(&alice, "j-1", &join("n-star")).await, 200),
        sent
    );
    for note in ["a\u{202E}b", "a\u{200B}b", "a\u{2028}b", "\u{FEFF}a"] {
        let body = json!({"kind": "join", "run_id": "n-star", "note": note});
        let reply = portal
            .write(&alice, "POST", SUBMIT, "x-1", Some(&body))
            .await;
        assert_eq!(refused(&reply), (422, "invalid_body".into()), "{note:?}");
    }
}
