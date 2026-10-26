//! Guides every store must keep (0042): guide writes behind a version (a
//! stale, missing, refused or unchanged write writes nothing), channels
//! unique across guides, at most one queued or running publish per guide,
//! ops recorded pending and settled once with their slot effect in the same
//! write, slots identified by `(channel, position)` so a channel move keeps
//! the old message ids, "Mark as not posted" only for unknown slots of an
//! idle guide, cancels asked of live runs, deleting a guide drops its slots
//! and runs, History rows (section `guides`) only with an effect, and keyed
//! writes replaying in the same transaction.

use chrono::{DateTime, TimeZone, Utc};
use serde_json::{Value, json};

use super::fixtures::{refused, refused_for};
use crate::replays::{KeyedWrite, ReplayScope, ReplayStore, StoredReplay};
use kanade_domain::guides::{
    Block, CancelRequest, Confirmations, GUIDES_SECTION, Guide, GuideChange, GuideRecord,
    GuideStore, GuideWrite, NotPosted, OpOutcome, PlanCounts, PlanOp, Publish, PublishKind,
    PublishOp, PublishRequest, PublishRequested, PublishState, PublishedSlot, SlotEffect,
    SlotState, StopReason, UploadRef,
};
use kanade_domain::history::{Actor, Surface};
use kanade_domain::settings::{SettingsChange, SettingsChangeQuery, SettingsStore};

const GUIDE: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";
const NOBODY: &str = "00000000-0000-4000-8000-000000000009";
const CHANNEL: &str = "100000000000000001";
const MOVED: &str = "100000000000000002";

pub async fn run_suite<S: GuideStore + ReplayStore + SettingsStore>(make: impl AsyncFn() -> S) {
    guides_are_written_behind_a_version(make().await).await;
    channels_are_unique_across_guides(make().await).await;
    one_run_is_live_per_guide(make().await).await;
    ops_settle_once_with_their_slot_effect(make().await).await;
    only_unknown_slots_are_marked_not_posted(make().await).await;
    slots_keep_their_identity_across_a_channel_move(make().await).await;
    cancels_are_asked_of_live_runs(make().await).await;
    deleting_a_guide_drops_its_slots_and_runs(make().await).await;
    keyed_guide_writes_replay_in_the_same_write(make().await).await;
}

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 16, hour, 0, 0).unwrap()
}

fn record(hour: u32) -> GuideRecord {
    GuideRecord {
        at: at(hour),
        actor: Actor::admin("token"),
        surface: Surface::AdminPortal,
    }
}

fn hash(fill: char) -> String {
    std::iter::repeat_n(fill, 64).collect()
}

fn message(n: u64) -> String {
    (1_000_000_000_000_000_000 + n).to_string()
}

fn source(n: u32) -> String {
    format!("guide-post:00000000-0000-4000-a000-{n:012}")
}

fn text(n: u32, content: &str) -> Block {
    Block::Text {
        id: format!("00000000-0000-4000-9000-{n:012}"),
        content: content.into(),
    }
}

fn create(id: &str, title: &str, channel: Option<&str>) -> GuideChange {
    GuideChange::Create {
        id: id.into(),
        title: title.into(),
        channel_id: channel.map(Into::into),
    }
}

fn save(
    id: &str,
    version: u32,
    title: &str,
    channel: Option<&str>,
    blocks: &[Block],
) -> GuideChange {
    GuideChange::Save {
        id: id.into(),
        version,
        title: title.into(),
        channel_id: channel.map(Into::into),
        blocks: blocks.to_vec(),
    }
}

fn delete(id: &str, version: u32) -> GuideChange {
    GuideChange::Delete {
        id: id.into(),
        version,
    }
}

async fn created<S: GuideStore>(store: &S, id: &str, title: &str, channel: Option<&str>) -> Guide {
    match store
        .apply_guide(create(id, title, channel), record(1))
        .await
        .unwrap()
    {
        GuideWrite::Created(guide) => guide,
        other => panic!("expected a created guide, got {other:?}"),
    }
}

async fn saved<S: GuideStore>(store: &S, change: GuideChange, hour: u32) -> Guide {
    match store.apply_guide(change, record(hour)).await.unwrap() {
        GuideWrite::Saved(guide) => guide,
        other => panic!("expected a saved guide, got {other:?}"),
    }
}

/// The guides section's History rows, oldest first.
async fn history<S: SettingsStore>(store: &S) -> Vec<SettingsChange> {
    let mut rows: Vec<SettingsChange> = store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|change| change.section == GUIDES_SECTION)
        .collect();
    rows.reverse();
    rows
}

/// Each History row's only key, oldest first.
async fn keys<S: SettingsStore>(store: &S) -> Vec<String> {
    history(store)
        .await
        .iter()
        .map(|change| {
            assert_eq!(change.values.len(), 1, "one key per guide row");
            change.values.keys().next().unwrap().clone()
        })
        .collect()
}

fn side(text: &str) -> Value {
    if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(text).unwrap()
    }
}

fn run(id: &str, guide: &str, version: u32, hour: u32) -> Publish {
    Publish {
        id: id.into(),
        guide_id: guide.into(),
        version,
        plan_hash: hash('e'),
        channel_id: CHANNEL.into(),
        kind: PublishKind::Publish,
        confirmed: Confirmations::default(),
        delete_old: true,
        state: PublishState::Queued,
        stop_reason: None,
        requested_by: "admin:token".into(),
        requested_at: at(hour),
        started_at: None,
        finished_at: None,
        cancel_requested_at: None,
    }
}

fn request(publish: Publish) -> PublishRequest {
    PublishRequest {
        publish,
        counts: PlanCounts {
            keep: 1,
            edit: 0,
            create: 2,
            delete: 0,
        },
    }
}

/// `publish` requested, recorded at its `requested_at`.
async fn queued<S: GuideStore>(store: &S, publish: Publish) -> Publish {
    let record = GuideRecord {
        at: publish.requested_at,
        ..record(1)
    };
    match store
        .request_publish(request(publish), record)
        .await
        .unwrap()
    {
        PublishRequested::Queued(queued) => queued,
        other => panic!("expected a queued run, got {other:?}"),
    }
}

/// `publish` queued and claimed at `hour` (no other run queued).
async fn started<S: GuideStore>(store: &S, publish: Publish, hour: u32) -> Publish {
    let id = queued(store, publish).await.id;
    let claimed = store.claim_next(at(hour)).await.unwrap().expect("claimed");
    assert_eq!(claimed.id, id);
    claimed
}

fn posted(channel: &str, position: u32, message_n: u64, run: &str, hour: u32) -> PublishedSlot {
    PublishedSlot {
        position,
        channel_id: channel.into(),
        message_id: Some(message(message_n)),
        hash: hash('a'),
        uploads: vec![UploadRef {
            file_name: "MaleficStar-portrait.png".into(),
            sha256: hash('b'),
        }],
        state: SlotState::Posted,
        source: None,
        publish_id: run.into(),
        updated_at: at(hour),
    }
}

fn unknown(channel: &str, position: u32, source_n: u32, run: &str, hour: u32) -> PublishedSlot {
    PublishedSlot {
        message_id: None,
        state: SlotState::Unknown,
        source: Some(source(source_n)),
        ..posted(channel, position, 0, run, hour)
    }
}

fn create_op(channel: &str, position: u32) -> PlanOp {
    PlanOp::Create {
        position,
        channel_id: channel.into(),
        hash: hash('a'),
    }
}

fn pending(ordinal: u32, op: PlanOp, source: Option<String>, hour: u32) -> PublishOp {
    PublishOp {
        ordinal,
        op,
        source,
        outcome: OpOutcome::Pending,
        candidates: Vec::new(),
        at: at(hour),
    }
}

/// Record op `ordinal` pending, then settle it with `effect`.
async fn settle<S: GuideStore>(
    store: &S,
    run: &str,
    op: PublishOp,
    outcome: OpOutcome,
    effect: SlotEffect,
) {
    settle_found(store, run, op, outcome, effect, Vec::new()).await;
}

/// [`settle`], with the messages a channel search matched.
async fn settle_found<S: GuideStore>(
    store: &S,
    run: &str,
    op: PublishOp,
    outcome: OpOutcome,
    effect: SlotEffect,
    candidates: Vec<String>,
) {
    let (ordinal, hour) = (op.ordinal, op.at);
    store.record_op(run.into(), op).await.unwrap();
    store
        .apply_op_result(run.into(), ordinal, outcome, effect, candidates, hour)
        .await
        .unwrap();
}

/// A create at `position` in `channel` that Discord confirmed.
async fn bound<S: GuideStore>(store: &S, run: &str, ordinal: u32, slot: PublishedSlot) {
    let op = pending(
        ordinal,
        create_op(&slot.channel_id, slot.position),
        Some(source(ordinal + 100)),
        3,
    );
    settle(store, run, op, OpOutcome::Done, SlotEffect::Put(slot)).await;
}

async fn finished<S: GuideStore>(
    store: &S,
    run: &str,
    state: PublishState,
    reason: Option<StopReason>,
    hour: u32,
) -> Publish {
    store
        .finish(run.into(), state, reason, at(hour))
        .await
        .unwrap()
}

async fn guides_are_written_behind_a_version<S: GuideStore + SettingsStore>(store: S) {
    let first = created(&store, GUIDE, "Lotus", Some(CHANNEL)).await;
    assert_eq!(
        (first.version, first.blocks.len(), first.created_by.as_str()),
        (1, 0, "admin:token")
    );
    assert_eq!(
        store.guide(GUIDE.into()).await.unwrap(),
        Some(first.clone())
    );
    refused_for(
        "ids are unique",
        store
            .apply_guide(create(GUIDE, "Again", None), record(1))
            .await,
    );

    let blocks = [text(1, "Phase one: stay left.")];
    let drafted = saved(&store, save(GUIDE, 1, "Lotus", Some(CHANNEL), &blocks), 2).await;
    assert_eq!(
        (
            drafted.version,
            drafted.blocks.as_slice(),
            drafted.updated_at
        ),
        (2, blocks.as_slice(), at(2))
    );
    assert_eq!(drafted.created_at, at(1));
    // The same form again writes nothing.
    assert_eq!(
        saved(&store, save(GUIDE, 2, "Lotus", Some(CHANNEL), &blocks), 3).await,
        drafted
    );
    assert_eq!(
        store
            .apply_guide(save(GUIDE, 1, "Lotus", Some(CHANNEL), &[]), record(3))
            .await
            .unwrap(),
        GuideWrite::Stale(drafted.clone())
    );
    for change in [save(NOBODY, 1, "Lotus", None, &[]), delete(NOBODY, 1)] {
        assert_eq!(
            store.apply_guide(change, record(3)).await.unwrap(),
            GuideWrite::Missing
        );
    }
    assert_eq!(
        keys(&store).await,
        [format!("guide.{GUIDE}")],
        "draft saves, no-ops and refusals are not listed"
    );

    let renamed = saved(
        &store,
        save(GUIDE, 2, "Lotus (hard)", Some(CHANNEL), &blocks),
        4,
    )
    .await;
    assert_eq!(renamed.version, 3);
    let rename = history(&store).await.pop().unwrap();
    let row = &rename.values[&format!("guide.{GUIDE}")];
    assert_eq!(
        side(&row.from),
        json!({"title": "Lotus", "channel": CHANNEL})
    );
    assert_eq!(
        side(&row.to),
        json!({"title": "Lotus (hard)", "channel": CHANNEL})
    );
    assert_eq!(
        (rename.actor, rename.surface, rename.at),
        (Actor::admin("token"), Surface::AdminPortal, at(4))
    );

    let other = created(&store, OTHER, "Akechi", None).await;
    assert_eq!(
        store.guides().await.unwrap(),
        [other, renamed.clone()],
        "by title"
    );
    assert_eq!(
        store
            .apply_guide(delete(GUIDE, 2), record(5))
            .await
            .unwrap(),
        GuideWrite::Stale(renamed.clone())
    );
    assert_eq!(
        store
            .apply_guide(delete(GUIDE, 3), record(5))
            .await
            .unwrap(),
        GuideWrite::Deleted(renamed)
    );
    assert_eq!(store.guide(GUIDE.into()).await.unwrap(), None);
    let removal = history(&store).await.pop().unwrap();
    let row = &removal.values[&format!("guide.{GUIDE}")];
    assert_eq!(
        (side(&row.from), row.to.as_str()),
        (json!({"title": "Lotus (hard)", "channel": CHANNEL}), "")
    );
    assert_eq!(
        keys(&store).await.len(),
        4,
        "create, rename, create, delete"
    );
}

async fn channels_are_unique_across_guides<S: GuideStore + SettingsStore>(store: S) {
    created(&store, GUIDE, "Lotus", Some(CHANNEL)).await;
    assert_eq!(
        store
            .apply_guide(create(OTHER, "Damien", Some(CHANNEL)), record(1))
            .await
            .unwrap(),
        GuideWrite::ChannelTaken
    );
    assert_eq!(store.guide(OTHER.into()).await.unwrap(), None);
    created(&store, OTHER, "Damien", None).await;
    assert_eq!(
        store
            .apply_guide(save(OTHER, 1, "Damien", Some(CHANNEL), &[]), record(2))
            .await
            .unwrap(),
        GuideWrite::ChannelTaken
    );
    // A guide's own channel is not taken from it.
    saved(
        &store,
        save(GUIDE, 1, "Lotus", Some(CHANNEL), &[text(1, "x")]),
        2,
    )
    .await;
    let moved = saved(
        &store,
        save(GUIDE, 2, "Lotus", Some(MOVED), &[text(1, "x")]),
        3,
    )
    .await;
    assert_eq!(moved.channel_id.as_deref(), Some(MOVED));
    let other = saved(&store, save(OTHER, 1, "Damien", Some(CHANNEL), &[]), 4).await;
    assert_eq!(other.channel_id.as_deref(), Some(CHANNEL));
    refused_for(
        "a channel is a snowflake",
        store
            .apply_guide(save(OTHER, 2, "Damien", Some("900"), &[]), record(5))
            .await,
    );
    refused_for(
        "titles are 1-100 characters",
        store
            .apply_guide(save(OTHER, 2, "", Some(CHANNEL), &[]), record(5))
            .await,
    );
    let channel_change = &history(&store).await[2];
    let row = &channel_change.values[&format!("guide.{GUIDE}")];
    assert_eq!(side(&row.to), json!({"title": "Lotus", "channel": MOVED}));
    assert_eq!(
        keys(&store).await,
        [
            format!("guide.{GUIDE}"),
            format!("guide.{OTHER}"),
            format!("guide.{GUIDE}"),
            format!("guide.{OTHER}"),
        ],
        "creates and channel changes; the draft save and refusals are not listed"
    );
}

async fn one_run_is_live_per_guide<S: GuideStore + SettingsStore>(store: S) {
    let guide = created(&store, GUIDE, "Lotus", Some(CHANNEL)).await;
    created(&store, OTHER, "Damien", Some(MOVED)).await;
    let first = queued(&store, run("run-1", GUIDE, 1, 2)).await;
    assert_eq!(first, run("run-1", GUIDE, 1, 2));
    for (publish, expected) in [
        (
            run("run-2", GUIDE, 1, 3),
            PublishRequested::Busy(first.clone()),
        ),
        (
            run("run-3", GUIDE, 2, 3),
            PublishRequested::Stale(guide.clone()),
        ),
        (run("run-4", NOBODY, 1, 3), PublishRequested::Missing),
    ] {
        assert_eq!(
            store
                .request_publish(request(publish), record(3))
                .await
                .unwrap(),
            expected
        );
    }
    let mut running = run("run-6", OTHER, 1, 3);
    running.state = PublishState::Running;
    refused_for(
        "requests start queued",
        store.request_publish(request(running), record(3)).await,
    );
    let mut other = run("run-5", OTHER, 1, 3);
    other.kind = PublishKind::Search;
    other.channel_id = MOVED.into();
    other.confirmed.repost_from = Some(2);
    other.delete_old = false;
    let other = queued(&store, other).await;
    assert_eq!(
        store.publish("run-5".into()).await.unwrap(),
        Some((other.clone(), Vec::new()))
    );

    // Oldest first, across guides.
    let claimed = store.claim_next(at(4)).await.unwrap().expect("run-1");
    assert_eq!(
        (claimed.id.as_str(), claimed.state, claimed.started_at),
        ("run-1", PublishState::Running, Some(at(4)))
    );
    let next = store.claim_next(at(4)).await.unwrap().expect("run-5");
    assert_eq!(next.id, "run-5");
    assert_eq!(store.claim_next(at(4)).await.unwrap(), None);
    assert_eq!(
        store
            .request_publish(request(run("run-2", GUIDE, 1, 3)), record(3))
            .await
            .unwrap(),
        PublishRequested::Busy(claimed.clone())
    );
    assert_eq!(
        store.latest_publish(GUIDE.into()).await.unwrap(),
        Some(claimed)
    );

    let done = finished(&store, "run-1", PublishState::Done, None, 5).await;
    assert_eq!(
        (done.state, done.finished_at),
        (PublishState::Done, Some(at(5)))
    );
    refused_for(
        "a finished run stays finished",
        store
            .finish("run-1".into(), PublishState::Cancelled, None, at(6))
            .await,
    );
    refused_for(
        "a run ends in a final state",
        store
            .finish("run-5".into(), PublishState::Queued, None, at(6))
            .await,
    );
    refused_for(
        "a stopped run names its reason",
        store
            .finish("run-5".into(), PublishState::Stopped, None, at(6))
            .await,
    );
    refused_for(
        "only a stopped run has a reason",
        store
            .finish(
                "run-5".into(),
                PublishState::Done,
                Some(StopReason::Rejected),
                at(6),
            )
            .await,
    );
    let second = queued(&store, run("run-2", GUIDE, 1, 6)).await;
    assert_eq!(
        store.latest_publish(GUIDE.into()).await.unwrap(),
        Some(second)
    );

    let rows = history(&store).await;
    assert_eq!(
        keys(&store).await,
        [
            format!("guide.{GUIDE}"),
            format!("guide.{OTHER}"),
            "publish.run-1".into(),
            "publish.run-5".into(),
            "publish.run-1".into(),
            "publish.run-2".into(),
        ],
        "busy, stale, missing and refused requests are not listed"
    );
    let asked = &rows[3].values["publish.run-5"];
    assert_eq!(side(&asked.from), json!({"slots": 0}));
    assert_eq!(
        side(&asked.to),
        json!({
            "kind": "search",
            "version": 1,
            "channel": MOVED,
            "plan": hash('e'),
            "ops": {"keep": 1, "edit": 0, "create": 2, "delete": 0},
            "confirmed": {
                "repost_from": 2,
                "interleaved": false,
                "channel_move": false,
                "delete_old": false,
            },
        })
    );
    let outcome = &rows[4];
    assert_eq!(
        (&outcome.actor, outcome.at),
        (&Actor::system("guides"), at(5))
    );
    let row = &outcome.values["publish.run-1"];
    assert_eq!(side(&row.from), json!({"state": "running"}));
    assert_eq!(
        side(&row.to),
        json!({"state": "done", "stop_reason": null, "ops": {}})
    );
}

async fn ops_settle_once_with_their_slot_effect<S: GuideStore + SettingsStore>(store: S) {
    created(&store, GUIDE, "Lotus", Some(CHANNEL)).await;
    queued(&store, run("run-1", GUIDE, 1, 2)).await;
    refused_for(
        "ops belong to a running run",
        store
            .record_op(
                "run-1".into(),
                pending(0, create_op(CHANNEL, 0), Some(source(1)), 3),
            )
            .await,
    );
    store.claim_next(at(2)).await.unwrap();

    let slot_0 = posted(CHANNEL, 0, 1, "run-1", 3);
    bound(&store, "run-1", 0, slot_0.clone()).await;
    // An unconfirmed create the search matched twice: indeterminate, the
    // candidates kept in the op log.
    let slot_1 = unknown(CHANNEL, 1, 2, "run-1", 3);
    settle_found(
        &store,
        "run-1",
        pending(1, create_op(CHANNEL, 1), Some(source(2)), 3),
        OpOutcome::Unknown,
        SlotEffect::Put(slot_1.clone()),
        vec![message(7), message(8)],
    )
    .await;
    // Not sent: nothing landed.
    settle(
        &store,
        "run-1",
        pending(2, create_op(CHANNEL, 2), Some(source(3)), 3),
        OpOutcome::Skipped,
        SlotEffect::Unchanged,
    )
    .await;
    assert_eq!(
        store.slots(GUIDE.into()).await.unwrap(),
        [slot_0.clone(), slot_1.clone()]
    );
    let (_, ops) = store.publish("run-1".into()).await.unwrap().unwrap();
    assert_eq!(
        ops.iter()
            .map(|op| (op.ordinal, op.op.clone(), op.outcome))
            .collect::<Vec<_>>(),
        [
            (0, create_op(CHANNEL, 0), OpOutcome::Done),
            (1, create_op(CHANNEL, 1), OpOutcome::Unknown),
            (2, create_op(CHANNEL, 2), OpOutcome::Skipped),
        ]
    );
    assert_eq!(ops[1].source, Some(source(2)));
    assert_eq!(
        ops.iter()
            .map(|op| op.candidates.clone())
            .collect::<Vec<_>>(),
        [vec![], vec![message(7), message(8)], vec![]],
        "candidates only where a search matched"
    );

    let settle_again = |ordinal: u32, outcome: OpOutcome, effect: SlotEffect| {
        store.apply_op_result("run-1".into(), ordinal, outcome, effect, Vec::new(), at(4))
    };
    refused_for(
        "an op settles once",
        settle_again(0, OpOutcome::Gone, SlotEffect::Unchanged).await,
    );
    refused_for(
        "only recorded ops settle",
        settle_again(9, OpOutcome::Done, SlotEffect::Unchanged).await,
    );
    refused_for(
        "an op settles to a final outcome",
        settle_again(3, OpOutcome::Pending, SlotEffect::Unchanged).await,
    );
    store
        .record_op(
            "run-1".into(),
            pending(3, create_op(CHANNEL, 2), Some(source(4)), 4),
        )
        .await
        .unwrap();
    refused_for(
        "a slot names the run that wrote it",
        settle_again(
            3,
            OpOutcome::Done,
            SlotEffect::Put(posted(CHANNEL, 2, 4, "run-9", 4)),
        )
        .await,
    );
    let refusals = [
        (
            "ordinals are unique",
            pending(0, create_op(CHANNEL, 3), Some(source(5)), 4),
        ),
        (
            "a create names its journal source",
            pending(4, create_op(CHANNEL, 3), None, 4),
        ),
        (
            "ops are recorded pending",
            PublishOp {
                outcome: OpOutcome::Done,
                ..pending(4, create_op(CHANNEL, 3), Some(source(5)), 4)
            },
        ),
        (
            "candidates come with the result, not the record",
            PublishOp {
                candidates: vec![message(9)],
                ..pending(4, create_op(CHANNEL, 3), Some(source(5)), 4)
            },
        ),
    ];
    for (check, op) in refusals {
        refused_for(check, store.record_op("run-1".into(), op).await);
    }
    // The refused settle left op 3 pending; it settles now.
    settle_again(
        3,
        OpOutcome::Done,
        SlotEffect::Put(posted(CHANNEL, 2, 4, "run-1", 4)),
    )
    .await
    .unwrap();
    finished(
        &store,
        "run-1",
        PublishState::Stopped,
        Some(StopReason::UnknownSlot),
        5,
    )
    .await;
    refused_for(
        "a finished run takes no ops",
        store
            .record_op(
                "run-1".into(),
                pending(4, create_op(CHANNEL, 3), Some(source(5)), 5),
            )
            .await,
    );
    let outcome = history(&store).await.pop().unwrap();
    assert_eq!(
        side(&outcome.values["publish.run-1"].to),
        json!({
            "state": "stopped",
            "stop_reason": "unknown_slot",
            "ops": {"done": 2, "unknown": 1, "skipped": 1},
        })
    );

    // A hand-deleted message is gone (its id kept); a gone slot is forgotten.
    let second = started(&store, run("run-2", GUIDE, 1, 6), 6).await;
    let gone = PublishedSlot {
        state: SlotState::Gone,
        publish_id: second.id.clone(),
        updated_at: at(7),
        ..slot_0.clone()
    };
    settle(
        &store,
        "run-2",
        pending(
            0,
            PlanOp::Edit {
                position: 0,
                channel_id: CHANNEL.into(),
                message_id: message(1),
                hash: hash('c'),
            },
            None,
            7,
        ),
        OpOutcome::Gone,
        SlotEffect::Put(gone.clone()),
    )
    .await;
    assert_eq!(store.slots(GUIDE.into()).await.unwrap()[0], gone);
    settle(
        &store,
        "run-2",
        pending(
            1,
            PlanOp::Forget {
                position: 0,
                channel_id: CHANNEL.into(),
            },
            None,
            7,
        ),
        OpOutcome::Done,
        SlotEffect::Remove {
            channel_id: CHANNEL.into(),
            position: 0,
        },
    )
    .await;
    let slots = store.slots(GUIDE.into()).await.unwrap();
    assert_eq!(
        slots
            .iter()
            .map(|slot| (slot.position, slot.state))
            .collect::<Vec<_>>(),
        [(1, SlotState::Unknown), (2, SlotState::Posted)]
    );
    refused(
        store
            .apply_op_result(
                "run-2".into(),
                1,
                OpOutcome::Done,
                SlotEffect::Unchanged,
                Vec::new(),
                at(8),
            )
            .await,
    );
}

async fn only_unknown_slots_are_marked_not_posted<S: GuideStore + SettingsStore>(store: S) {
    created(&store, GUIDE, "Lotus", Some(CHANNEL)).await;
    created(&store, OTHER, "Damien", Some(MOVED)).await;
    started(&store, run("run-1", GUIDE, 1, 2), 2).await;
    let slot_0 = posted(CHANNEL, 0, 1, "run-1", 3);
    bound(&store, "run-1", 0, slot_0.clone()).await;
    let slot_1 = unknown(CHANNEL, 1, 2, "run-1", 3);
    settle(
        &store,
        "run-1",
        pending(1, create_op(CHANNEL, 1), Some(source(2)), 3),
        OpOutcome::Unknown,
        SlotEffect::Put(slot_1.clone()),
    )
    .await;
    let mark = |guide: &str, channel: &str, position: u32| {
        store.mark_not_posted(guide.into(), channel.into(), position, record(5))
    };
    assert_eq!(mark(GUIDE, CHANNEL, 1).await.unwrap(), NotPosted::Busy);
    finished(
        &store,
        "run-1",
        PublishState::Stopped,
        Some(StopReason::UnknownSlot),
        4,
    )
    .await;
    assert_eq!(
        mark(GUIDE, CHANNEL, 0).await.unwrap(),
        NotPosted::NotUnknown
    );
    assert_eq!(mark(GUIDE, MOVED, 1).await.unwrap(), NotPosted::Missing);
    assert_eq!(mark(OTHER, CHANNEL, 1).await.unwrap(), NotPosted::Missing);
    let before = keys(&store).await.len();
    assert_eq!(
        mark(GUIDE, CHANNEL, 1).await.unwrap(),
        NotPosted::Removed(slot_1)
    );
    assert_eq!(mark(GUIDE, CHANNEL, 1).await.unwrap(), NotPosted::Missing);
    assert_eq!(store.slots(GUIDE.into()).await.unwrap(), [slot_0]);
    let rows = history(&store).await;
    assert_eq!(rows.len(), before + 1, "only the removal is listed");
    let removal = rows.last().unwrap();
    let row = &removal.values[&format!("slot.{GUIDE}.1")];
    assert_eq!(
        (side(&row.from), row.to.as_str()),
        (
            json!({"channel": CHANNEL, "state": "unknown", "source": source(2)}),
            ""
        )
    );
    assert_eq!(removal.actor, Actor::admin("token"));
}

async fn slots_keep_their_identity_across_a_channel_move<S: GuideStore + SettingsStore>(store: S) {
    created(&store, GUIDE, "Lotus", Some(CHANNEL)).await;
    created(&store, OTHER, "Damien", None).await;
    started(&store, run("run-1", GUIDE, 1, 2), 2).await;
    let old = [
        posted(CHANNEL, 0, 1, "run-1", 3),
        posted(CHANNEL, 1, 2, "run-1", 3),
    ];
    for (ordinal, slot) in (0..).zip(&old) {
        bound(&store, "run-1", ordinal, slot.clone()).await;
    }
    finished(&store, "run-1", PublishState::Done, None, 3).await;
    saved(&store, save(GUIDE, 1, "Lotus", Some(MOVED), &[]), 4).await;

    // The move posts the new set first; the process stops mid-create.
    let mut moving = run("run-2", GUIDE, 2, 5);
    moving.channel_id = MOVED.into();
    moving.confirmed.channel_move = true;
    started(&store, moving, 5).await;
    let new_0 = posted(MOVED, 0, 3, "run-2", 6);
    bound(&store, "run-2", 0, new_0.clone()).await;
    store
        .record_op(
            "run-2".into(),
            pending(1, create_op(MOVED, 1), Some(source(7)), 6),
        )
        .await
        .unwrap();
    queued(&store, run("run-3", OTHER, 1, 7)).await;

    let back = store.interrupted_on_start(at(9)).await.unwrap();
    assert_eq!(back.len(), 1, "only the running run comes back");
    let (running, ops) = &back[0];
    assert_eq!(
        (running.id.as_str(), running.state),
        ("run-2", PublishState::Running)
    );
    assert_eq!(
        ops.iter().map(|op| op.outcome).collect::<Vec<_>>(),
        [OpOutcome::Done, OpOutcome::Pending]
    );
    let (queued_run, _) = store.publish("run-3".into()).await.unwrap().unwrap();
    assert_eq!(
        (
            queued_run.state,
            queued_run.stop_reason,
            queued_run.finished_at
        ),
        (
            PublishState::Stopped,
            Some(StopReason::Interrupted),
            Some(at(9))
        )
    );
    let outcome = history(&store).await.pop().unwrap();
    assert_eq!(outcome.actor, Actor::system("guides"));
    assert!(outcome.values.contains_key("publish.run-3"));

    // Both sets coexist: the old message ids are kept until deleted.
    let mut expected = old.to_vec();
    expected.push(new_0.clone());
    assert_eq!(store.slots(GUIDE.into()).await.unwrap(), expected);
    store
        .apply_op_result(
            "run-2".into(),
            1,
            OpOutcome::Recovered,
            SlotEffect::Put(posted(MOVED, 1, 4, "run-2", 10)),
            Vec::new(),
            at(10),
        )
        .await
        .unwrap();
    finished(
        &store,
        "run-2",
        PublishState::Stopped,
        Some(StopReason::Interrupted),
        10,
    )
    .await;
    assert!(store.interrupted_on_start(at(11)).await.unwrap().is_empty());
    let slots = store.slots(GUIDE.into()).await.unwrap();
    assert_eq!(
        slots
            .iter()
            .map(|slot| (
                slot.channel_id.as_str(),
                slot.position,
                slot.message_id.clone()
            ))
            .collect::<Vec<_>>(),
        [
            (CHANNEL, 0, Some(message(1))),
            (CHANNEL, 1, Some(message(2))),
            (MOVED, 0, Some(message(3))),
            (MOVED, 1, Some(message(4))),
        ]
    );
}

async fn cancels_are_asked_of_live_runs<S: GuideStore>(store: S) {
    created(&store, GUIDE, "Lotus", Some(CHANNEL)).await;
    queued(&store, run("run-1", GUIDE, 1, 2)).await;
    let cancel = |guide: &str, run: &str, hour: u32| {
        store.request_cancel(guide.into(), run.into(), at(hour))
    };
    let CancelRequest::Requested(asked) = cancel(GUIDE, "run-1", 3).await.unwrap() else {
        panic!("a queued run takes a cancel");
    };
    assert_eq!(asked.cancel_requested_at, Some(at(3)));
    // The first request's instant stays.
    assert_eq!(
        cancel(GUIDE, "run-1", 4).await.unwrap(),
        CancelRequest::Requested(asked)
    );
    assert_eq!(
        cancel(OTHER, "run-1", 4).await.unwrap(),
        CancelRequest::Missing
    );
    assert_eq!(
        cancel(GUIDE, "run-9", 4).await.unwrap(),
        CancelRequest::Missing
    );
    let claimed = store.claim_next(at(5)).await.unwrap().unwrap();
    assert_eq!(claimed.cancel_requested_at, Some(at(3)));
    let ended = finished(&store, "run-1", PublishState::Cancelled, None, 6).await;
    assert_eq!(
        cancel(GUIDE, "run-1", 7).await.unwrap(),
        CancelRequest::Finished(ended)
    );
}

async fn deleting_a_guide_drops_its_slots_and_runs<S: GuideStore + SettingsStore>(store: S) {
    created(&store, GUIDE, "Lotus", Some(CHANNEL)).await;
    created(&store, OTHER, "Damien", Some(MOVED)).await;
    started(&store, run("run-1", GUIDE, 1, 2), 2).await;
    let refuse = async |hour: u32, expected: GuideWrite| {
        assert_eq!(
            store
                .apply_guide(delete(GUIDE, 1), record(hour))
                .await
                .unwrap(),
            expected
        );
    };
    // A live run may still post, with or without posted slots.
    refuse(3, GuideWrite::Busy).await;
    let slot = posted(CHANNEL, 0, 1, "run-1", 3);
    bound(&store, "run-1", 0, slot.clone()).await;
    refuse(3, GuideWrite::Busy).await;
    finished(&store, "run-1", PublishState::Done, None, 4).await;
    refuse(4, GuideWrite::Published).await;

    started(&store, run("run-2", GUIDE, 1, 5), 5).await;
    let gone = PublishedSlot {
        state: SlotState::Gone,
        publish_id: "run-2".into(),
        ..slot
    };
    settle(
        &store,
        "run-2",
        pending(
            0,
            PlanOp::Edit {
                position: 0,
                channel_id: CHANNEL.into(),
                message_id: message(1),
                hash: hash('c'),
            },
            None,
            5,
        ),
        OpOutcome::Gone,
        SlotEffect::Put(gone),
    )
    .await;
    finished(
        &store,
        "run-2",
        PublishState::Stopped,
        Some(StopReason::PlanChanged),
        6,
    )
    .await;
    let rows = keys(&store).await.len();
    let other_run = started(&store, run("run-3", OTHER, 1, 7), 7).await;

    // Only gone slots left: the guide goes with its slots, runs and ops.
    let GuideWrite::Deleted(_) = store
        .apply_guide(delete(GUIDE, 1), record(8))
        .await
        .unwrap()
    else {
        panic!("a guide with only gone slots is deleted");
    };
    assert!(store.slots(GUIDE.into()).await.unwrap().is_empty());
    for id in ["run-1", "run-2"] {
        assert_eq!(store.publish(id.into()).await.unwrap(), None);
    }
    assert_eq!(store.latest_publish(GUIDE.into()).await.unwrap(), None);
    assert_eq!(
        store.latest_publish(OTHER.into()).await.unwrap(),
        Some(other_run),
        "other guides keep theirs"
    );
    assert_eq!(
        keys(&store).await.len(),
        rows + 2,
        "the other run's request and the delete; refusals are not listed"
    );
}

async fn keyed_guide_writes_replay_in_the_same_write<
    S: GuideStore + ReplayStore + SettingsStore,
>(
    store: S,
) {
    let replay = |key: &str, request: char, hour: u32| {
        StoredReplay::new(
            ReplayScope::Config,
            "admin:token".into(),
            key.into(),
            std::iter::repeat_n(request, 64).collect(),
            201,
            r#"{"ok":true}"#.into(),
            at(hour),
        )
    };
    let stored = async |key: &str, hour: u32| {
        store
            .replay(ReplayScope::Config, "admin:token", key, at(hour))
            .await
            .unwrap()
    };

    let KeyedWrite::Applied(GuideWrite::Created(guide)) = store
        .apply_guide_replayed(
            create(GUIDE, "Lotus", Some(CHANNEL)),
            record(1),
            replay("k-1", 'a', 1),
        )
        .await
        .unwrap()
    else {
        panic!("a keyed create applies");
    };
    let found = stored("k-1", 2).await.expect("stored with the write");
    assert_eq!(
        store
            .apply_guide_replayed(
                create(GUIDE, "Lotus", Some(CHANNEL)),
                record(2),
                replay("k-1", 'a', 2),
            )
            .await
            .unwrap(),
        KeyedWrite::Replayed(found)
    );
    assert_eq!(
        store
            .apply_guide_replayed(
                create(OTHER, "Damien", None),
                record(2),
                replay("k-1", 'b', 2),
            )
            .await
            .unwrap(),
        KeyedWrite::KeyTaken
    );
    assert_eq!(store.guide(OTHER.into()).await.unwrap(), None);
    // A refusal stores no answer: the key applies later.
    assert_eq!(
        store
            .apply_guide_replayed(
                create(OTHER, "Damien", Some(CHANNEL)),
                record(2),
                replay("k-2", 'c', 2),
            )
            .await
            .unwrap(),
        KeyedWrite::Applied(GuideWrite::ChannelTaken)
    );
    assert_eq!(stored("k-2", 2).await, None);
    assert!(matches!(
        store
            .apply_guide_replayed(
                create(OTHER, "Damien", None),
                record(2),
                replay("k-2", 'd', 2),
            )
            .await
            .unwrap(),
        KeyedWrite::Applied(GuideWrite::Created(_))
    ));

    let first = run("run-1", GUIDE, guide.version, 3);
    assert_eq!(
        store
            .request_publish_replayed(request(first.clone()), record(3), replay("k-3", 'e', 3))
            .await
            .unwrap(),
        KeyedWrite::Applied(PublishRequested::Queued(first.clone()))
    );
    assert!(matches!(
        store
            .request_publish_replayed(request(first.clone()), record(3), replay("k-3", 'e', 3))
            .await
            .unwrap(),
        KeyedWrite::Replayed(_)
    ));
    assert_eq!(
        store
            .request_publish_replayed(
                request(run("run-2", GUIDE, 1, 3)),
                record(3),
                replay("k-4", 'f', 3),
            )
            .await
            .unwrap(),
        KeyedWrite::Applied(PublishRequested::Busy(first))
    );
    assert_eq!(
        stored("k-4", 3).await,
        None,
        "a busy refusal stores nothing"
    );

    let KeyedWrite::Applied(CancelRequest::Requested(asked)) = store
        .request_cancel_replayed(GUIDE.into(), "run-1".into(), replay("k-5", 'a', 4))
        .await
        .unwrap()
    else {
        panic!("a keyed cancel applies");
    };
    assert_eq!(asked.cancel_requested_at, Some(at(4)));
    assert!(matches!(
        store
            .request_cancel_replayed(GUIDE.into(), "run-1".into(), replay("k-5", 'a', 4))
            .await
            .unwrap(),
        KeyedWrite::Replayed(_)
    ));

    store.claim_next(at(5)).await.unwrap();
    let slot = unknown(CHANNEL, 0, 1, "run-1", 5);
    settle(
        &store,
        "run-1",
        pending(0, create_op(CHANNEL, 0), Some(source(1)), 5),
        OpOutcome::Unknown,
        SlotEffect::Put(slot.clone()),
    )
    .await;
    finished(&store, "run-1", PublishState::Cancelled, None, 5).await;
    let mark = |request: char| {
        store.mark_not_posted_replayed(
            GUIDE.into(),
            CHANNEL.into(),
            0,
            record(6),
            replay("k-6", request, 6),
        )
    };
    assert_eq!(
        mark('b').await.unwrap(),
        KeyedWrite::Applied(NotPosted::Removed(slot))
    );
    assert!(matches!(mark('b').await.unwrap(), KeyedWrite::Replayed(_)));
    assert_eq!(mark('c').await.unwrap(), KeyedWrite::KeyTaken);
    assert_eq!(
        keys(&store).await,
        [
            format!("guide.{GUIDE}"),
            format!("guide.{OTHER}"),
            "publish.run-1".into(),
            "publish.run-1".into(),
            format!("slot.{GUIDE}.0"),
        ],
        "replays, key clashes and refusals record nothing"
    );
}
