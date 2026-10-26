//! The guide publisher's admin API (module `guides`) over the seeded store
//! of `reads.rs`, with a stub [`GuidePort`] (no Discord): off, every route
//! is the generic 404 (and never on the public origin); on, CSRF, keyed
//! replays, version and plan preconditions, confirmations, the live-run
//! and published refusals, Discord offline, and History rows only with an
//! effect. A finished run is simulated through the store, as the worker
//! would record it. Invented text-only guides (no art is read).

use std::{
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use chrono::{DateTime, TimeZone, Utc};
use kanade::{
    api::{listeners::Site, state::GuidePort},
    infrastructure::{files::GuideFiles, store::SqliteStore},
};
use kanade_domain::{
    catalog::BossTable,
    guides::{
        GuideGrants, GuideStore, Inspection, OpOutcome, PlanOp, PublishOp, PublishState,
        PublishedSlot, SlotEffect, SlotState, plan, plan_unpublish, render,
    },
    settings::{SettingsChangeQuery, SettingsStore},
};
use serde_json::{Value, json};

use crate::{
    reads::{GUIDE_CHANNELS, Reads},
    schemas::assert_valid,
    support::{Fixture, PUBLIC_HOST, send},
};

const LIST: &str = "/api/admin/guides";
const LISTED: &str = "guides.json#/$defs/GuideList";
const SAVED: &str = "guides.json#/$defs/GuideSaved";
const DETAIL: &str = "guides.json#/$defs/GuideDetail";
const PLAN: &str = "guides.json#/$defs/GuidePlan";
const RESULT: &str = "guides.json#/$defs/GuidePublishResult";
const RUN: &str = "guides.json#/$defs/GuideRun";
const SESSION: &str = "identity.json#/$defs/Session";
const ERROR: &str = "error.json#/$defs/ApiError";
const MESSAGE: &str = "common.json#/$defs/Message";
const ONE: &str = "6a2f0b1e-4c1d-4f3a-9b7e-0c1d2e3f4a51";
const TWO: &str = "6a2f0b1e-4c1d-4f3a-9b7e-0c1d2e3f4a52";
const THREE: &str = "6a2f0b1e-4c1d-4f3a-9b7e-0c1d2e3f4a53";

/// The delivery side as the API sees it: a channel read answered from
/// `inspection`, and wakes counted.
struct StubPort {
    inspection: Mutex<Option<Inspection>>,
    wakes: AtomicUsize,
}

/// A visible channel with every grant, `gone` positions and no foreign
/// message after the guide.
fn channel(gone: &[u32]) -> Inspection {
    Inspection::Channel {
        grants: GuideGrants {
            view: true,
            send: true,
            history: true,
            embed: true,
            attach: true,
        },
        gone: gone.iter().copied().collect(),
        newest_message_id: None,
    }
}

impl GuidePort for StubPort {
    fn inspect(
        &self,
        _channel_id: String,
        _slots: Vec<PublishedSlot>,
    ) -> Pin<Box<dyn Future<Output = Option<Inspection>> + Send + '_>> {
        let inspection = self.inspection.lock().unwrap().clone();
        Box::pin(async move { inspection })
    }

    fn wake(&self) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
    }
}

/// Module `guides` on, Discord `connected` or not.
async fn module(connected: bool) -> (Reads, Arc<StubPort>) {
    let port = Arc::new(StubPort {
        inspection: Mutex::new(Some(channel(&[]))),
        wakes: AtomicUsize::new(0),
    });
    let attached: Arc<dyn GuidePort> = port.clone();
    let reads = Reads::build(|b| {
        b.guides = Some(attached);
        b.role_directory_connected = connected;
    })
    .await;
    (reads, port)
}

fn guide(id: &str) -> String {
    format!("{LIST}/{id}")
}

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 0).unwrap()
}

fn text(id: &str, content: &str) -> Value {
    json!({"kind": "text", "id": id, "content": content})
}

fn blocks() -> Value {
    json!([text(ONE, "Bring the invented potion."), {"kind": "spacer", "id": TWO}])
}

/// The guides section's History rows.
async fn history(reads: &Reads) -> usize {
    reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .iter()
        .filter(|change| change.section == "guides")
        .count()
}

/// A guide in the first guide channel with a text block and a spacer, at
/// version 2.
async fn drafted(reads: &Reads) -> String {
    let created = reads
        .call(
            "POST",
            LIST,
            json!({"title": " Kalos basics ", "channel_id": GUIDE_CHANNELS[0]}),
            &[],
        )
        .await
        .expect(201, SAVED);
    let id = created["id"].as_str().unwrap().to_owned();
    let saved = reads
        .ok(
            "PATCH",
            &guide(&id),
            json!({
                "version": 1,
                "title": "Kalos basics",
                "channel_id": GUIDE_CHANNELS[0],
                "blocks": blocks(),
            }),
            SAVED,
        )
        .await;
    assert_eq!(saved["version"], 2);
    id
}

async fn plan_of(reads: &Reads, id: &str, query: &str) -> Value {
    reads
        .read(&format!("{}/plan{query}", guide(id)), PLAN)
        .await
}

/// Record each op of `ops` pending, then settled with its slot effect, and
/// finish the run `done`, as the worker does once Discord confirms.
async fn play(store: &SqliteStore, run_id: &str, ops: Vec<(PlanOp, SlotEffect)>) {
    for (ordinal, (op, effect)) in ops.into_iter().enumerate() {
        let ordinal = u32::try_from(ordinal).unwrap();
        let source =
            matches!(op, PlanOp::Create { .. }).then(|| format!("guide-post:{run_id}-{ordinal}"));
        let pending = PublishOp {
            ordinal,
            op,
            source,
            outcome: OpOutcome::Pending,
            candidates: Vec::new(),
            at: at(),
        };
        store.record_op(run_id.to_owned(), pending).await.unwrap();
        store
            .apply_op_result(
                run_id.to_owned(),
                ordinal,
                OpOutcome::Done,
                effect,
                Vec::new(),
                at(),
            )
            .await
            .unwrap();
    }
    store
        .finish(run_id.to_owned(), PublishState::Done, None, at())
        .await
        .unwrap();
}

fn removed(op: &PlanOp) -> SlotEffect {
    SlotEffect::Remove {
        channel_id: op.channel_id().to_owned(),
        position: op.position(),
    }
}

/// Play the worker on the queued publish: re-plan as it does (the hash must
/// match what the API planned) and settle every op.
async fn settle(store: &SqliteStore, catalog: &BossTable) {
    let run = store.claim_next(at()).await.unwrap().expect("a queued run");
    let saved = store.guide(run.guide_id.clone()).await.unwrap().unwrap();
    let rendered = render(&saved.blocks, catalog, &GuideFiles::default()).unwrap();
    let slots = store.slots(saved.id.clone()).await.unwrap();
    let planned = plan(
        &slots,
        &rendered.slots,
        &run.channel_id,
        run.delete_old,
        None,
    );
    assert_eq!(planned.hash, run.plan_hash, "the API plans as the worker");
    let ops = planned
        .ops
        .into_iter()
        .map(|op| {
            let effect = match &op {
                PlanOp::Create { .. } | PlanOp::Edit { .. } => {
                    let slot = &rendered.slots[usize::try_from(op.position()).unwrap()];
                    let message = op.message_id().map_or_else(
                        || format!("2000000000000000{:02}", op.position()),
                        str::to_owned,
                    );
                    SlotEffect::Put(PublishedSlot {
                        position: op.position(),
                        channel_id: op.channel_id().to_owned(),
                        message_id: Some(message),
                        hash: slot.hash.clone(),
                        uploads: slot.manifest(),
                        state: SlotState::Posted,
                        source: None,
                        publish_id: run.id.clone(),
                        updated_at: at(),
                    })
                }
                PlanOp::Delete { .. } | PlanOp::Forget { .. } => removed(&op),
            };
            (op, effect)
        })
        .collect();
    play(store, &run.id, ops).await;
}

/// Play the worker on the queued unpublish: every slot deleted.
async fn settle_unpublish(store: &SqliteStore) {
    let run = store.claim_next(at()).await.unwrap().expect("a queued run");
    let slots = store.slots(run.guide_id.clone()).await.unwrap();
    let planned = plan_unpublish(&slots, &run.channel_id);
    assert_eq!(planned.hash, run.plan_hash);
    let ops = planned
        .ops
        .into_iter()
        .map(|op| {
            let effect = removed(&op);
            (op, effect)
        })
        .collect();
    play(store, &run.id, ops).await;
}

/// Publish the saved draft as previewed and finish the run.
async fn published(reads: &Reads, id: &str) {
    let planned = plan_of(reads, id, "").await;
    let queued = reads
        .call(
            "POST",
            &format!("{}/publish", guide(id)),
            json!({"version": planned["version"], "plan_hash": planned["plan_hash"]}),
            &[],
        )
        .await
        .expect(202, RESULT);
    assert_eq!(queued["publish"]["state"], "queued");
    settle(&reads.store, &reads.catalog()).await;
}

#[tokio::test]
async fn every_route_is_the_generic_404_while_the_module_is_off() {
    let reads = Reads::new().await;
    let session = reads.read("/api/admin/session", SESSION).await;
    assert_eq!(session["guides"], false);
    let one = guide("g1");
    let routes = [
        ("GET", LIST.to_owned()),
        ("POST", LIST.to_owned()),
        ("GET", one.clone()),
        ("PATCH", one.clone()),
        ("DELETE", one.clone()),
        ("POST", format!("{one}/render")),
        ("GET", format!("{one}/plan")),
        ("POST", format!("{one}/publish")),
        ("POST", format!("{one}/unpublish")),
        ("POST", format!("{one}/search")),
        ("POST", format!("{one}/slots/not-posted")),
        ("GET", format!("{one}/publishes/r1")),
        ("POST", format!("{one}/publishes/r1/cancel")),
    ];
    for (method, path) in &routes {
        reads
            .assert_unmounted(method, path, json!({"title": "x", "version": 1}))
            .await;
    }
    assert_eq!(history(&reads).await, 0);

    // On, the routes are still never on the public origin, even with the
    // admin's session and the module's state behind it.
    let (reads, _) = module(true).await;
    let fixture = Fixture::new();
    let mut public = Site::public(&fixture.http()).unwrap();
    public.state = reads.site.state.clone();
    let public = crate::support::spawn(public).await;
    let headers = [
        ("Cookie", reads.cookie.as_str()),
        ("X-Kanade-CSRF", reads.csrf.as_str()),
    ];
    for (method, path) in &routes {
        let reply = send(public, method, PUBLIC_HOST, path, &headers, Some("{}")).await;
        assert_eq!(reply.status, 404, "{method} {path}");
        assert_valid(ERROR, path, &reply.json());
    }
}

#[tokio::test]
async fn guides_are_created_saved_previewed_and_listed() {
    let (reads, _) = module(true).await;
    let session = reads.read("/api/admin/session", SESSION).await;
    assert_eq!(session["guides"], true);
    let id = drafted(&reads).await;

    let detail = reads.read(&guide(&id), DETAIL).await;
    assert_eq!(detail["guide"]["status"], "not_published");
    assert_eq!(
        detail["guide"]["channel_name"],
        format!("#{}", GUIDE_CHANNELS[0])
    );
    assert_eq!(detail["blocks"][0], text(ONE, "Bring the invented potion."));
    let listed = reads.read(LIST, LISTED).await;
    assert_eq!(listed["guides"][0]["title"], "Kalos basics");

    // The unsaved preview counts UTF-16 units (a surrogate pair is 2).
    let preview = reads
        .ok(
            "POST",
            &format!("{}/render", guide(&id)),
            json!({"blocks": [text(THREE, "🐉 ok")]}),
            "guides.json#/$defs/GuidePreview",
        )
        .await;
    assert_eq!(preview["slots"][0]["content_units"], 5);

    // Every problem is named: block, field, units and limit.
    let refused = reads
        .call(
            "PATCH",
            &guide(&id),
            json!({
                "version": 2,
                "title": "Kalos basics",
                "channel_id": GUIDE_CHANNELS[0],
                "blocks": [text(ONE, &"x".repeat(2001))],
            }),
            &[],
        )
        .await
        .expect(422, "guides.json#/$defs/GuideBlocksRefused");
    let problem = &refused["problems"][0];
    assert_eq!(refused["error"], "invalid_blocks");
    assert_eq!(
        (
            &problem["block_id"],
            &problem["field"],
            &problem["units"],
            &problem["limit"]
        ),
        (&json!(ONE), &json!("content"), &json!(2001), &json!(2000))
    );

    // A channel the bot does not list, and one another guide uses.
    let unlisted = reads
        .call(
            "POST",
            LIST,
            json!({"title": "Other", "channel_id": "kalos-four"}),
            &[],
        )
        .await;
    assert_eq!(unlisted.error(), (422, "invalid".into()));
    let taken = reads
        .call(
            "POST",
            LIST,
            json!({"title": "Other", "channel_id": GUIDE_CHANNELS[0]}),
            &[],
        )
        .await;
    assert_eq!(taken.error(), (409, "channel_taken".into()));
}

#[tokio::test]
async fn writes_need_the_csrf_token() {
    let (reads, _) = module(true).await;
    let id = drafted(&reads).await;
    for (method, path, body) in [
        ("POST", LIST.to_owned(), json!({"title": "No token"})),
        (
            "PATCH",
            guide(&id),
            json!({"version": 2, "title": "x", "channel_id": null, "blocks": []}),
        ),
        (
            "POST",
            format!("{}/publish", guide(&id)),
            json!({"version": 2, "plan_hash": "0"}),
        ),
    ] {
        let refused = reads.without_csrf(method, &path, &body).await;
        assert_eq!(refused.error(), (403, "csrf".into()), "{method} {path}");
    }
    assert_eq!(history(&reads).await, 1, "only the create");
}

#[tokio::test]
async fn keyed_writes_replay_their_first_answer() {
    let (reads, port) = module(true).await;
    let body = json!({"title": "Lotus", "channel_id": GUIDE_CHANNELS[1]});
    let first = reads.keyed("POST", LIST, "create-1", body.clone()).await;
    let again = reads.keyed("POST", LIST, "create-1", body).await;
    assert_eq!(first.expect(201, SAVED), again.expect(201, SAVED));
    let listed = reads.read(LIST, LISTED).await;
    assert_eq!(listed["guides"].as_array().unwrap().len(), 1);
    let other = reads
        .keyed("POST", LIST, "create-1", json!({"title": "Lotus 2"}))
        .await;
    assert_eq!(other.error(), (422, "idempotency_mismatch".into()));
    assert_eq!(history(&reads).await, 1);

    // A double-clicked Publish queues one run and wakes the worker once.
    let id = drafted(&reads).await;
    let planned = plan_of(&reads, &id, "").await;
    let body = json!({"version": 2, "plan_hash": planned["plan_hash"]});
    let path = format!("{}/publish", guide(&id));
    let first = reads.keyed("POST", &path, "publish-1", body.clone()).await;
    let again = reads.keyed("POST", &path, "publish-1", body).await;
    assert_eq!(first.expect(202, RESULT), again.expect(202, RESULT));
    assert_eq!(port.wakes.load(Ordering::SeqCst), 1);
    let mismatch = reads
        .keyed(
            "POST",
            &path,
            "publish-1",
            json!({"version": 2, "plan_hash": "0"}),
        )
        .await;
    assert_eq!(mismatch.error(), (422, "idempotency_mismatch".into()));
}

#[tokio::test]
async fn stale_versions_changed_plans_and_live_runs_are_refused() {
    let (reads, port) = module(true).await;
    let id = drafted(&reads).await;
    let before = history(&reads).await;

    let stale = reads
        .call(
            "PATCH",
            &guide(&id),
            json!({"version": 1, "title": "Old", "channel_id": null, "blocks": []}),
            &[],
        )
        .await;
    assert_eq!(stale.error(), (409, "stale".into()));
    let planned = plan_of(&reads, &id, "").await;
    assert_eq!(
        (&planned["checked"], &planned["counts"]["create"]),
        (&json!(true), &json!(2))
    );
    let publish = format!("{}/publish", guide(&id));
    let stale = reads
        .call(
            "POST",
            &publish,
            json!({"version": 1, "plan_hash": planned["plan_hash"]}),
            &[],
        )
        .await;
    assert_eq!(stale.error(), (409, "stale".into()));
    assert_eq!(history(&reads).await, before, "refusals record nothing");

    // Published: nothing to do, until the channel shows message 2 gone and
    // the plan the admin previewed is no longer the one that would run.
    published(&reads, &id).await;
    let detail = reads.read(&guide(&id), DETAIL).await;
    assert_eq!(
        (&detail["guide"]["status"], &detail["guide"]["messages"]),
        (&json!("published"), &json!(2))
    );
    let noop = plan_of(&reads, &id, "").await;
    assert_eq!(noop["no_op"], true);
    let body = json!({"version": 2, "plan_hash": noop["plan_hash"]});
    let answered = reads
        .call("POST", &publish, body.clone(), &[])
        .await
        .expect(200, RESULT);
    assert_eq!(answered, json!({"no_op": true, "publish": null}));
    *port.inspection.lock().unwrap() = Some(channel(&[1]));
    let changed = reads.call("POST", &publish, body, &[]).await;
    assert_eq!(changed.error(), (409, "plan_changed".into()));

    // While a run is queued, another publish and a delete wait.
    let replanned = plan_of(&reads, &id, "").await;
    let body = json!({"version": 2, "plan_hash": replanned["plan_hash"]});
    reads
        .call("POST", &publish, body.clone(), &[])
        .await
        .expect(202, RESULT);
    let busy = reads.call("POST", &publish, body, &[]).await;
    assert_eq!(busy.error(), (409, "publishing".into()));
    let delete = reads
        .call("DELETE", &guide(&id), json!({"version": 2}), &[])
        .await;
    assert_eq!(delete.error(), (409, "publishing".into()));
    let listed = reads.read(LIST, LISTED).await;
    let row = &listed["guides"][0];
    assert_eq!(row["status"], "publishing");
    assert_eq!(row["progress"], json!({"done": 0, "total": 1}));

    // The progress screen, then a cancel the worker will honour.
    let run_id = row["last_run"]["id"].as_str().unwrap().to_owned();
    let run = reads
        .read(&format!("{}/publishes/{run_id}", guide(&id)), RUN)
        .await;
    assert_eq!(run["state"], "queued");
    let cancelled = reads
        .ok(
            "POST",
            &format!("{}/publishes/{run_id}/cancel", guide(&id)),
            json!({}),
            RUN,
        )
        .await;
    assert_eq!(cancelled["cancel_requested_at"], "2026-09-29T04:00:00Z");
}

#[tokio::test]
async fn confirmations_are_required_and_published_guides_unpublish_first() {
    let (reads, _) = module(true).await;
    let id = drafted(&reads).await;
    published(&reads, &id).await;

    let delete = reads
        .call("DELETE", &guide(&id), json!({"version": 2}), &[])
        .await;
    assert_eq!(delete.error(), (409, "unpublish_first".into()));

    // A channel change needs `channel_move`.
    reads
        .ok(
            "PATCH",
            &guide(&id),
            json!({
                "version": 2,
                "title": "Kalos basics",
                "channel_id": GUIDE_CHANNELS[1],
                "blocks": blocks(),
            }),
            SAVED,
        )
        .await;
    let moved = plan_of(&reads, &id, "").await;
    assert_eq!(moved["needs"]["channel_move"], true);
    assert_eq!(
        (&moved["counts"]["create"], &moved["counts"]["delete"]),
        (&json!(2), &json!(2))
    );
    let before = history(&reads).await;
    let publish = format!("{}/publish", guide(&id));
    let asked = reads
        .call(
            "POST",
            &publish,
            json!({"version": 3, "plan_hash": moved["plan_hash"]}),
            &[],
        )
        .await
        .expect(422, "guides.json#/$defs/GuideNeedsConfirmation");
    assert_eq!(asked["needs"]["channel_move"], true);
    assert_eq!(history(&reads).await, before, "nothing queued");
    let confirmed = json!({
        "version": 3,
        "plan_hash": moved["plan_hash"],
        "confirm": {"channel_move": true},
    });
    reads
        .call("POST", &publish, confirmed, &[])
        .await
        .expect(202, RESULT);
    assert_eq!(
        history(&reads).await,
        before + 1,
        "the request's History row"
    );
    settle(&reads.store, &reads.catalog()).await;

    // Unpublish (confirmed) is a run that deletes everything; then the
    // guide can go.
    let unpublish = plan_of(&reads, &id, "?kind=unpublish").await;
    assert_eq!(unpublish["counts"]["delete"], 2);
    let path = format!("{}/unpublish", guide(&id));
    let unconfirmed = reads
        .call(
            "POST",
            &path,
            json!({"plan_hash": unpublish["plan_hash"]}),
            &[],
        )
        .await;
    assert_eq!(unconfirmed.error(), (422, "confirm_required".into()));
    let queued = reads
        .call(
            "POST",
            &path,
            json!({"plan_hash": unpublish["plan_hash"], "confirm": true}),
            &[],
        )
        .await
        .expect(202, RESULT);
    assert_eq!(queued["publish"]["kind"], "unpublish");
    settle_unpublish(&reads.store).await;
    let deleted = reads
        .ok("DELETE", &guide(&id), json!({"version": 3}), MESSAGE)
        .await;
    assert_eq!(deleted["message"], "Kalos basics is deleted.");
}

#[tokio::test]
async fn publishing_needs_discord() {
    let (reads, port) = module(false).await;
    let id = drafted(&reads).await;
    // Offline the plan is "Unchecked": no channel read is asked.
    let planned = plan_of(&reads, &id, "").await;
    assert_eq!(planned["checked"], false);
    let before = history(&reads).await;
    for (path, body) in [
        (
            "publish",
            json!({"version": 2, "plan_hash": planned["plan_hash"]}),
        ),
        (
            "unpublish",
            json!({"plan_hash": planned["plan_hash"], "confirm": true}),
        ),
        ("search", json!({})),
    ] {
        let offline = reads
            .call("POST", &format!("{}/{path}", guide(&id)), body, &[])
            .await;
        assert_eq!(offline.error(), (503, "discord_offline".into()), "{path}");
    }
    assert_eq!(history(&reads).await, before);
    assert_eq!(port.wakes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn history_rows_are_written_only_with_an_effect() {
    let (reads, _) = module(true).await;
    let id = drafted(&reads).await;
    assert_eq!(
        history(&reads).await,
        1,
        "the create; draft saves are unlisted"
    );
    // An unchanged form answers its version and records nothing.
    let same = reads
        .ok(
            "PATCH",
            &guide(&id),
            json!({
                "version": 2,
                "title": "Kalos basics",
                "channel_id": GUIDE_CHANNELS[0],
                "blocks": blocks(),
            }),
            SAVED,
        )
        .await;
    assert_eq!(same["version"], 2);
    // A rename is listed.
    let renamed = json!({
        "version": 2,
        "title": "Renamed",
        "channel_id": GUIDE_CHANNELS[0],
        "blocks": [],
    });
    reads.ok("PATCH", &guide(&id), renamed, SAVED).await;
    assert_eq!(history(&reads).await, 2);
    // Refusals: nothing to search for, a stale plan hash, a slot that is
    // not waiting to be settled.
    let nothing = reads
        .call("POST", &format!("{}/search", guide(&id)), json!({}), &[])
        .await;
    assert_eq!(nothing.error(), (409, "nothing_unknown".into()));
    let changed = reads
        .call(
            "POST",
            &format!("{}/publish", guide(&id)),
            json!({"version": 3, "plan_hash": "0".repeat(64)}),
            &[],
        )
        .await;
    assert_eq!(changed.error(), (409, "plan_changed".into()));
    let not_posted = reads
        .call(
            "POST",
            &format!("{}/slots/not-posted", guide(&id)),
            json!({"channel_id": GUIDE_CHANNELS[0], "position": 0, "confirm": true}),
            &[],
        )
        .await;
    assert_eq!(not_posted.error(), (404, "not_found".into()));
    assert_eq!(history(&reads).await, 2);
}
