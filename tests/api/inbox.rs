//! A6 inbox against the seeded store of `reads.rs` (pinned clock Tue 29 Sep
//! 12:00 KL): extractor and chat proposals plus every member-request type,
//! listed with previews and decided per source. Every response is validated
//! against the contract schemas.

use std::sync::Arc;

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::{
    api::write::ApiClock,
    domain::{
        drafts::{DraftEventKind, DraftStatus, DraftStore, ProposalSource},
        history::{Actor, ChangeHistory, Origin, Surface},
        ids::RandomIds,
        members::{MemberStore, Roster},
        model_log::{ModelLogStore, WatchedMessage},
        proposals::{CardDetails, CardPayload, ChangeKind, ProposalCardStore, ProposedChange},
        requests::{NoFreezes, RequestSpec, Subject},
        schedule::{
            FixedEdit, NewFixedRun, NewRun, ReminderPolicy, RunSource, RunStatus, SchedulePolicy,
        },
        scheduler::{ProposalRequest, SchedulerService, Supersede},
    },
    infrastructure::store::SqliteStore,
};
use serde_json::{Value, json};

use crate::{
    reads::{EDGE_HEADERS, Reads},
    schemas::assert_valid,
    support::{ADMIN_HOST, Reply, request, send},
};

const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
const PROPOSALS: &str = "inbox.json#/$defs/Proposals";
const MESSAGE: &str = "common.json#/$defs/Message";
const ERROR: &str = "error.json#/$defs/ApiError";
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

fn utc(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
}

fn snowflake(at: DateTime<Utc>) -> String {
    (((at.timestamp_millis() - DISCORD_EPOCH_MS) as u64) << 22).to_string()
}

fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::Asia::Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

type Service = SchedulerService<Arc<SqliteStore>, RandomIds, ApiClock>;

fn service(store: &Arc<SqliteStore>, at: DateTime<Utc>) -> Service {
    SchedulerService::new(store.clone(), RandomIds, ApiClock(Arc::new(move || at)))
}

async fn directory(store: &SqliteStore) -> Roster {
    let mut roster = Roster::new();
    for profile in store.list_members().await.unwrap() {
        roster.upsert(profile.member);
    }
    roster.watch("kalos-four");
    roster
}

/// Ids of everything seeded.
struct Ids {
    expired_request: String,
    expired_chat: String,
    expired_extraction: String,
    moved: String,
    to_edit: String,
    cancel_chat: String,
    to_no_effect: String,
    cancel_nkalos: String,
    join: String,
    leave: String,
    swap: String,
    new_fixed: String,
    change: String,
    unauthorised: String,
}

struct Inbox {
    reads: Reads,
    ids: Ids,
    /// The Discord admin (Cara, 1003: admin role, no bossing role).
    discord: (String, String),
}

async fn propose(
    service: &mut Service,
    change: ProposedChange,
    source: ProposalSource,
    directory: &Roster,
) -> String {
    service
        .propose(
            ProposalRequest {
                change,
                source,
                source_id: "log-1".into(),
                supersede: Supersede::Keep,
            },
            &policy(),
            directory,
        )
        .await
        .unwrap()
        .proposal
        .id
}

fn change(kind: ChangeKind, run: &str, to: Option<DateTime<Utc>>) -> ProposedChange {
    ProposedChange {
        run_id: Some(run.into()),
        channel_id: Some("kalos-four".into()),
        new_datetime: to,
        ..ProposedChange::new(kind)
    }
}

async fn submit(
    service: &mut Service,
    member: &str,
    title: &str,
    spec: RequestSpec,
    directory: &Roster,
) -> String {
    service
        .submit_request(member, title, spec, None, &policy(), directory, &NoFreezes)
        .await
        .unwrap()
        .id
}

async fn seeded() -> Inbox {
    let reads = Reads::with_logins().await;
    let store = reads.store.clone();
    for (id, name) in [("1007", "Finn"), ("1008", "Gus")] {
        let mut profile = kanade::domain::members::MemberProfile::default();
        profile.member.user_id = id.into();
        profile.member.display_name = Some(name.into());
        profile.member.has_role = true;
        store.put_member(profile).await.unwrap();
    }
    let dir = directory(&store).await;

    // Last boss week: a run and a request that has since expired.
    let mut past = service(&store, utc(9, 19, 4, 0));
    let p_run = past
        .as_origin(Origin::new(Actor::admin("seed"), Surface::AdminPortal))
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some("kalos-four".into()),
            week_start: utc(9, 16, 16, 0),
            datetime: utc(9, 20, 13, 0),
            bosses: vec!["NMaleficStar".into()],
            participants: vec!["1001".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        })
        .await
        .unwrap();
    let expired_request = submit(
        &mut past,
        "1007",
        "let me in",
        RequestSpec::Join(Subject::Run(p_run)),
        &dir,
    )
    .await;

    // Yesterday: two proposals now past their 24 h.
    let mut yesterday = service(&store, utc(9, 28, 3, 0));
    let expired_chat = propose(
        &mut yesterday,
        change(ChangeKind::Cancel, "r-kalos", None),
        ProposalSource::Chat,
        &dir,
    )
    .await;
    let expired_extraction = propose(
        &mut yesterday,
        change(ChangeKind::Otot, "r-kalos", None),
        ProposalSource::Extraction,
        &dir,
    )
    .await;

    let mut now = service(&store, utc(9, 29, 4, 0));
    // r-kalos moves off its weekly slot (Tue 22:00 → 21:00), so a timing
    // change must ask about it.
    now.as_origin(Origin::new(Actor::admin("seed"), Surface::AdminPortal))
        .amend_run("r-kalos", utc(9, 29, 13, 0), &policy())
        .await
        .unwrap();
    let moved = propose(
        &mut now,
        change(ChangeKind::Move, "r-kalos", Some(utc(9, 30, 13, 0))),
        ProposalSource::Extraction,
        &dir,
    )
    .await;
    let said = utc(9, 29, 3, 30);
    let gone = utc(9, 29, 3, 31);
    store
        .upsert_message(WatchedMessage {
            id: snowflake(said),
            channel_id: "kalos-four".into(),
            author_id: "1001".into(),
            created_at: said,
            edited_at: None,
            content: "wed 9pm instead?".into(),
            processed_at: None,
        })
        .await
        .unwrap();
    store
        .save_card(
            &moved,
            "kalos-four",
            &CardDetails {
                kind: ChangeKind::Move,
                run_id: Some("r-kalos".into()),
                bosses: vec!["XKalos".into()],
                participants: vec!["1001".into()],
                new_datetime: Some(utc(9, 30, 13, 0)),
                day_ref: Some("wed".into()),
                time_ref: Some("9pm".into()),
                rsvp: None,
                is_question: true,
                summary: Some("Alice asks for Wednesday".into()),
                also_mentioned: Vec::new(),
                confidence: 0.9,
                payload: CardPayload::default(),
                evidence_message_ids: vec![snowflake(said), snowflake(gone)],
                self_service: None,
            },
            utc(9, 29, 4, 0),
        )
        .await
        .unwrap();
    let to_edit = propose(
        &mut now,
        change(ChangeKind::Move, "r-kalos", Some(utc(9, 30, 12, 30))),
        ProposalSource::Extraction,
        &dir,
    )
    .await;
    let cancel_chat = propose(
        &mut now,
        change(ChangeKind::Cancel, "n-star", None),
        ProposalSource::Chat,
        &dir,
    )
    .await;
    let to_no_effect = propose(
        &mut now,
        change(ChangeKind::Move, "n-kalos", Some(utc(10, 7, 13, 0))),
        ProposalSource::Extraction,
        &dir,
    )
    .await;

    let cancel_nkalos = propose(
        &mut now,
        change(ChangeKind::Cancel, "n-kalos", None),
        ProposalSource::Extraction,
        &dir,
    )
    .await;
    let join = submit(
        &mut now,
        "1007",
        "can I come?",
        RequestSpec::Join(Subject::Run("r-kalos".into())),
        &dir,
    )
    .await;
    let leave = submit(
        &mut now,
        "1002",
        "busy next week",
        RequestSpec::Leave(Subject::Run("n-kalos".into())),
        &dir,
    )
    .await;
    let swap = submit(
        &mut now,
        "1001",
        "Gus takes my spot",
        RequestSpec::Swap {
            subject: Subject::Run("r-kalos".into()),
            with: "1008".into(),
        },
        &dir,
    )
    .await;
    let new_fixed = submit(
        &mut now,
        "1004",
        "weekly star",
        RequestSpec::NewFixed(NewFixedRun {
            owner_id: "1004".into(),
            channel_id: Some("kalos-four".into()),
            bosses: vec!["NMaleficStar".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(20, 0, 0).unwrap(),
            participants: vec!["1004".into()],
            note: None,
        }),
        &dir,
    )
    .await;
    let change_fixed = submit(
        &mut now,
        "1001",
        "Wednesdays suit us",
        RequestSpec::ChangeFixed {
            fixed_id: "f-kalos".into(),
            edit: FixedEdit {
                weekday: Some(Weekday::Wed),
                time: Some(NaiveTime::from_hms_opt(21, 0, 0).unwrap()),
                ..FixedEdit::default()
            },
        },
        &dir,
    )
    .await;
    let unauthorised = submit(
        &mut now,
        "1008",
        "next week too",
        RequestSpec::Join(Subject::Run("n-kalos".into())),
        &dir,
    )
    .await;
    // Gus loses the bossing role after asking.
    let mut gus = store.load_member("1008").await.unwrap().unwrap();
    gus.member.has_role = false;
    store.put_member(gus).await.unwrap();

    let discord = reads.discord_session(1003, "Cara").await;
    Inbox {
        reads,
        ids: Ids {
            expired_request,
            expired_chat,
            expired_extraction,
            moved,
            to_edit,
            cancel_chat,
            to_no_effect,
            cancel_nkalos,
            join,
            leave,
            swap,
            new_fixed,
            change: change_fixed,
            unauthorised,
        },
        discord,
    }
}

impl Inbox {
    /// A write as the given session (`cookie`, `csrf`) plus `extra` headers.
    async fn as_session(
        &self,
        session: &(String, String),
        path: &str,
        body: Value,
        extra: &[(&str, &str)],
    ) -> Reply {
        let mut headers = vec![
            ("Cookie", session.0.as_str()),
            ORIGIN,
            ("X-Kanade-CSRF", session.1.as_str()),
        ];
        headers.extend_from_slice(extra);
        let method = if path.ends_with("/participants") {
            "PATCH"
        } else {
            "POST"
        };
        send(
            self.reads.admin,
            method,
            ADMIN_HOST,
            path,
            &headers,
            Some(&body.to_string()),
        )
        .await
    }

    async fn discord(&self, path: &str, body: Value, extra: &[(&str, &str)]) -> Reply {
        self.as_session(&self.discord, path, body, extra).await
    }

    fn token(&self) -> (String, String) {
        (self.reads.cookie.clone(), self.reads.csrf.clone())
    }

    async fn token_call(&self, path: &str, body: Value, extra: &[(&str, &str)]) -> Reply {
        self.as_session(&self.token(), path, body, extra).await
    }

    async fn list(&self) -> Vec<Value> {
        let reply = request(
            self.reads.admin,
            "GET",
            ADMIN_HOST,
            "/api/admin/inbox",
            &[("Cookie", self.discord.0.as_str())],
        )
        .await;
        assert_eq!(reply.status, 200, "{}", reply.text());
        let value = reply.json();
        assert_valid(PROPOSALS, "/api/admin/inbox", &value);
        value.as_array().unwrap().clone()
    }

    async fn item(&self, id: &str) -> Value {
        self.list()
            .await
            .into_iter()
            .find(|item| item["id"] == id)
            .unwrap_or_else(|| panic!("{id} listed"))
    }

    async fn head(&self) -> u64 {
        self.reads.store.history_head().await.unwrap().seq
    }
}

fn path(id: &str, action: &str) -> String {
    format!("/api/admin/inbox/{id}/{action}")
}

/// 2xx validated as a `Message`.
fn ok(reply: &Reply) -> String {
    assert!((200..300).contains(&reply.status), "{}", reply.text());
    let value = reply.json();
    assert_valid(MESSAGE, "decision", &value);
    value["message"].as_str().unwrap().to_owned()
}

/// A refusal validated as an `ApiError`: `(status, code)`.
fn refused(reply: &Reply) -> (u16, String) {
    assert!(reply.status >= 400, "{}", reply.text());
    assert_valid(ERROR, "decision", &reply.json());
    (reply.status, reply.api_error())
}

fn code(status: u16, code: &str) -> (u16, String) {
    (status, code.to_owned())
}

#[tokio::test]
async fn the_inbox_lists_both_proposal_sources_and_every_request_type() {
    let inbox = seeded().await;
    let ids = &inbox.ids;
    let items = inbox.list().await;
    let listed: Vec<&str> = items
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        items.len(),
        14,
        "{listed:?} {:?}",
        [
            &ids.expired_request,
            &ids.expired_chat,
            &ids.expired_extraction,
            &ids.moved,
            &ids.to_edit,
            &ids.cancel_chat,
            &ids.to_no_effect,
            &ids.cancel_nkalos,
            &ids.join,
            &ids.leave,
            &ids.swap,
            &ids.new_fixed,
            &ids.change,
            &ids.unauthorised
        ]
    );
    let by = |id: &str| items.iter().find(|item| item["id"] == id).unwrap().clone();
    // Oldest first: last week's request, then yesterday's proposals.
    assert_eq!(items[0]["id"], ids.expired_request.as_str());

    let moved = by(&ids.moved);
    assert_eq!(
        (
            &moved["source"],
            &moved["tab"],
            &moved["kind"],
            &moved["kind_label"]
        ),
        (
            &json!("extraction"),
            &json!("extractor"),
            &json!("move"),
            &json!("Move")
        )
    );
    assert_eq!(moved["when"], "Wed 30 Sep 21:00");
    assert_eq!(moved["from_when"], "Tue 29 Sep 21:00");
    assert_eq!(moved["confidence"], 0.9);
    assert_eq!(moved["is_question"], true);
    assert_eq!(moved["summary"], "Alice asks for Wednesday");
    assert_eq!(moved["channel"], "#kalos-four");
    assert_eq!(moved["card_url"], Value::Null, "the card was never posted");
    assert_eq!(moved["flags"], json!([]));
    assert_eq!(moved["expires_at"], "Wed 30 Sep 12:00");
    let evidence = moved["evidence"].as_array().unwrap();
    assert_eq!(evidence[0]["author"], "Alice");
    assert_eq!(evidence[0]["author_id"], "1001");
    assert_eq!(evidence[0]["content"], "wed 9pm instead?");
    assert_eq!(evidence[0]["missing"], false);
    assert!(
        evidence[0]["url"]
            .as_str()
            .unwrap()
            .starts_with("https://discord.com/channels/900/kalos-four/")
    );
    assert_eq!(evidence[1]["missing"], true);
    assert_eq!(evidence[1]["content"], Value::Null);
    assert_eq!(evidence[1]["author_id"], Value::Null);
    assert_eq!(
        moved["preview"],
        json!({"no_effect": false, "changes": [
            {"field": "slot", "from": "Tue 29 Sep 21:00", "to": "Wed 30 Sep 21:00"}
        ], "conflicts": []})
    );

    // A chat proposal whose card was never saved.
    let chat = by(&ids.cancel_chat);
    assert_eq!(
        (&chat["source"], &chat["kind"]),
        (&json!("chat"), &json!("cancel"))
    );
    assert_eq!(chat["confidence"], Value::Null);
    assert_eq!(chat["evidence"], json!([]));
    assert_eq!(chat["summary"], "cancel proposal");
    assert_eq!(chat["bosses"][0]["token"], "NMaleficStar");
    assert_eq!(chat["when"], "Fri 02 Oct 21:00");
    assert_eq!(chat["preview"]["changes"][0]["field"], "status");

    for (id, kind, label) in [
        (&ids.join, "join", "Join"),
        (&ids.leave, "leave", "Leave"),
        (&ids.new_fixed, "new_fixed", "New weekly run"),
        (&ids.change, "change_fixed", "Weekly timing change"),
    ] {
        let item = by(id);
        assert_eq!(
            (
                &item["kind"],
                &item["kind_label"],
                &item["tab"],
                &item["source"]
            ),
            (
                &json!(kind),
                &json!(label),
                &json!("self_service"),
                &json!("self_service")
            ),
            "{kind}"
        );
        assert_eq!(item["self_service"]["via"], "request", "{kind}");
        assert_eq!(item["confidence"], Value::Null, "{kind}");
        assert_eq!(item["flags"], json!([]), "{kind}: {item:#}");
    }
    let join = by(&ids.join);
    assert_eq!(join["public_summary"], "member request: join run:r-kalos");
    assert_eq!(
        join["self_service"]["member"],
        json!({"id": "1007", "name": "Finn"})
    );
    assert_eq!(join["self_service"]["note"], "can I come?");
    assert_eq!(
        join["participants"],
        json!([{"id": "1007", "name": "Finn"}])
    );
    assert_eq!(join["when"], "Tue 29 Sep 21:00");
    assert_eq!(join["expires_at"], "Thu 01 Oct 00:00");
    assert_eq!(
        join["preview"]["changes"],
        json!([
            {"field": "participants", "from": "Alice, Bobby, Dan", "to": "Alice, Bobby, Dan, Finn"},
            // Bob said no; a newcomer without an answer re-derives the status.
            {"field": "status", "from": "planned", "to": "at_risk"},
        ])
    );
    // Gus lost the bossing role, so the swap no longer applies as asked.
    let swap = by(&ids.swap);
    assert_eq!(
        (&swap["kind"], &swap["kind_label"]),
        (&json!("swap"), &json!("Swap"))
    );
    assert_eq!(swap["participants"].as_array().unwrap().len(), 2);
    assert_eq!(swap["flags"], json!(["conflict"]));
    assert_eq!(swap["preview"]["conflicts"][0]["field"], "change");
    let new_fixed = by(&ids.new_fixed);
    assert_eq!(new_fixed["when"], "Sat 20:00");
    assert_eq!(
        new_fixed["expires_at"],
        Value::Null,
        "weekly-only requests never expire"
    );
    assert_eq!(new_fixed["preview"]["changes"][0]["field"], "new_fixed");
    let change = by(&ids.change);
    assert_eq!(
        (&change["from_when"], &change["when"]),
        (&json!("Tue 22:00"), &json!("Wed 21:00"))
    );
    assert_eq!(
        change["choices"],
        json!([{"run_id": "r-kalos", "label": "This week", "when": "Tue 29 Sep 21:00", "amended": true}])
    );
    assert_eq!(by(&ids.leave)["choices"], Value::Null);

    assert_eq!(
        by(&ids.unauthorised)["flags"],
        json!(["conflict", "requester_unauthorised"])
    );
    assert_eq!(by(&ids.expired_request)["flags"], json!(["expired"]));
    for id in [&ids.expired_chat, &ids.expired_extraction] {
        assert_eq!(by(id)["flags"], json!(["expired"]));
    }

    // The token session lists the same items (no Discord id for previews).
    let reply = request(
        inbox.reads.admin,
        "GET",
        ADMIN_HOST,
        "/api/admin/inbox",
        &[("Cookie", inbox.reads.cookie.as_str())],
    )
    .await;
    assert_eq!(reply.json().as_array().unwrap().len(), 14);
}

#[tokio::test]
async fn member_requests_are_decided_by_any_admin_session_with_every_refusal() {
    let inbox = seeded().await;
    let ids = &inbox.ids;
    let head = inbox.head().await;
    let approve = |id: &str| path(id, "approve");
    let reject = |id: &str| path(id, "reject");
    let join = inbox.item(&ids.join).await;
    let v = join["version"].as_u64().unwrap();
    for (body, expected) in [
        (json!({}), code(422, "version_required")),
        (json!({"version": v + 1}), code(409, "stale")),
        (
            json!({"version": v, "choices": {"r-kalos": "keep"}}),
            code(422, "choices_not_applicable"),
        ),
        (
            json!({"version": v, "day": 6, "time": "20:00"}),
            code(422, "edit_not_applicable"),
        ),
        (
            json!({"version": v, "force": true}),
            code(422, "force_unsupported"),
        ),
        (json!({"version": v, "extra": 1}), code(400, "invalid_body")),
    ] {
        let reply = inbox
            .token_call(&approve(&ids.join), body.clone(), &[])
            .await;
        assert_eq!(refused(&reply), expected, "{body}");
    }
    assert_eq!(
        refused(
            &inbox
                .token_call(&approve("nope"), json!({"version": 1}), &[])
                .await
        ),
        code(404, "not_found")
    );
    assert_eq!(
        refused(
            &inbox
                .token_call(
                    &approve(&ids.join),
                    json!({"version": v}),
                    &[("Idempotency-Key", "bad key")]
                )
                .await
        ),
        code(400, "invalid_idempotency_key")
    );
    // CSRF is required.
    let no_csrf = send(
        inbox.reads.admin,
        "POST",
        ADMIN_HOST,
        &approve(&ids.join),
        &[("Cookie", inbox.reads.cookie.as_str()), ORIGIN],
        Some(&json!({"version": v}).to_string()),
    )
    .await;
    assert_eq!((no_csrf.status, no_csrf.api_error()), (403, "csrf".into()));
    assert_eq!(inbox.head().await, head, "no refusal wrote anything");

    // A weekly-timing change needs a choice for its amended run.
    let change = inbox.item(&ids.change).await;
    let cv = change["version"].as_u64().unwrap();
    for (body, expected) in [
        (json!({"version": cv}), code(422, "choices_required")),
        (
            json!({"version": cv, "choices": {}}),
            code(422, "choices_required"),
        ),
        (
            json!({"version": cv, "choices": {"r-kalos": "maybe"}}),
            code(422, "invalid"),
        ),
        (
            json!({"version": cv, "choices": {"r-kalos": "keep", "n-kalos": "keep"}}),
            code(422, "choices_not_applicable"),
        ),
    ] {
        let reply = inbox
            .token_call(&approve(&ids.change), body.clone(), &[])
            .await;
        assert_eq!(refused(&reply), expected, "{body}");
    }
    // The token session approves; a retry with the same key answers the same.
    let key = [("Idempotency-Key", "approve-change-1")];
    let body = json!({"version": cv, "choices": {"r-kalos": "keep"}, "force": false});
    let first = ok(&inbox
        .token_call(&approve(&ids.change), body.clone(), &key)
        .await);
    let record = inbox
        .reads
        .store
        .load_change(head + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.origin.actor, Actor::admin("token"));
    assert_eq!(record.origin.surface, Surface::RequestMerge);
    assert_eq!(
        ok(&inbox.token_call(&approve(&ids.change), body, &key).await),
        first
    );
    assert_eq!(
        refused(
            &inbox
                .token_call(
                    &approve(&ids.change),
                    json!({"version": cv, "choices": {"r-kalos": "update"}}),
                    &key
                )
                .await
        ),
        code(422, "idempotency_mismatch")
    );
    assert_eq!(inbox.head().await, head + 1);
    // Another admin finds it decided.
    assert_eq!(
        refused(
            &inbox
                .discord(
                    &approve(&ids.change),
                    json!({"version": cv, "choices": {"r-kalos": "keep"}}),
                    &[]
                )
                .await
        ),
        code(409, "stale")
    );

    // Requester no longer authorised, twice over: lost the role; left the run.
    let unauthorised = inbox.item(&ids.unauthorised).await;
    assert_eq!(
        refused(
            &inbox
                .token_call(
                    &approve(&ids.unauthorised),
                    json!({"version": unauthorised["version"]}),
                    &[]
                )
                .await
        ),
        code(409, "requester_unauthorised")
    );
    let week = inbox.reads.version().await;
    let reply = inbox
        .token_call(
            "/api/admin/runs/n-kalos/participants",
            json!({"remove": "1002", "version": week}),
            &[],
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let leave = inbox.item(&ids.leave).await;
    assert!(
        leave["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("requester_unauthorised"))
    );
    assert_eq!(
        refused(
            &inbox
                .token_call(
                    &approve(&ids.leave),
                    json!({"version": leave["version"]}),
                    &[]
                )
                .await
        ),
        code(409, "requester_unauthorised")
    );

    let expired = inbox.item(&ids.expired_request).await;
    assert_eq!(
        refused(
            &inbox
                .token_call(
                    &approve(&ids.expired_request),
                    json!({"version": expired["version"]}),
                    &[]
                )
                .await
        ),
        code(410, "expired")
    );
    let loaded = inbox
        .reads
        .store
        .load_draft(&ids.expired_request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.draft.status, DraftStatus::Expired);

    // Join: Finn is added directly first, so approving changes nothing.
    let week = inbox.reads.version().await;
    let reply = inbox
        .token_call(
            "/api/admin/runs/r-kalos/participants",
            json!({"add": "1007", "version": week}),
            &[],
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(inbox.item(&ids.join).await["flags"], json!(["no_effect"]));
    assert_eq!(
        refused(
            &inbox
                .token_call(&approve(&ids.join), json!({"version": v}), &[])
                .await
        ),
        code(409, "no_effect")
    );

    // Reject (the Discord admin): a reason is required; retries answer the same.
    let swap = inbox.item(&ids.swap).await;
    let sv = swap["version"].as_u64().unwrap();
    for (body, expected) in [
        (json!({"version": sv}), code(422, "reason_required")),
        (
            json!({"version": sv, "reason": "  "}),
            code(422, "reason_required"),
        ),
        (
            json!({"version": sv, "reason": "x".repeat(501)}),
            code(422, "reason_invalid"),
        ),
        (json!({"reason": "no"}), code(422, "version_required")),
        (
            json!({"version": sv + 1, "reason": "no"}),
            code(409, "stale"),
        ),
    ] {
        let reply = inbox.discord(&reject(&ids.swap), body.clone(), &[]).await;
        assert_eq!(refused(&reply), expected, "{body}");
    }
    let key = [("Idempotency-Key", "reject-swap-1")];
    let body = json!({"version": sv, "reason": "We need you this week."});
    let first = ok(&inbox.discord(&reject(&ids.swap), body.clone(), &key).await);
    assert_eq!(
        ok(&inbox.discord(&reject(&ids.swap), body, &key).await),
        first
    );
    assert_eq!(
        refused(
            &inbox
                .discord(
                    &reject(&ids.swap),
                    json!({"version": sv, "reason": "Another reason."}),
                    &key
                )
                .await
        ),
        code(422, "idempotency_mismatch")
    );
    assert_eq!(
        refused(
            &inbox
                .token_call(
                    &reject(&ids.swap),
                    json!({"version": sv, "reason": "Me too."}),
                    &[]
                )
                .await
        ),
        code(409, "stale")
    );
    let loaded = inbox
        .reads
        .store
        .load_draft(&ids.swap)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            loaded.draft.status,
            loaded.draft.closed_by,
            loaded.draft.close_reason.as_deref()
        ),
        (
            DraftStatus::Rejected,
            Some(Actor::admin("discord:1003")),
            Some("We need you this week.")
        )
    );
    let events = inbox.reads.store.draft_events(&ids.swap).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == DraftEventKind::Rejected)
            .count(),
        1
    );

    // Tailscale sessions decide member requests too.
    let tailscale = inbox.reads.tailscale_session().await;
    let nf = inbox.item(&ids.new_fixed).await;
    ok(&inbox
        .as_session(
            &tailscale,
            &reject(&ids.new_fixed),
            json!({"version": nf["version"], "reason": "One weekly is enough."}),
            &EDGE_HEADERS,
        )
        .await);
}

#[tokio::test]
async fn proposals_need_a_discord_session_and_answer_as_that_member() {
    let inbox = seeded().await;
    let ids = &inbox.ids;
    let head = inbox.head().await;
    let approve = path(&ids.moved, "approve");
    let reject = path(&ids.cancel_chat, "reject");

    // Token and Tailscale sessions: refused before anything else.
    for target in [&approve, &reject] {
        assert_eq!(
            refused(&inbox.token_call(target, json!({}), &[]).await),
            code(403, "discord_session_required")
        );
    }
    let tailscale = inbox.reads.tailscale_session().await;
    for target in [&approve, &reject] {
        let reply = inbox
            .as_session(&tailscale, target, json!({}), &EDGE_HEADERS)
            .await;
        assert_eq!(refused(&reply), code(403, "discord_session_required"));
    }

    let moved = inbox.item(&ids.moved).await;
    let v = moved["version"].as_u64().unwrap();
    for (body, expected) in [
        (json!({"version": v + 1}), code(409, "stale")),
        (
            json!({"choices": {"r-kalos": "update"}}),
            code(422, "choices_not_applicable"),
        ),
        (json!({"force": true}), code(422, "force_unsupported")),
    ] {
        assert_eq!(
            refused(&inbox.discord(&approve, body.clone(), &[]).await),
            expected,
            "{body}"
        );
    }
    assert_eq!(inbox.head().await, head);

    // Past the TTL: approving is refused and closes it; rejecting closes it.
    assert_eq!(
        refused(
            &inbox
                .discord(&path(&ids.expired_chat, "approve"), json!({}), &[])
                .await
        ),
        code(410, "expired")
    );
    ok(&inbox
        .discord(&path(&ids.expired_extraction, "reject"), json!({}), &[])
        .await);
    for id in [&ids.expired_chat, &ids.expired_extraction] {
        let loaded = inbox.reads.store.load_draft(id).await.unwrap().unwrap();
        assert_eq!(loaded.draft.status, DraftStatus::Expired);
    }
    assert!(inbox.reads.proposal_refreshes.lock().unwrap().is_empty());

    let key = [("Idempotency-Key", "approve-moved-1")];
    let first = ok(&inbox.discord(&approve, json!({"version": v}), &key).await);
    let record = inbox
        .reads
        .store
        .load_change(head + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.origin.actor, Actor::member("1003"));
    assert_eq!(record.origin.surface, Surface::ExtractionApproval);
    assert_eq!(
        record.origin.request_id.as_deref(),
        Some(format!("approve:{}", ids.moved).as_str())
    );
    let refreshes = inbox.reads.proposal_refreshes.lock().unwrap().clone();
    assert_eq!(refreshes.len(), 1, "only the committed approval refreshes");
    assert!(refreshes[0].contains(&ids.moved));
    assert!(refreshes[0].contains(&ids.to_edit));
    assert_eq!(
        ok(&inbox.discord(&approve, json!({"version": v}), &key).await),
        first
    );
    assert_eq!(inbox.head().await, head + 1);
    assert_eq!(inbox.reads.proposal_refreshes.lock().unwrap().len(), 1);
    // The approval retired the other live proposal about r-kalos.
    assert_eq!(
        refused(
            &inbox
                .discord(&path(&ids.to_edit, "approve"), json!({}), &[])
                .await
        ),
        code(409, "stale")
    );

    // Reject: no reason; the chat proposal closes as the Discord member.
    assert_eq!(
        refused(&inbox.discord(&reject, json!({"reason": "nope"}), &[]).await),
        code(422, "reason_not_applicable")
    );
    let key = [("Idempotency-Key", "reject-chat-1")];
    let first = ok(&inbox.discord(&reject, json!({"reason": ""}), &key).await);
    assert_eq!(
        inbox.reads.proposal_refreshes.lock().unwrap().as_slice(),
        [refreshes[0].clone(), vec![ids.cancel_chat.clone()]],
        "only committed rejection adds one refresh"
    );
    assert_eq!(ok(&inbox.discord(&reject, json!({}), &key).await), first);
    assert_eq!(inbox.reads.proposal_refreshes.lock().unwrap().len(), 2);
    let loaded = inbox
        .reads
        .store
        .load_draft(&ids.cancel_chat)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.draft.status, DraftStatus::Rejected);
    assert_eq!(loaded.draft.closed_by, Some(Actor::member("1003")));
    assert_eq!(
        refused(
            &inbox
                .discord(&path(&ids.cancel_chat, "approve"), json!({}), &[])
                .await
        ),
        code(409, "stale")
    );

    // n-kalos moved elsewhere upstream: the proposed move conflicts...
    let week = inbox.reads.version().await;
    let reply = inbox
        .token_call(
            "/api/admin/runs/n-kalos/move",
            json!({"day": 5, "time": "23:00", "version": week}),
            &[],
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    assert_eq!(
        inbox.item(&ids.to_no_effect).await["flags"],
        json!(["conflict"])
    );
    let target = path(&ids.to_no_effect, "approve");
    assert_eq!(
        refused(&inbox.discord(&target, json!({}), &[]).await),
        code(409, "conflicts")
    );
    // Cancelled directly: the proposed cancel would change nothing.
    let reply = send(
        inbox.reads.admin,
        "PATCH",
        ADMIN_HOST,
        "/api/admin/runs/n-kalos/status",
        &[
            ("Cookie", inbox.reads.cookie.as_str()),
            ORIGIN,
            ("X-Kanade-CSRF", inbox.reads.csrf.as_str()),
        ],
        Some(&json!({"status": "cancelled", "version": week + 1}).to_string()),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let item = inbox.item(&ids.cancel_nkalos).await;
    assert_eq!(item["flags"], json!(["no_effect"]), "{:#}", item["preview"]);
    assert_eq!(
        refused(
            &inbox
                .discord(&path(&ids.cancel_nkalos, "approve"), json!({}), &[])
                .await
        ),
        code(409, "no_effect")
    );
    assert_eq!(
        refused(&inbox.discord(&path("nope", "reject"), json!({}), &[]).await),
        code(404, "not_found")
    );
}

#[tokio::test]
async fn an_edited_approval_moves_to_the_admins_time_in_one_record() {
    let inbox = seeded().await;
    let ids = &inbox.ids;
    let head = inbox.head().await;
    let approve = path(&ids.to_edit, "approve");
    for (body, expected) in [
        (json!({"day": 7, "time": "20:00"}), code(422, "invalid")),
        (json!({"day": 6, "time": "8pm"}), code(422, "invalid")),
        (json!({"day": 6}), code(422, "invalid")),
        (json!({"time": "20:00"}), code(422, "invalid")),
    ] {
        assert_eq!(
            refused(&inbox.discord(&approve, body.clone(), &[]).await),
            expected,
            "{body}"
        );
    }
    assert_eq!(
        refused(
            &inbox
                .discord(
                    &path(&ids.cancel_chat, "approve"),
                    json!({"day": 6, "time": "20:00"}),
                    &[]
                )
                .await
        ),
        code(422, "edit_not_applicable")
    );

    // Wednesday (day 6 of the boss week starting Thu 24 Sep) at 20:00 KL.
    let key = [("Idempotency-Key", "edit-1")];
    let body = json!({"day": 6, "time": "20:00"});
    let first = ok(&inbox.discord(&approve, body.clone(), &key).await);
    assert_eq!(inbox.head().await, head + 1, "one record");
    let snapshot = kanade::domain::scheduler::ScheduleStore::load(
        inbox.reads.store.as_ref(),
        &kanade::domain::scheduler::Scope::All,
    )
    .await
    .unwrap();
    let run = snapshot
        .runs
        .iter()
        .find(|run| run.id == "r-kalos")
        .unwrap();
    assert_eq!(run.datetime, utc(9, 30, 12, 0));
    let record = inbox
        .reads
        .store
        .load_change(head + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (record.origin.actor.clone(), record.origin.surface),
        (Actor::member("1003"), Surface::ExtractionApproval)
    );
    let merged = inbox
        .reads
        .store
        .draft_events(&ids.to_edit)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.kind == DraftEventKind::Merged)
        .unwrap();
    assert_eq!(
        merged.detail.as_deref(),
        Some(format!("{} edited=2026-09-30T12:00:00+00:00", head + 1).as_str())
    );

    // The same edit again answers the first result; another edit is refused.
    assert_eq!(ok(&inbox.discord(&approve, body, &key).await), first);
    assert_eq!(
        refused(
            &inbox
                .discord(&approve, json!({"day": 6, "time": "19:00"}), &key)
                .await
        ),
        code(422, "idempotency_mismatch")
    );
    assert_eq!(inbox.head().await, head + 1);
}

#[tokio::test]
async fn an_edited_approval_still_blocks_on_conflicts() {
    let inbox = seeded().await;
    let week = inbox.reads.version().await;
    let reply = inbox
        .token_call(
            "/api/admin/runs/r-kalos/move",
            json!({"day": 5, "time": "23:00", "version": week}),
            &[],
        )
        .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let head = inbox.head().await;
    assert_eq!(
        refused(
            &inbox
                .discord(
                    &path(&inbox.ids.moved, "approve"),
                    json!({"day": 6, "time": "20:00"}),
                    &[]
                )
                .await
        ),
        code(409, "conflicts")
    );
    assert_eq!(inbox.head().await, head);
    // `moved` stays in the inbox, flagged.
    assert_eq!(
        inbox.item(&inbox.ids.moved).await["flags"],
        json!(["conflict"])
    );
}

fn decided(notice: &kanade::domain::schedule::Notice) -> Option<(String, &'static str)> {
    match &notice.change {
        kanade::domain::schedule::NoticeChange::RequestDecided {
            request, decision, ..
        } => Some((request.clone(), decision.as_str())),
        _ => None,
    }
}

async fn draft_notices(store: &SqliteStore, id: &str) -> Vec<kanade::domain::schedule::Notice> {
    use kanade::domain::notify::{NoticeOutbox, draft_source};
    let source = draft_source(id);
    store
        .outbox_notices()
        .await
        .unwrap()
        .into_iter()
        .filter(|row| row.source == source)
        .map(|row| row.notice)
        .collect()
}

#[tokio::test]
async fn request_decisions_enqueue_merge_and_requester_notices_with_the_decision() {
    use kanade::domain::notify::NoticeOutbox;
    let inbox = seeded().await;
    let ids = &inbox.ids;
    let store = &inbox.reads.store;
    let head = inbox.head().await;
    let v = inbox.item(&ids.join).await["version"].as_u64().unwrap();
    let key = [("Idempotency-Key", "outbox-join")];
    ok(&inbox
        .token_call(&path(&ids.join, "approve"), json!({"version": v}), &key)
        .await);
    let merged = crate::outbox::written_by(store, head + 1).await;
    let (last, summaries) = merged.split_last().expect("notices");
    assert_eq!(decided(last), Some((ids.join.clone(), "approved")));
    assert!(!summaries.is_empty() && summaries.iter().all(|n| decided(n).is_none()));
    let total = store.outbox_notices().await.unwrap().len();
    ok(&inbox
        .token_call(&path(&ids.join, "approve"), json!({"version": v}), &key)
        .await);
    assert_eq!(
        store.outbox_notices().await.unwrap().len(),
        total,
        "a replayed approval enqueues nothing"
    );

    let lv = inbox.item(&ids.leave).await["version"].as_u64().unwrap();
    ok(&inbox
        .token_call(
            &path(&ids.leave, "reject"),
            json!({"version": lv, "reason": "not this week"}),
            &[],
        )
        .await);
    let rejected = draft_notices(store, &ids.leave).await;
    assert_eq!(
        rejected.iter().map(decided).collect::<Vec<_>>(),
        [Some((ids.leave.clone(), "rejected"))]
    );

    // Approving a request whose week passed closes it and enqueues "expired".
    let ev = inbox.item(&ids.expired_request).await["version"]
        .as_u64()
        .unwrap();
    let reply = inbox
        .token_call(
            &path(&ids.expired_request, "approve"),
            json!({"version": ev}),
            &[],
        )
        .await;
    assert!(reply.status >= 400, "{}", reply.text());
    let expired = draft_notices(store, &ids.expired_request).await;
    assert_eq!(
        expired.iter().map(decided).collect::<Vec<_>>(),
        [Some((ids.expired_request.clone(), "expired"))]
    );
}

#[tokio::test]
async fn proposal_approvals_enqueue_their_merge_notices() {
    // Each on a fresh inbox: both proposals are about r-kalos.
    for edited in [false, true] {
        let inbox = seeded().await;
        let (id, body) = if edited {
            (&inbox.ids.to_edit, json!({"day": 6, "time": "20:00"}))
        } else {
            (&inbox.ids.moved, json!({}))
        };
        let head = inbox.head().await;
        ok(&inbox.discord(&path(id, "approve"), body, &[]).await);
        let written = crate::outbox::written_by(&inbox.reads.store, head + 1).await;
        let record = inbox
            .reads
            .store
            .load_change(head + 1)
            .await
            .unwrap()
            .unwrap();
        assert!(!written.is_empty(), "{id}");
        assert_eq!(
            written
                .iter()
                .map(kanade::domain::schedule::Notice::effect_kind)
                .collect::<Vec<_>>(),
            record.notices,
            "{id}"
        );
    }
}
