//! `/debug ping` and `/debug clear_test` through the real journal and the
//! fake Discord (v4 `DebugGroup`): each kind posts the real message prefixed
//! `🧪 TEST — `, bound and registered for the run; reactions on it drive the
//! run's RSVPs; the run's reminder rows are untouched; clear_test deletes and
//! releases the channel's test cards.

use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use chrono::TimeDelta;
use kanade::bot::commands::{DebugCards, TestKind, TestPosted};
use kanade::bot::delivery::cards::{CardKit, REACT_HINT};
use kanade::bot::delivery::{
    AlertThrottle, CardRefresh, DebugCardStore, DebugDesk, SendOutcome, StoreRef, TEST_PREFIX,
};
use kanade::bot::events::{ReactionRouter, RsvpAnswer, RsvpReaction};
use kanade::bot::transport::{Call, FakeDiscord, Op, Outcome, RejectionKind, Step};
use kanade::domain::ids::{RandomIds, short_id};
use kanade::domain::notify::DeliveryJournal;
use kanade::domain::schedule::{RsvpState, RunStatus};
use kanade::domain::scheduler::SchedulerService;
use kanade::infrastructure::store::MemoryScheduleStore;
use twilight_model::id::Id;

use crate::cards::{
    STAR_COLOUR, allowed, art_dir, created, due, edits, fields, kit, pictures, run, tonight,
    uploads, world,
};
use crate::intercept::Intercept;
use crate::scenarios::{self, HOME, POST, World, now};
use crate::support::{self, Store};

pub(crate) fn desk<S: Store + DebugCardStore + 'static>(
    store: &Arc<S>,
    fake: &Arc<FakeDiscord>,
    world: &World,
    cards: CardKit,
    channels: &[&str],
    post: Option<&str>,
) -> DebugDesk<S, FakeDiscord> {
    let channels: BTreeSet<String> = channels.iter().map(|id| (*id).to_owned()).collect();
    DebugDesk {
        store: Arc::clone(store),
        transport: Arc::clone(fake),
        members: Arc::new(world.roster.clone()),
        channels: Arc::new(channels),
        cards,
        policy: scenarios::config().policy,
        quiet: Arc::new(AtomicBool::new(false)),
        post_channel: Arc::new(RwLock::new(post.map(str::to_owned))),
        test_channel: None,
        instance_id: support::INSTANCE.into(),
        now: Arc::new(now),
        throttle: AlertThrottle::new(),
    }
}

/// Run `$f(Arc<store>)` on a fresh memory store, then a fresh SQLite store.
macro_rules! on_both_arcs {
    ($f:path) => {{
        $f(Arc::new(
            kanade::infrastructure::store::MemoryScheduleStore::new(),
        ))
        .await;
        let dir = $crate::support::TempDir::new();
        let sqlite = Arc::new(dir.open().await);
        $f(Arc::clone(&sqlite)).await;
        Arc::try_unwrap(sqlite)
            .ok()
            .expect("sole owner")
            .close()
            .await
            .expect("close store");
    }};
}
pub(crate) use on_both_arcs;

async fn ping<S: Store + DebugCardStore + 'static>(
    desk: &DebugDesk<S, FakeDiscord>,
    run: &str,
    kind: TestKind,
    by: &str,
) -> TestPosted {
    desk.post(run.into(), kind, by.into())
        .await
        .expect("test post")
}

fn content(fake: &FakeDiscord) -> String {
    created(fake)
        .pop()
        .and_then(|message| message.content)
        .unwrap_or_default()
}

/// The Malefic Star run tonight (1001 pings at `all`, 1002 never) with its
/// real day-of reminder due.
pub(crate) async fn star<S: Store>(store: &S) -> String {
    let id = run(
        store,
        &["HMaleficStar"],
        &["1001", "1002"],
        tonight(),
        RunStatus::Planned,
    )
    .await;
    due(store, &id, "day_of").await;
    id
}

async fn every_kind_posts_the_real_message_prefixed<S: Store + DebugCardStore + 'static>(
    store: Arc<S>,
) {
    let art = art_dir();
    let world = world();
    let fake = Arc::new(support::fake());
    let run_id = star(&*store).await;
    let reminders = support::snapshot(&*store).await.reminders;
    let desk = desk(
        &store,
        &fake,
        &world,
        kit(Some(&art)),
        &[HOME, POST],
        Some(POST),
    );
    let posted = TestPosted::Posted {
        channel_id: HOME.into(),
    };

    assert_eq!(ping(&desk, &run_id, TestKind::DayOf, "1002").await, posted);
    let day_of = created(&fake).pop().expect("day-of");
    assert_eq!(
        day_of.content.as_deref(),
        Some(format!("{TEST_PREFIX}📅 **Today — Thu 10 Sep**\n<@1001> Bex").as_str()),
        "v4's heading (no rewrite), the test audience"
    );
    let embed = &day_of.embeds[0];
    assert_eq!(
        fields(embed),
        [(
            "🕘 21:00  ·  HMaleficStar".to_owned(),
            "**HMaleficStar** · Radiant Malefic Star (Hard, Lv280)\n⚠️ unconfirmed · 0/2 ✅\nStill to answer: <@1001> Bex"
                .to_owned()
        )]
    );
    assert_eq!(embed.color, Some(STAR_COLOUR));
    assert_eq!(
        pictures(embed),
        (
            Some("attachment://MaleficStar.png".to_owned()),
            Some("attachment://image-MaleficStar.png".to_owned())
        )
    );
    assert_eq!(uploads(&day_of).len(), 2);
    assert_eq!(allowed(&day_of), ["1001"], "test: only ping level `all`");
    let reactions = fake
        .calls()
        .iter()
        .filter(|call| matches!(call, Call::AddReaction { .. }))
        .count();
    assert_eq!(reactions, 2, "✅ and ❌");

    assert_eq!(
        ping(&desk, &run_id, TestKind::Countdown60, "1002").await,
        posted
    );
    assert_eq!(
        content(&fake),
        format!("{TEST_PREFIX}⏰ **HMaleficStar** in 1h (21:00) — <@1001> Bex")
    );
    assert_eq!(
        ping(&desk, &run_id, TestKind::Countdown15, "1002").await,
        posted
    );
    let countdown = created(&fake).pop().expect("countdown");
    assert!(
        countdown.content.as_deref().is_some_and(
            |text| text.starts_with(&format!("{TEST_PREFIX}⏰ **HMaleficStar** in 15m"))
        ),
        "{countdown:?}"
    );
    assert_eq!(pictures(&countdown.embeds[0]).1, None, "no entry art");

    assert_eq!(ping(&desk, &run_id, TestKind::Amend, "1002").await, posted);
    let amend = created(&fake).pop().expect("amend");
    assert_eq!(
        amend.content.as_deref(),
        Some(
            format!(
                "{TEST_PREFIX}🔁 **HMaleficStar** moved: ~~Wed 09 Sep 21:00~~ → **Thu 10 Sep 21:00** \
                 — <@1001> Bex\n{REACT_HINT}"
            )
            .as_str()
        )
    );
    assert!(amend.embeds.is_empty() && amend.attachments.is_empty());
    assert_eq!(
        ping(&desk, &run_id, TestKind::Decline, "1002").await,
        posted
    );
    assert_eq!(
        content(&fake),
        format!(
            "{TEST_PREFIX}<@1001> Bex can't make **HMaleficStar** (Thu 10 Sep 21:00) — \
             reschedule? `/amend run_id:{} to:...`",
            short_id(&run_id)
        )
    );

    // Every ping is its own post (v4 operation dedupe), bound and listed.
    assert_eq!(ping(&desk, &run_id, TestKind::DayOf, "1002").await, posted);
    assert_eq!(created(&fake).len(), 6);
    let listed = store
        .debug_cards_in(HOME, now() - TimeDelta::hours(1))
        .await
        .expect("list");
    assert_eq!(
        listed
            .iter()
            .map(|card| card.kind.as_str())
            .collect::<Vec<_>>(),
        [
            "day_of",
            "countdown_60",
            "countdown_15",
            "amend",
            "decline",
            "day_of"
        ]
    );
    assert_eq!(
        support::snapshot(&*store).await.reminders,
        reminders,
        "the scheduled reminders are untouched"
    );
}

#[tokio::test]
async fn ping_posts_each_kind_prefixed_bound_and_with_art() {
    on_both_arcs!(every_kind_posts_the_real_message_prefixed);
}

async fn reactions_and_refresh_follow_the_run<S: Store + DebugCardStore + 'static>(store: Arc<S>) {
    let world = world();
    let fake = Arc::new(support::fake());
    let run_id = star(&*store).await;
    let desk = desk(&store, &fake, &world, kit(None), &[HOME], None);
    ping(&desk, &run_id, TestKind::DayOf, "1001").await;
    ping(&desk, &run_id, TestKind::Amend, "1001").await;
    let messages: Vec<u64> = fake
        .calls()
        .iter()
        .filter_map(|call| match call {
            Call::Create {
                outcome: Outcome::Delivered(id),
                ..
            } => Some(id.get()),
            _ => None,
        })
        .collect();
    // ✅ on the amend test message: the run's RSVP, like any card.
    let sink = SchedulerService::new(StoreRef(&*store), RandomIds, scenarios_clock());
    let mut router = ReactionRouter::new(StoreRef(&*store), sink);
    let results = router
        .route(&RsvpReaction {
            channel_id: Id::new(222),
            message_id: Id::new(messages[1]),
            user_id: Id::new(1002),
            answer: RsvpAnswer::Yes,
            added: true,
            display_name: "1002".into(),
        })
        .await
        .expect("route");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].run_id, run_id);
    assert_eq!(results[0].state, Some(RsvpState::Yes));
    // Someone not on the run reacting changes nothing.
    let rsvps = support::snapshot(&*store).await.rsvps;
    let stranger = router
        .route(&RsvpReaction {
            channel_id: Id::new(222),
            message_id: Id::new(messages[0]),
            user_id: Id::new(1009),
            answer: RsvpAnswer::No,
            added: true,
            display_name: "1009".into(),
        })
        .await
        .expect("route");
    assert!(
        stranger.iter().all(|result| !result.applied),
        "{stranger:?}"
    );
    assert_eq!(support::snapshot(&*store).await.rsvps, rsvps);
    // The day-of test card is re-rendered with its prefix; the amend text is not.
    let refresh = CardRefresh {
        store: Arc::clone(&store),
        transport: Arc::clone(&fake),
        members: Arc::new(world.roster.clone()),
        cards: kit(None),
        policy: scenarios::config().policy,
        quiet: Arc::new(AtomicBool::new(false)),
        now: Arc::new(now),
    };
    refresh.refresh(std::slice::from_ref(&run_id)).await;
    let edited = edits(&fake);
    let test_edits: Vec<_> = fake
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Edit { message, edit, .. } => Some((message.get(), edit)),
            _ => None,
        })
        .filter(|(message, _)| messages.contains(message))
        .collect();
    assert_eq!(test_edits.len(), 1, "{edited:?}");
    assert_eq!(test_edits[0].0, messages[0]);
    let edit = &test_edits[0].1;
    assert_eq!(
        edit.content.as_deref(),
        Some(format!("{TEST_PREFIX}📅 **Today — Thu 10 Sep**\n<@1001> Bex").as_str())
    );
    assert!(
        edit.embeds.as_ref().expect("embed")[0].fields[0]
            .value
            .contains("⚠️ unconfirmed · 1/2 ✅")
    );
}

fn scenarios_clock() -> kanade::bot::delivery::FixedClock {
    kanade::bot::delivery::FixedClock(now())
}

#[tokio::test]
async fn reactions_on_test_cards_drive_rsvps_and_card_tests_refresh() {
    on_both_arcs!(reactions_and_refresh_follow_the_run);
}

async fn clear_deletes_and_releases<S: Store + DebugCardStore + 'static>(store: Arc<S>) {
    let world = world();
    let fake = Arc::new(support::fake());
    let run_id = star(&*store).await;
    let desk = desk(&store, &fake, &world, kit(None), &[HOME, POST], Some(POST));
    for kind in [TestKind::DayOf, TestKind::Countdown15, TestKind::Amend] {
        ping(&desk, &run_id, kind, "1001").await;
    }
    let since = now() - TimeDelta::hours(24);
    assert_eq!(
        desk.clear(POST.into(), since).await,
        Ok((0, 0)),
        "per channel"
    );
    // One refused delete, one already gone, one deleted now.
    fake.script(Op::Delete, Step::Reject(RejectionKind::MissingPermissions));
    fake.script(Op::Delete, Step::Reject(RejectionKind::UnknownMessage));
    assert_eq!(desk.clear(HOME.into(), since).await, Ok((2, 1)));
    assert_eq!(
        store.debug_cards_in(HOME, since).await.expect("list").len(),
        1,
        "the refused one stays listed"
    );
    assert_eq!(desk.clear(HOME.into(), since).await, Ok((1, 0)));
    assert!(
        store
            .debug_cards_in(HOME, since)
            .await
            .expect("list")
            .is_empty()
    );
    assert!(
        store
            .posted_cards(&run_id)
            .await
            .expect("posted")
            .iter()
            .all(|card| !card.test),
        "cleared test cards are no longer refreshed"
    );
}

#[tokio::test]
async fn clear_test_deletes_the_channels_test_cards_and_releases_them() {
    on_both_arcs!(clear_deletes_and_releases);
}

#[tokio::test]
async fn an_unreachable_home_falls_back_to_the_post_channel_or_refuses() {
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let fake = Arc::new(support::fake());
    let run_id = star(&*store).await;
    let fallback = desk(&store, &fake, &world, kit(None), &[POST], Some(POST));
    assert_eq!(
        ping(&fallback, &run_id, TestKind::Countdown60, "1001").await,
        TestPosted::Posted {
            channel_id: POST.into()
        }
    );
    let nowhere = desk(&store, &fake, &world, kit(None), &[POST], None);
    assert_eq!(
        ping(&nowhere, &run_id, TestKind::DayOf, "1001").await,
        TestPosted::Unreachable
    );
    assert_eq!(created(&fake).len(), 1, "nothing posted when unreachable");
    // A refused post is reported, not bound.
    fake.script(Op::Create, Step::Reject(RejectionKind::MissingPermissions));
    assert_eq!(
        ping(&fallback, &run_id, TestKind::DayOf, "1001").await,
        TestPosted::Unconfirmed
    );
}

async fn the_real_reminder_still_posts<S: Store + DebugCardStore + 'static>(store: Arc<S>) {
    let world = world();
    let fake = Arc::new(support::fake());
    let run_id = star(&*store).await;
    let desk = desk(&store, &fake, &world, kit(None), &[HOME], None);
    ping(&desk, &run_id, TestKind::DayOf, "1001").await;
    let mut delivery = scenarios::delivery(&*store, &world, &*fake);
    let report = delivery.dispatch_reminders(now()).await.expect("dispatch");
    assert!(
        matches!(report.sends.as_slice(), [send] if matches!(send.outcome, SendOutcome::Bound(_))),
        "the real day-of is claimed fresh and posted: {report:?}"
    );
    let posts = created(&fake);
    assert_eq!(posts.len(), 2);
    assert!(
        !posts[1]
            .content
            .as_deref()
            .unwrap_or_default()
            .starts_with(TEST_PREFIX)
    );
}

#[tokio::test]
async fn a_bound_test_card_never_holds_the_runs_real_reminder() {
    on_both_arcs!(the_real_reminder_still_posts);
}

#[tokio::test]
async fn quiet_plain_test_texts_carry_v4s_quiet_line() {
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let fake = Arc::new(support::fake());
    let run_id = star(&*store).await;
    let quiet = DebugDesk {
        quiet: Arc::new(AtomicBool::new(true)),
        ..desk(&store, &fake, &world, kit(None), &[HOME], None)
    };
    let note = "\n_🔕 quiet mode - nobody was notified_";
    ping(&quiet, &run_id, TestKind::Amend, "1002").await;
    let amend = created(&fake).pop().expect("amend");
    assert_eq!(
        amend.content.as_deref(),
        Some(
            format!(
                "{TEST_PREFIX}🔁 **HMaleficStar** moved: ~~Wed 09 Sep 21:00~~ → **Thu 10 Sep 21:00** \
                 — Aria Bex\n{REACT_HINT}{note}"
            )
            .as_str()
        )
    );
    assert!(allowed(&amend).is_empty());
    ping(&quiet, &run_id, TestKind::Decline, "1002").await;
    assert_eq!(
        content(&fake),
        format!(
            "{TEST_PREFIX}Aria Bex can't make **HMaleficStar** (Thu 10 Sep 21:00) — \
             reschedule? `/amend run_id:{} to:...`{note}",
            short_id(&run_id)
        )
    );
    // Cards announce nothing (v5 rule for embeds).
    ping(&quiet, &run_id, TestKind::DayOf, "1002").await;
    assert!(!content(&fake).contains("quiet mode"));
}

/// Discord accepted the post, then the journal could not record it: the
/// reply says to check the channel, never a generic failure.
#[tokio::test]
async fn a_post_the_journal_could_not_record_is_unconfirmed() {
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let fake: &'static FakeDiscord = Box::leak(Box::new(support::fake()));
    let run_id = star(&*store).await;
    let orphaning = Arc::clone(&store);
    let transport = Intercept::new(fake).on_create(move |outcome| {
        let store = Arc::clone(&orphaning);
        Box::pin(async move {
            // A restart's recovery in between: the lease is gone.
            store.recover_on_start(now()).await.expect("recover");
            outcome
        })
    });
    let base = desk(
        &store,
        &Arc::new(support::fake()),
        &world,
        kit(None),
        &[HOME],
        None,
    );
    let desk = DebugDesk {
        store: base.store,
        transport: Arc::new(transport),
        members: base.members,
        channels: base.channels,
        cards: base.cards,
        policy: base.policy,
        quiet: base.quiet,
        post_channel: base.post_channel,
        test_channel: base.test_channel,
        instance_id: base.instance_id,
        now: base.now,
        throttle: base.throttle,
    };
    assert_eq!(
        desk.post(run_id, TestKind::DayOf, "1001".into()).await,
        Ok(TestPosted::Unconfirmed)
    );
    assert_eq!(created(fake).len(), 1, "it was posted");
}
