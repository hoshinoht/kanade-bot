//! Commands over ports wired by other slices: `/limits` (chat pilot),
//! `/rescan` (rescan runner), `/debug` (test cards), and `/say` (transport).

use std::sync::{Arc, Mutex};

use serde_json::json;

use kanade::api::rescan::{RescanFuture, RescanRunner, RescanView};
use kanade::bot::commands::{
    ChatAllowance, DebugCards, PortFuture, STAFF_LIMITS_REPLY, TestKind, TestPosted,
};
use kanade::bot::transport::{Call, Op, RejectionKind, Step};
use kanade::chat::pilot::{AllowanceSnapshot, MemberUsage, PoolUsage};
use kanade::domain::model_log::{RescanJob, RescanStatus};
use kanade::extract::rescan::RescanRequest;

use super::super::support::{ADMIN_ROLE, BOSSING_ROLE};
use super::{ALICE, BOB, DAN, KALOS, LOUNGE, Ports, R_KALOS, Slash, now, opt, sub};

struct Allowance(AllowanceSnapshot);

impl ChatAllowance for Allowance {
    fn snapshot(&self) -> AllowanceSnapshot {
        self.0.clone()
    }
}

fn allowance(used: usize, pool_used: usize) -> Arc<dyn ChatAllowance> {
    Arc::new(Allowance(AllowanceSnapshot {
        member_default: (4, 300.0),
        members: vec![MemberUsage {
            member_id: BOB.to_string(),
            used,
            limit: 4,
            window_s: 300.0,
            resets_in_s: 125.0,
            overridden: false,
        }],
        pool: PoolUsage {
            used: pool_used,
            limit: 12,
            window_s: 900.0,
            resets_in_s: 30.0,
        },
    }))
}

#[tokio::test]
async fn limits_reads_the_pilot_allowance() {
    let slash = Slash::new().await;
    assert_eq!(
        slash.run(BOB, "limits", json!([])).await,
        "❌ Chat limits aren't available right now."
    );
    let reply = slash
        .run_in(BOB, &[BOSSING_ROLE, ADMIN_ROLE], KALOS, "limits", json!([]))
        .await;
    assert_eq!(reply.content, STAFF_LIMITS_REPLY);

    let slash = Slash::with(Ports {
        allowance: Some(allowance(1, 3)),
        ..Ports::default()
    })
    .await;
    assert_eq!(
        slash.run(BOB, "limits", json!([])).await,
        "🎀 **Your chat answers** — 1 of 4 used\n▰▰▰▱▱▱▱▱▱▱▱▱\n3 left — a spent one comes back \
         in about 3 min."
    );
    assert_eq!(
        slash.run(DAN, "limits", json!([])).await,
        "🎀 **Your chat answers** — 0 of 4 used\n▱▱▱▱▱▱▱▱▱▱▱▱\nYour whole allowance is there — \
         ask away."
    );
    let slash = Slash::with(Ports {
        allowance: Some(allowance(4, 12)),
        ..Ports::default()
    })
    .await;
    assert_eq!(
        slash.run(BOB, "limits", json!([])).await,
        "🎀 **Your chat answers** — 4 of 4 used\n▰▰▰▰▰▰▰▰▰▰▰▰\nNone left — ask me again in about \
         3 min.\n-# The guild's shared pool is spent too, so nobody is being answered for about \
         30s."
    );
}

/// Queues jobs and lets them be cancelled.
#[derive(Default)]
struct Runner {
    jobs: Mutex<Vec<RescanView>>,
}

impl RescanRunner for Runner {
    fn submit(&self, request: RescanRequest) -> RescanFuture<'_, RescanView> {
        Box::pin(async move {
            let view = RescanView {
                job: RescanJob {
                    id: format!(
                        "abcdef{:02}-0000-4000-8000-000000000000",
                        self.jobs.lock().unwrap().len()
                    ),
                    channels: request.channels,
                    window: request.window,
                    source: request.source,
                    automated: request.automated,
                    requested_by: request.requested_by,
                    status: RescanStatus::Running,
                    created_at: now(),
                    started_at: Some(now()),
                    finished_at: None,
                    results: json!([{}]),
                    error: None,
                },
                current: None,
                expected: Default::default(),
                stopping: false,
            };
            self.jobs.lock().unwrap().push(view.clone());
            Ok(view)
        })
    }

    fn job(&self, id: String) -> RescanFuture<'_, Option<RescanView>> {
        Box::pin(async move {
            Ok(self
                .jobs
                .lock()
                .unwrap()
                .iter()
                .find(|view| view.job.id == id)
                .cloned())
        })
    }

    fn cancel(&self, id: String) -> RescanFuture<'_, Option<RescanView>> {
        Box::pin(async move {
            let mut jobs = self.jobs.lock().unwrap();
            let view = jobs.iter_mut().find(|view| view.job.id == id);
            Ok(view.map(|view| {
                view.job.status = RescanStatus::Cancelled;
                view.clone()
            }))
        })
    }
}

#[tokio::test]
async fn rescan_queues_through_the_runner() {
    let slash = Slash::new().await;
    assert_eq!(
        slash.run(BOB, "rescan", json!([])).await,
        "❌ Rescans aren't available right now."
    );

    let runner = Arc::new(Runner::default());
    let slash = Slash::with(Ports {
        rescans: Some(runner.clone()),
        ..Ports::default()
    })
    .await;
    assert_eq!(
        slash.run(BOB, "rescan", json!([opt("cancel", true)])).await,
        "Nothing is being re-read."
    );
    let reply = slash
        .run_in(BOB, &[BOSSING_ROLE], LOUNGE, "rescan", json!([]))
        .await;
    assert_eq!(
        reply.content,
        "❌ This channel isn't watched, so there's nothing to re-read. `/rescan scope:all \
         channels` reads the ones that are."
    );
    assert_eq!(
        slash
            .run(BOB, "rescan", json!([opt("window", "24h")]))
            .await,
        "🔎 Re-reading **#kalos-four** (the last 24 hours) — I'll post the cards in this channel \
         as I find them. `/rescan cancel:True` stops it.\n-# job `abcdef00`"
    );
    let submitted = runner.jobs.lock().unwrap()[0].job.clone();
    assert_eq!(submitted.channels, [KALOS.to_string()]);
    assert_eq!(submitted.source, "slash");
    assert_eq!(submitted.requested_by, Some(BOB.to_string()));
    assert_eq!(
        slash.run(BOB, "rescan", json!([opt("cancel", true)])).await,
        "🛑 `abcdef00` will stop after the channel it is on (1 of 1 done)."
    );
    let reply = slash
        .run_in(
            BOB,
            &[BOSSING_ROLE],
            LOUNGE,
            "rescan",
            json!([opt("scope", "all-channels")]),
        )
        .await;
    assert!(reply.content.starts_with(
        "🔎 Re-reading **#kalos-four** (this boss week) — I'll post the cards in each channel"
    ));
}

/// Records posts; `clear` reports what it was asked.
#[derive(Default)]
struct Cards {
    posted: Mutex<Vec<(String, TestKind, String)>>,
}

impl DebugCards for Cards {
    fn post(
        &self,
        run_id: String,
        kind: TestKind,
        requested_by: String,
    ) -> PortFuture<'_, Result<TestPosted, String>> {
        self.posted
            .lock()
            .unwrap()
            .push((run_id, kind, requested_by));
        Box::pin(async {
            Ok(TestPosted::Posted {
                channel_id: KALOS.to_string(),
            })
        })
    }

    fn clear(
        &self,
        channel_id: String,
        since: chrono::DateTime<chrono::Utc>,
    ) -> PortFuture<'_, Result<(usize, usize), String>> {
        assert_eq!(channel_id, KALOS.to_string());
        assert_eq!(now() - since, chrono::TimeDelta::hours(24));
        Box::pin(async { Ok((2, 1)) })
    }
}

#[tokio::test]
async fn debug_posts_test_cards_through_the_port() {
    let ping = |run: &str, kind: &str| sub("ping", json!([opt("run_id", run), opt("kind", kind)]));
    let slash = Slash::new().await;
    // Dan is a listed debug user.
    assert_eq!(
        slash.run(DAN, "debug", ping(R_KALOS, "day_of")).await,
        "❌ Test cards aren't available right now."
    );

    let cards = Arc::new(Cards::default());
    let slash = Slash::with(Ports {
        debug_cards: Some(cards.clone()),
        ..Ports::default()
    })
    .await;
    assert_eq!(
        slash.run(DAN, "debug", ping("1111", "countdown_60")).await,
        "✅ Posted a `countdown_60` test for run `#11111111` in <#301>. Its ✅/❌ drive the real \
         RSVP flow; the scheduled reminders are untouched."
    );
    assert_eq!(
        cards.posted.lock().unwrap().as_slice(),
        [(R_KALOS.to_owned(), TestKind::Countdown60, DAN.to_string())]
    );
    assert_eq!(
        slash.run(DAN, "debug", ping("nope", "day_of")).await,
        "❌ No run matches `nope`."
    );
    assert_eq!(
        slash.run(DAN, "debug", sub("clear_test", json!([]))).await,
        "🧹 Removed 2 test message(s), 1 could not be deleted."
    );
    // The picker lists every run for testers.
    let choices = slash
        .suggest(
            DAN,
            &[BOSSING_ROLE],
            "debug",
            sub("ping", json!([super::focused("run_id", "")])),
        )
        .await;
    assert_eq!(choices.len(), 3);
}

#[tokio::test]
async fn say_posts_verbatim_and_notifies_only_written_users() {
    let slash = Slash::with(Ports {
        closed: vec![LOUNGE.to_string()],
        ..Ports::default()
    })
    .await;
    let say = |text: &str| json!([opt("message", text)]);
    let text = "Raid at 9 <@1002> <@&777> @everyone 1004";
    let reply = slash
        .run_in(ALICE, &[ADMIN_ROLE], KALOS, "say", say(text))
        .await;
    assert_eq!(
        reply.content,
        "✅ Posted in <#301> (notifying 1 member(s))."
    );
    let calls = slash.discord.calls();
    let Some(Call::Create {
        channel, message, ..
    }) = calls
        .iter()
        .find(|call| matches!(call, Call::Create { .. }))
    else {
        panic!("posted: {calls:?}");
    };
    assert_eq!(channel.get(), KALOS);
    assert_eq!(message.content.as_deref(), Some(text));
    let allowed = serde_json::to_value(&message.allowed_mentions).unwrap();
    assert_eq!(allowed["users"], json!(["1002"]));
    assert!(allowed.get("roles").is_none_or(|roles| roles == &json!([])));
    assert_eq!(allowed["parse"], json!([]));

    let reply = slash
        .run_in(ALICE, &[ADMIN_ROLE], KALOS, "say", say("   "))
        .await;
    assert_eq!(reply.content, "❌ Nothing to say.");
    let long = "x".repeat(1901);
    let reply = slash
        .run_in(ALICE, &[ADMIN_ROLE], KALOS, "say", say(&long))
        .await;
    assert_eq!(
        reply.content,
        "❌ That's 1901 characters; keep it under 1900."
    );
    // Never split: over Discord's 2,000 however it counts, refused whole.
    let astral = "🧪".repeat(1_001);
    let reply = slash
        .run_in(ALICE, &[ADMIN_ROLE], KALOS, "say", say(&astral))
        .await;
    assert_eq!(
        reply.content,
        "❌ That's 1001 characters; keep it under 1900."
    );
    let reply = slash
        .run_in(
            ALICE,
            &[ADMIN_ROLE],
            KALOS,
            "say",
            json!([
                opt("message", "hi"),
                json!({ "name": "channel", "type": 7, "value": LOUNGE.to_string() })
            ]),
        )
        .await;
    assert_eq!(
        reply.content,
        "❌ the bot has no access to #lounge - grant the Kanade role View Channel + Send \
         Messages there"
    );
    // An unconfirmed post is reported, never retried.
    slash.discord.script(
        Op::Create,
        Step::Ambiguous {
            kind: kanade::bot::transport::AmbiguousKind::Timeout,
            applied: true,
        },
    );
    let reply = slash
        .run_in(ALICE, &[ADMIN_ROLE], KALOS, "say", say("hello"))
        .await;
    assert_eq!(
        reply.content,
        "⚠️ Discord did not confirm delivery. Check the channel before retrying."
    );
    slash
        .discord
        .script(Op::Create, Step::Reject(RejectionKind::MissingPermissions));
    let reply = slash
        .run_in(ALICE, &[ADMIN_ROLE], KALOS, "say", say("hello"))
        .await;
    assert!(
        reply
            .content
            .starts_with("❌ the bot has no access to #kalos-four")
    );
    assert_eq!(slash.discord.count(Op::Create), 3);
}
