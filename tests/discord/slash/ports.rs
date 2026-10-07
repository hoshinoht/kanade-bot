//! Commands over ports wired by other slices: `/limits` (chat pilot),
//! `/rescan` (rescan runner), `/debug` (test cards), and `/say` (transport).

use std::sync::{Arc, Mutex};

use serde_json::json;

use kanade::api::rescan::{RescanFuture, RescanRunner, RescanView};
use kanade::bot::commands::{
    ChatAllowance, DebugCards, HeaderNote, HeaderRequest, HeaderTrialKind, HeaderTrials,
    PingRequest, PortFuture, STAFF_LIMITS_REPLY, SampleRun, TestKind, TestPosted, TestReport,
    TestSubject,
};
use kanade::bot::transport::{Call, Op, RejectionKind, Step};
use kanade::chat::pilot::{AllowanceSnapshot, MemberUsage, PoolUsage};
use kanade::domain::model_log::{RescanJob, RescanStatus};
use kanade::domain::schedule::RunStatus;
use kanade::domain::settings::MessageStyle;
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

/// Records requests; answers with `reply` (default: posted in the party
/// channel) and `trials`.
#[derive(Default)]
struct Cards {
    posted: Mutex<Vec<PingRequest>>,
    headers: Mutex<Vec<HeaderRequest>>,
    reply: Mutex<Option<TestReport>>,
    trials: Mutex<Option<HeaderTrials>>,
}

impl DebugCards for Cards {
    fn ping(&self, request: PingRequest) -> PortFuture<'_, Result<TestReport, String>> {
        self.posted.lock().unwrap().push(request);
        let reply = self.reply.lock().unwrap().clone().unwrap_or(TestReport {
            posted: TestPosted::Posted {
                channel_id: KALOS.to_string(),
            },
            header: None,
        });
        Box::pin(async move { Ok(reply) })
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

    fn headers(&self, request: HeaderRequest) -> PortFuture<'_, Result<HeaderTrials, String>> {
        self.headers.lock().unwrap().push(request);
        let trials = self
            .trials
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(HeaderTrials::Disabled);
        Box::pin(async move { Ok(trials) })
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
        [PingRequest {
            invoked_in: Some(KALOS.to_string()),
            ..PingRequest::run(R_KALOS.to_owned(), TestKind::Countdown60, DAN.to_string())
        }]
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

fn channel_opt(name: &str, id: u64) -> serde_json::Value {
    json!({ "name": name, "type": 7, "value": id.to_string() })
}

fn int_opt(name: &str, value: i64) -> serde_json::Value {
    json!({ "name": name, "type": 4, "value": value })
}

#[tokio::test]
async fn debug_ping_options_reach_the_port_and_shape_the_reply() {
    let cards = Arc::new(Cards::default());
    let slash = Slash::with(Ports {
        debug_cards: Some(cards.clone()),
        ..Ports::default()
    })
    .await;
    let last = || {
        cards
            .posted
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a ping")
    };

    // Another channel, a style override and a rewrite that fell back.
    *cards.reply.lock().unwrap() = Some(TestReport {
        posted: TestPosted::Sandboxed {
            channel_id: LOUNGE.to_string(),
        },
        header: Some(HeaderNote {
            rewritten: false,
            reason: "rejected (markup)".into(),
        }),
    });
    let reply = slash
        .run(
            DAN,
            "debug",
            sub(
                "ping",
                json!([
                    opt("kind", "day_of"),
                    opt("run_id", R_KALOS),
                    channel_opt("channel", LOUNGE),
                    opt("style", "redesigned"),
                    opt("header", "rewrite"),
                ]),
            ),
        )
        .await;
    assert_eq!(
        reply,
        "✅ Posted a sandbox `day_of` test for run `#11111111` in <#302> (display only: nothing \
         is stored for it and reactions do nothing here). Style: `redesigned` (this post only). \
         Header: the seed (rejected (markup))."
    );
    let request = last();
    assert_eq!(request.channel.as_deref(), Some("302"));
    assert_eq!(request.style, Some(MessageStyle::Redesigned));
    assert!(request.rewrite);

    // A sample run from options; nobody named in `party` means the invoker.
    *cards.reply.lock().unwrap() = Some(TestReport {
        posted: TestPosted::Sandboxed {
            channel_id: KALOS.to_string(),
        },
        header: Some(HeaderNote {
            rewritten: true,
            reason: "accepted".into(),
        }),
    });
    let reply = slash
        .run(
            DAN,
            "debug",
            sub(
                "ping",
                json!([
                    opt("kind", "countdown_15"),
                    opt("bosses", "hard star, xkalos"),
                    int_opt("time", 30),
                    opt("party", "<@1001> <@1002>"),
                    opt("in", "<@1001>"),
                    opt("out", "<@1006>"),
                    opt("status", "at_risk"),
                    opt("header", "rewrite"),
                ]),
            ),
        )
        .await;
    assert_eq!(
        reply,
        "✅ Posted a sample `countdown_15` test in <#301> (display only: nothing is stored for \
         it and reactions do nothing here). Header: fresh rewrite (not stored)."
    );
    assert_eq!(
        last().subject,
        TestSubject::Sample(SampleRun {
            bosses: vec!["HMaleficStar".into(), "XKalos".into()],
            at: now() + chrono::TimeDelta::minutes(30),
            party: vec!["1001".into(), "1002".into()],
            yes: vec!["1001".into()],
            no: vec!["1006".into()],
            status: RunStatus::AtRisk,
        })
    );
    slash
        .run(
            DAN,
            "debug",
            sub(
                "ping",
                json!([opt("kind", "amend"), opt("bosses", "normal star")]),
            ),
        )
        .await;
    let TestSubject::Sample(sample) = last().subject else {
        panic!("a sample");
    };
    assert_eq!(sample.party, [DAN.to_string()]);
    assert_eq!(sample.at, now() + chrono::TimeDelta::minutes(120));
    assert_eq!(sample.status, RunStatus::Planned);

    // The week's digest needs no run; other kinds need a run or bosses.
    *cards.reply.lock().unwrap() = Some(TestReport {
        posted: TestPosted::Sandboxed {
            channel_id: KALOS.to_string(),
        },
        header: None,
    });
    assert_eq!(
        slash
            .run(DAN, "debug", sub("ping", json!([opt("kind", "digest")])))
            .await,
        "✅ Posted a `digest` test of this boss week in <#301> (display only: nothing is stored \
         for it and reactions do nothing here)."
    );
    assert_eq!(last().subject, TestSubject::Week);
    let before = cards.posted.lock().unwrap().len();
    for (options, refusal) in [
        (
            json!([opt("kind", "day_of")]),
            "❌ Give a `run_id`, or `bosses` for a sample run.",
        ),
        (
            json!([opt("kind", "day_of"), opt("party", "<@1001>")]),
            "❌ A sample run needs `bosses`.",
        ),
        (
            json!([
                opt("kind", "day_of"),
                opt("run_id", R_KALOS),
                opt("bosses", "xkalos")
            ]),
            "❌ Give either `run_id` or sample run options, not both.",
        ),
        (
            json!([
                opt("kind", "day_of"),
                opt("bosses", "xkalos"),
                opt("in", "Bob")
            ]),
            "❌ `in` needs member mentions like <@123>.",
        ),
    ] {
        assert_eq!(slash.run(DAN, "debug", sub("ping", options)).await, refusal);
    }
    assert!(
        slash
            .run(
                DAN,
                "debug",
                sub(
                    "ping",
                    json!([opt("kind", "day_of"), opt("bosses", "hard kalos")])
                ),
            )
            .await
            .starts_with("❌ "),
        "the catalog's own parse error"
    );
    assert_eq!(
        cards.posted.lock().unwrap().len(),
        before,
        "refused before the port"
    );
}

#[tokio::test]
async fn debug_header_tries_through_the_port() {
    let cards = Arc::new(Cards::default());
    let slash = Slash::with(Ports {
        debug_cards: Some(cards.clone()),
        ..Ports::default()
    })
    .await;
    assert_eq!(
        slash
            .run(DAN, "debug", sub("header", json!([opt("kind", "day_of")])))
            .await,
        "Header rewrites aren't set up here (no rewrite model or persona), so there is nothing \
         to try."
    );
    *cards.trials.lock().unwrap() = Some(HeaderTrials::Posted {
        channel_id: LOUNGE.to_string(),
        accepted: 1,
    });
    assert_eq!(
        slash
            .run(
                DAN,
                "debug",
                sub(
                    "header",
                    json!([
                        opt("kind", "countdown"),
                        int_opt("tries", 2),
                        channel_opt("channel", LOUNGE)
                    ]),
                ),
            )
            .await,
        "✅ Tried the `countdown` header rewrite 2 time(s), 1 accepted; results posted in \
         <#302>. Nothing was stored."
    );
    assert_eq!(
        cards.headers.lock().unwrap().as_slice(),
        [
            HeaderRequest {
                kind: HeaderTrialKind::DayOf,
                tries: 3,
                channel: None,
                invoked_in: Some(KALOS.to_string()),
            },
            HeaderRequest {
                kind: HeaderTrialKind::Countdown,
                tries: 2,
                channel: Some(LOUNGE.to_string()),
                invoked_in: Some(KALOS.to_string()),
            }
        ]
    );
}

#[tokio::test]
async fn debug_materialises_through_the_writer_and_lists_reminder_rows() {
    let slash = Slash::new().await;
    let reminders = |run: Option<&str>| {
        sub(
            "reminders",
            run.map_or_else(|| json!([]), |run| json!([opt("run_id", run)])),
        )
    };
    // The seeded runs were written without reminder rows.
    assert_eq!(slash.run(DAN, "debug", reminders(None)).await, "_none_");
    assert_eq!(
        slash.run(DAN, "debug", reminders(Some(R_KALOS))).await,
        "_none_"
    );

    // The week after next has no run from the Tuesday timing yet.
    let created = slash.run(DAN, "debug", sub("materialise", json!([]))).await;
    let (head, line) = created.split_once('\n').unwrap();
    assert_eq!(head, "Created 1 run(s):");
    let short = line
        .strip_prefix("run `#")
        .and_then(|rest| rest.strip_suffix("` · XKalos · Tue 13 Oct 22:00"))
        .unwrap_or_else(|| panic!("{line}"));
    assert_eq!(
        slash.run(DAN, "debug", sub("materialise", json!([]))).await,
        "Nothing new - both weeks were already materialised."
    );

    // Materialising also placed the seeded runs' missing rows; soonest first.
    let all = slash.run(DAN, "debug", reminders(None)).await;
    assert_eq!(
        all.lines().collect::<Vec<_>>(),
        [
            "run `#11111111` · `day_of` · Tue 29 Sep 09:00 · sent".to_owned(),
            "run `#11111111` · `countdown_60` · Tue 29 Sep 21:00 · pending".to_owned(),
            "run `#11111111` · `countdown_15` · Tue 29 Sep 21:45 · pending".to_owned(),
            "run `#33333333` · `day_of` · Tue 06 Oct 09:00 · pending".to_owned(),
            "run `#33333333` · `countdown_60` · Tue 06 Oct 21:00 · pending".to_owned(),
            "run `#33333333` · `countdown_15` · Tue 06 Oct 21:45 · pending".to_owned(),
            format!("run `#{short}` · `day_of` · Tue 13 Oct 09:00 · pending"),
            format!("run `#{short}` · `countdown_60` · Tue 13 Oct 21:00 · pending"),
            format!("run `#{short}` · `countdown_15` · Tue 13 Oct 21:45 · pending"),
        ]
    );
    let mine = slash.run(DAN, "debug", reminders(Some(short))).await;
    assert_eq!(mine, all.lines().skip(6).collect::<Vec<_>>().join("\n"));
    assert_eq!(
        slash.run(DAN, "debug", reminders(Some("nope"))).await,
        "❌ No run matches `nope`."
    );
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
