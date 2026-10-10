//! `/debug ping`'s channel, style, header, digest and sample options and
//! `/debug header`, through the real journal and the fake Discord: a test
//! card outside the run's home channel is display only, samples and header
//! trials store nothing, and overrides never touch settings or records.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::TimeDelta;
use kanade::bot::commands::{
    DebugCards, HeaderNote, HeaderRequest, HeaderTrialKind, HeaderTrials, PingRequest, SampleRun,
    TestKind, TestPosted, TestReport, TestSubject,
};
use kanade::bot::delivery::cards::{CardKit, DigestPhraseStore, HeadingRewrite, ReminderCardStore};
use kanade::bot::delivery::{
    CardRefresh, DebugCardStore, DebugDesk, FixedClock, SAMPLE_RUN_ID, StoreRef, TEST_PREFIX,
};
use kanade::bot::events::{ReactionRouter, RsvpAnswer, RsvpReaction};
use kanade::bot::transport::{Call, FakeDiscord, Outcome};
use kanade::chat::nudge::{NudgeRewriter, RewriteFailure, RewritePrompt, SharedRewriter};
use kanade::domain::ids::RandomIds;
use kanade::domain::notify::{ActiveClaims, DedupeKey, DeliveryJournal, DeliveryTarget};
use kanade::domain::schedule::{RunStatus, ScheduleSnapshot};
use kanade::domain::scheduler::SchedulerService;
use kanade::domain::settings::MessageStyle;
use kanade::infrastructure::store::MemoryScheduleStore;
use twilight_model::id::Id;

use crate::cards::{Script, Scripted, allowed, created, kit, persona, rewriting, tonight, world};
use crate::debug::{desk, on_both_arcs, star};
use crate::scenarios::{self, HOME, POST, now, week};
use crate::support::{self, Store, seed_digest};

/// A reachable channel that is no run's home.
const SANDBOX: &str = "777";

fn ping(subject: TestSubject, kind: TestKind) -> PingRequest {
    PingRequest {
        subject,
        ..PingRequest::run(String::new(), kind, "1002".into())
    }
}

fn reactions(fake: &FakeDiscord) -> usize {
    fake.calls()
        .iter()
        .filter(|call| matches!(call, Call::AddReaction { .. }))
        .count()
}

fn posted_ids(fake: &FakeDiscord) -> Vec<u64> {
    fake.calls()
        .iter()
        .filter_map(|call| match call {
            Call::Create {
                outcome: Outcome::Delivered(id),
                ..
            } => Some(id.get()),
            _ => None,
        })
        .collect()
}

fn content(fake: &FakeDiscord) -> String {
    created(fake)
        .pop()
        .and_then(|message| message.content)
        .unwrap_or_default()
}

/// The schedule rows, ignoring the journal's revision bumps.
async fn rows(store: &impl Store) -> ScheduleSnapshot {
    ScheduleSnapshot {
        revision: 0,
        ..support::snapshot(store).await
    }
}

fn record_key(target: DeliveryTarget) -> String {
    DedupeKey::native(&[target])
        .expect("key")
        .as_str()
        .to_owned()
}

async fn sandbox_posts_are_display_only<S: Store + DebugCardStore + 'static>(store: Arc<S>) {
    let world = world();
    let fake = Arc::new(support::fake());
    let run_id = star(&*store).await;
    let desk = desk(
        &store,
        &fake,
        &world,
        kit(None),
        &[HOME, POST, SANDBOX],
        Some(POST),
    );
    let elsewhere = PingRequest {
        channel: Some(SANDBOX.into()),
        ..PingRequest::run(run_id.clone(), TestKind::DayOf, "1002".into())
    };
    assert_eq!(
        desk.ping(elsewhere).await,
        Ok(TestReport {
            posted: TestPosted::Sandboxed {
                channel_id: SANDBOX.into()
            },
            header: None,
        })
    );
    let card = created(&fake).pop().expect("sandbox card");
    assert_eq!(
        card.content.as_deref(),
        Some(format!("{TEST_PREFIX}📅 **Today — Thu 10 Sep**\n<@1001> Bex").as_str()),
        "the real card's look"
    );
    assert!(allowed(&card).is_empty(), "pings nobody");
    assert_eq!(reactions(&fake), 0, "no ✅/❌ seeded");

    // Naming the home channel is the registered ping, unchanged.
    let home = PingRequest {
        channel: Some(HOME.into()),
        ..PingRequest::run(run_id.clone(), TestKind::DayOf, "1002".into())
    };
    assert_eq!(
        desk.ping(home).await.expect("home").posted,
        TestPosted::Posted {
            channel_id: HOME.into()
        }
    );
    assert_eq!(reactions(&fake), 2);
    let ids = posted_ids(&fake);

    // ✅ on the sandbox card changes no RSVP.
    let before = support::snapshot(&*store).await.rsvps;
    let sink = SchedulerService::new(StoreRef(&*store), RandomIds, FixedClock(now()));
    let mut router = ReactionRouter::new(StoreRef(&*store), sink);
    let routed = router
        .route(&RsvpReaction {
            channel_id: Id::new(777),
            message_id: Id::new(ids[0]),
            user_id: Id::new(1002),
            answer: RsvpAnswer::Yes,
            added: true,
            display_name: "1002".into(),
        })
        .await
        .expect("route");
    assert!(routed.is_empty(), "{routed:?}");
    assert_eq!(support::snapshot(&*store).await.rsvps, before);
    assert!(
        store
            .runs_for_message(Id::new(ids[0]))
            .await
            .expect("index")
            .is_empty()
    );

    // Only the home card is refreshed.
    let refresh = CardRefresh {
        store: Arc::clone(&store),
        transport: Arc::clone(&fake),
        members: Arc::new(world.roster.clone()),
        cards: kit(None),
        policy: scenarios::config().policy,
        quiet: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        now: Arc::new(now),
    };
    refresh.refresh(std::slice::from_ref(&run_id)).await;
    let edited: Vec<u64> = fake
        .calls()
        .iter()
        .filter_map(|call| match call {
            Call::Edit { message, .. } => Some(message.get()),
            _ => None,
        })
        .collect();
    assert!(edited.contains(&ids[1]), "{edited:?}");
    assert!(!edited.contains(&ids[0]), "{edited:?}");

    // clear_test still finds it.
    let since = now() - TimeDelta::hours(1);
    let listed = store.debug_cards_in(SANDBOX, since).await.expect("list");
    assert_eq!(
        listed
            .iter()
            .map(|card| (card.run_id.as_str(), card.kind.as_str()))
            .collect::<Vec<_>>(),
        [(run_id.as_str(), "sandbox_day_of")]
    );
    assert_eq!(desk.clear(SANDBOX.into(), since).await, Ok((1, 0)));

    let nowhere = PingRequest {
        channel: Some("999".into()),
        ..PingRequest::run(run_id, TestKind::DayOf, "1002".into())
    };
    assert_eq!(
        desk.ping(nowhere).await.expect("ping").posted,
        TestPosted::Unreachable
    );
}

#[tokio::test]
async fn a_test_card_outside_the_home_channel_is_display_only() {
    on_both_arcs!(sandbox_posts_are_display_only);
}

#[tokio::test]
async fn the_test_channel_is_the_default_target() {
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let fake = Arc::new(support::fake());
    let run_id = star(&*store).await;
    let base = || desk(&store, &fake, &world, kit(None), &[HOME, SANDBOX], None);
    let run = || PingRequest::run(run_id.clone(), TestKind::Countdown60, "1001".into());
    let testing = DebugDesk {
        test_channel: Some(SANDBOX.into()),
        ..base()
    };
    assert_eq!(
        testing.ping(run()).await.expect("ping").posted,
        TestPosted::Sandboxed {
            channel_id: SANDBOX.into()
        }
    );
    let named = PingRequest {
        channel: Some(HOME.into()),
        ..run()
    };
    assert_eq!(
        testing.ping(named).await.expect("ping").posted,
        TestPosted::Posted {
            channel_id: HOME.into()
        },
        "`channel:` wins"
    );
    let at_home = DebugDesk {
        test_channel: Some(HOME.into()),
        ..base()
    };
    assert_eq!(
        at_home.ping(run()).await.expect("ping").posted,
        TestPosted::Posted {
            channel_id: HOME.into()
        },
        "the home channel as test channel registers"
    );
    let gone = DebugDesk {
        test_channel: Some("999".into()),
        ..base()
    };
    assert_eq!(
        gone.ping(run()).await.expect("ping").posted,
        TestPosted::Unreachable
    );
}

#[tokio::test(start_paused = true)]
async fn style_and_header_overrides_touch_no_setting_or_record() {
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let fake = Arc::new(support::fake());
    let run_id = star(&*store).await;
    let reminder = support::snapshot(&*store).await.reminders[0].id.clone();
    let day_of_key = record_key(DeliveryTarget::Reminder(reminder));
    let shine = Scripted::new(Script::Reply("Rise and shine, it's {day}!"));
    let cards = CardKit {
        style: Some(Arc::new(|| MessageStyle::Classic)),
        ..rewriting(&shine)
    };
    let desk = desk(&store, &fake, &world, cards.clone(), &[HOME], None);
    let run = |kind| PingRequest::run(run_id.clone(), kind, "1002".into());

    let report = desk
        .ping(PingRequest {
            style: Some(MessageStyle::Redesigned),
            rewrite: true,
            ..run(TestKind::DayOf)
        })
        .await
        .expect("ping");
    assert_eq!(
        report,
        TestReport {
            posted: TestPosted::Posted {
                channel_id: HOME.into()
            },
            header: Some(HeaderNote {
                rewritten: true,
                reason: "accepted".into()
            }),
        }
    );
    let redesigned = content(&fake);
    assert!(
        redesigned.contains("Rise and shine, it's Thu 10 Sep!"),
        "{redesigned}"
    );
    assert!(redesigned.contains("\n-# "), "redesigned: {redesigned}");
    assert_eq!(
        cards.style(),
        MessageStyle::Classic,
        "the setting is untouched"
    );
    assert_eq!(store.card_record(&day_of_key).await.expect("read"), None);

    // Without overrides: the live style and the seed, no model call.
    desk.ping(run(TestKind::DayOf)).await.expect("ping");
    assert_eq!(
        content(&fake),
        format!("{TEST_PREFIX}📅 **Today — Thu 10 Sep**\n<@1001> Bex")
    );
    assert_eq!(shine.calls(), 1);

    // Countdown and digest phrases: only the redesigned style shows them.
    let waku = Scripted::new(Script::Reply("Waku waku!"));
    let phrases = desk_with(&store, &fake, &world, rewriting(&waku));
    phrases
        .ping(PingRequest {
            style: Some(MessageStyle::Redesigned),
            rewrite: true,
            ..run(TestKind::Countdown60)
        })
        .await
        .expect("ping");
    assert!(
        content(&fake).contains(":R> · Waku waku! "),
        "{}",
        content(&fake)
    );
    phrases
        .ping(PingRequest {
            style: Some(MessageStyle::Redesigned),
            rewrite: true,
            ..run(TestKind::Digest)
        })
        .await
        .expect("ping");
    // The redesigned digest pings nobody: its test copy is V2 too, labelled
    // as a test above the header.
    let digest = created(&fake).pop().expect("a digest post");
    assert_eq!((digest.content.as_deref(), digest.embeds.len()), (None, 0));
    let texts = support::v2_texts(&digest.components);
    assert_eq!(texts[0], "-# 🧪 TEST", "{texts:?}");
    assert_eq!(
        texts[1].lines().nth(1),
        Some("Waku waku!"),
        "the phrase under the title: {texts:?}"
    );
    assert_eq!(
        store
            .digest_phrase(&record_key(DeliveryTarget::Digest(week())))
            .await
            .expect("read"),
        None
    );
    let amend = phrases
        .ping(PingRequest {
            rewrite: true,
            ..run(TestKind::Amend)
        })
        .await
        .expect("ping");
    assert_eq!(amend.header, None, "amend has no header");
    assert_eq!(waku.calls(), 2);

    // Any failure falls back to the seed and says why.
    for (script, reason) in [
        (Script::Reply("**Wake up**, {day}"), "rejected (markup)"),
        (Script::Fail, "unavailable"),
        (Script::Hang, "timeout"),
    ] {
        let rewriter = Scripted::new(script);
        let failing = desk_with(&store, &fake, &world, rewriting(&rewriter));
        let report = failing
            .ping(PingRequest {
                rewrite: true,
                ..run(TestKind::DayOf)
            })
            .await
            .expect("ping");
        assert_eq!(
            report.header,
            Some(HeaderNote {
                rewritten: false,
                reason: reason.into()
            })
        );
        assert_eq!(
            content(&fake),
            format!("{TEST_PREFIX}📅 **Today — Thu 10 Sep**\n<@1001> Bex")
        );
    }
    assert_eq!(store.card_record(&day_of_key).await.expect("read"), None);
}

fn desk_with(
    store: &Arc<MemoryScheduleStore>,
    fake: &Arc<FakeDiscord>,
    world: &scenarios::World,
    cards: CardKit,
) -> DebugDesk<MemoryScheduleStore, FakeDiscord> {
    desk(store, fake, world, cards, &[HOME], None)
}

async fn the_week_digest_is_a_test_copy<S: Store + DebugCardStore + 'static>(store: Arc<S>) {
    let world = world();
    let fake = Arc::new(support::fake());
    star(&*store).await;
    seed_digest(&*store, week(), POST, "9001", now()).await;
    let digests = store.load_digests().await.expect("digests");
    let view = store.load_view().await.expect("view");
    let desk = desk(&store, &fake, &world, kit(None), &[HOME, POST], Some(POST));
    assert_eq!(
        desk.ping(ping(TestSubject::Week, TestKind::Digest)).await,
        Ok(TestReport {
            posted: TestPosted::Sandboxed {
                channel_id: POST.into()
            },
            header: None,
        }),
        "the real digest's channel"
    );
    let card = created(&fake).pop().expect("digest");
    assert_eq!(
        card.content.as_deref(),
        Some(format!("{TEST_PREFIX}🗓️ Boss week of Wed 09 Sep").as_str())
    );
    let embed = &card.embeds[0];
    assert!(
        embed
            .description
            .as_deref()
            .is_some_and(|text| text.starts_with("**0/1 Cleared**")),
        "{embed:?}"
    );
    assert!(allowed(&card).is_empty());
    assert_eq!(reactions(&fake), 0);
    assert!(
        !fake
            .calls()
            .iter()
            .any(|call| matches!(call, Call::Delete { .. })),
        "the real digest is never replaced"
    );
    assert_eq!(store.load_digests().await.expect("digests"), digests);
    assert_eq!(store.load_view().await.expect("view"), view);
    let listed = store
        .debug_cards_in(POST, now() - TimeDelta::hours(1))
        .await
        .expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, "sandbox_digest");
    assert!(listed[0].run_id.starts_with("week:"), "{listed:?}");
}

#[tokio::test]
async fn the_digest_kind_posts_a_test_copy_and_leaves_the_real_digest() {
    on_both_arcs!(the_week_digest_is_a_test_copy);
}

async fn samples_store_nothing<S: Store + DebugCardStore + 'static>(store: Arc<S>) {
    let world = world();
    let fake = Arc::new(support::fake());
    star(&*store).await;
    let before = rows(&*store).await;
    let view = store.load_view().await.expect("view");
    let desk = desk(&store, &fake, &world, kit(None), &[HOME, SANDBOX], None);
    let sample = SampleRun {
        bosses: vec!["HMaleficStar".into()],
        at: tonight(),
        party: vec!["1001".into(), "1002".into()],
        yes: vec!["1001".into()],
        no: vec!["1003".into()],
        status: RunStatus::Confirmed,
    };
    for kind in TestKind::ALL {
        let request = PingRequest {
            invoked_in: Some(SANDBOX.into()),
            ..ping(TestSubject::Sample(sample.clone()), kind)
        };
        assert_eq!(
            desk.ping(request).await,
            Ok(TestReport {
                posted: TestPosted::Sandboxed {
                    channel_id: SANDBOX.into()
                },
                header: None,
            }),
            "{kind:?}"
        );
    }
    let posts = created(&fake);
    assert_eq!(posts.len(), TestKind::ALL.len());
    let day_of = &posts[0];
    assert_eq!(
        day_of.content.as_deref(),
        Some(format!("{TEST_PREFIX}📅 **Today — Thu 10 Sep**\n<@1001> Bex <@1003>").as_str())
    );
    let field = &day_of.embeds[0].fields[0];
    assert_eq!(field.name, "🕘 21:00  ·  HMaleficStar");
    assert!(
        field.value.contains("✅ confirmed · 1/3 ✅ · 1 ❌"),
        "{}",
        field.value
    );
    assert!(
        posts[1]
            .content
            .as_deref()
            .is_some_and(|text| text.contains("**HMaleficStar** in 1h")),
        "{:?}",
        posts[1].content
    );
    assert!(
        posts[5].embeds[0]
            .description
            .as_deref()
            .is_some_and(|text| text.starts_with("**0/1 Cleared** · 1 run(s) across 1 day(s)")),
        "{:?}",
        posts[5].embeds
    );
    assert!(posts.iter().all(|post| allowed(post).is_empty()));
    assert_eq!(reactions(&fake), 0);

    // Nothing of it reached the schedule, the RSVPs, the reminders, the
    // card index or the refresh list.
    assert_eq!(rows(&*store).await, before);
    assert_eq!(store.load_view().await.expect("view"), view);
    assert_eq!(view, ActiveClaims::default());
    assert!(
        store
            .posted_cards(SAMPLE_RUN_ID)
            .await
            .expect("posted")
            .is_empty()
    );
    for id in posted_ids(&fake) {
        assert!(
            store
                .runs_for_message(Id::new(id))
                .await
                .expect("index")
                .is_empty()
        );
    }
    // clear_test removes them all.
    assert_eq!(
        desk.clear(SANDBOX.into(), now() - TimeDelta::hours(24))
            .await,
        Ok((TestKind::ALL.len(), 0))
    );

    // No `channel:` and no reachable invoking channel: nowhere to post.
    let lost = desk
        .ping(ping(TestSubject::Sample(sample), TestKind::DayOf))
        .await
        .expect("ping");
    assert_eq!(lost.posted, TestPosted::Unreachable);
}

#[tokio::test]
async fn sample_runs_render_from_options_and_store_nothing() {
    on_both_arcs!(samples_store_nothing);
}

/// One scripted rewrite: a reply after a delay, a failure, or no answer.
enum Step {
    Reply(&'static str, u64),
    Fail(RewriteFailure),
    Hang,
}

struct Steps(Mutex<VecDeque<Step>>);

impl NudgeRewriter for Steps {
    async fn rewrite(
        &self,
        _prompt: &RewritePrompt,
        _deadline: Duration,
    ) -> Result<String, RewriteFailure> {
        let step = self.0.lock().unwrap().pop_front().expect("a scripted step");
        match step {
            Step::Reply(text, millis) => {
                tokio::time::sleep(Duration::from_millis(millis)).await;
                Ok(text.to_owned())
            }
            Step::Fail(failure) => Err(failure),
            Step::Hang => std::future::pending().await,
        }
    }
}

fn stepping(steps: Vec<Step>) -> CardKit {
    CardKit {
        heading: HeadingRewrite {
            rewriter: Some(SharedRewriter(Arc::new(Steps(Mutex::new(steps.into()))))),
            persona: Some(persona()),
            words: None,
            log: None,
        },
        ..kit(None)
    }
}

fn header(kind: HeaderTrialKind, tries: u8) -> HeaderRequest {
    HeaderRequest {
        kind,
        tries,
        channel: None,
        invoked_in: Some(HOME.into()),
    }
}

#[tokio::test(start_paused = true)]
async fn header_trials_post_each_verdict_and_store_nothing() {
    let store = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let fake = Arc::new(support::fake());
    star(&*store).await;
    let before = rows(&*store).await;
    let trials = |steps| {
        desk(
            &store,
            &fake,
            &world,
            stepping(steps),
            &[HOME, SANDBOX],
            None,
        )
    };

    let countdown = trials(vec![
        Step::Reply("Waku waku!", 120),
        Step::Reply("Ready at 9!", 5),
        Step::Reply("XKalos!", 0),
        Step::Hang,
        Step::Fail(RewriteFailure::Refused),
    ]);
    assert_eq!(
        countdown
            .headers(header(HeaderTrialKind::Countdown, 5))
            .await,
        Ok(HeaderTrials::Posted {
            channel_id: HOME.into(),
            accepted: 1
        })
    );
    let report = created(&fake).pop().expect("report");
    assert_eq!(
        report.content.as_deref(),
        Some(
            "🧪 TEST — `countdown` header rewrite · 5 tries · 1 accepted\n\
             1. ✅ accepted · 120 ms · `Waku waku!`\n\
             2. ❌ rejected (factual term) · 5 ms · `Ready at 9!`\n\
             3. ❌ rejected (catalog term) · 0 ms · `XKalos!`\n\
             4. ⏱️ timeout · 30000 ms\n\
             5. ⚠️ refused · 0 ms"
        )
    );
    assert_eq!(
        serde_json::to_value(&report.allowed_mentions).unwrap(),
        serde_json::json!({ "parse": [] }),
        "no mentions allowed"
    );
    assert!(report.embeds.is_empty() && report.attachments.is_empty());

    // Day-of lines: the gate rule, a mention shown inert, then accepted.
    let day_of = trials(vec![
        Step::Reply("<@1001> it's {day}!", 0),
        Step::Reply("**Wake `up`**, {day}", 0),
        Step::Reply("Fresh dawn — {day}!", 0),
    ]);
    day_of
        .headers(header(HeaderTrialKind::DayOf, 3))
        .await
        .expect("trials");
    assert_eq!(
        content(&fake),
        "🧪 TEST — `day_of` header rewrite · 3 tries · 1 accepted\n\
         1. ❌ rejected (line rules) · 0 ms · `<@1001> it's {day}!`\n\
         2. ❌ rejected (markup) · 0 ms · `**Wake ˋupˋ**, {day}`\n\
         3. ✅ accepted · 0 ms · `Fresh dawn — {day}!`"
    );

    // Long replies are clipped so the report fits one message.
    let long: &'static str = Box::leak("a".repeat(3000).into_boxed_str());
    let clipped = trials((0..5).map(|_| Step::Reply(long, 0)).collect());
    clipped
        .headers(header(HeaderTrialKind::Digest, 5))
        .await
        .expect("trials");
    let text = content(&fake);
    assert!(text.chars().count() <= 2000, "{}", text.chars().count());
    assert_eq!(text.matches('…').count(), 5);

    // The configured test channel is the default; `channel:` wins.
    let testing = DebugDesk {
        test_channel: Some(SANDBOX.into()),
        ..trials(vec![Step::Reply("Yay!", 0), Step::Reply("Yay!", 0)])
    };
    assert_eq!(
        testing.headers(header(HeaderTrialKind::Digest, 1)).await,
        Ok(HeaderTrials::Posted {
            channel_id: SANDBOX.into(),
            accepted: 1
        })
    );
    let named = HeaderRequest {
        channel: Some(HOME.into()),
        ..header(HeaderTrialKind::Digest, 1)
    };
    assert_eq!(
        testing.headers(named).await,
        Ok(HeaderTrials::Posted {
            channel_id: HOME.into(),
            accepted: 1
        })
    );
    let nowhere = HeaderRequest {
        channel: Some("999".into()),
        ..header(HeaderTrialKind::Digest, 1)
    };
    assert_eq!(
        trials(Vec::new()).headers(nowhere).await,
        Ok(HeaderTrials::Unreachable)
    );

    // Nothing was journalled or stored.
    assert_eq!(rows(&*store).await, before);
    assert_eq!(
        store.load_view().await.expect("view"),
        ActiveClaims::default()
    );
    for channel in [HOME, SANDBOX] {
        assert!(
            store
                .debug_cards_in(channel, now() - TimeDelta::hours(24))
                .await
                .expect("list")
                .is_empty()
        );
    }
    assert_eq!(
        store
            .digest_phrase(&record_key(DeliveryTarget::Digest(week())))
            .await
            .expect("read"),
        None
    );

    // No rewriter configured: nothing is tried or posted.
    let posts = created(&fake).len();
    let plain = desk(&store, &fake, &world, kit(None), &[HOME], None);
    assert_eq!(
        plain.headers(header(HeaderTrialKind::DayOf, 3)).await,
        Ok(HeaderTrials::Disabled)
    );
    assert_eq!(created(&fake).len(), posts);
}
