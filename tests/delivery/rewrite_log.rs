//! The Rewrites log from the delivery side: header pre-generation, `/debug
//! header` and `/debug ping header:rewrite` each write exactly one row per
//! rewrite with its kind, stage, context, verdict and error code, and a
//! failing log never changes what was rewritten or posted.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kanade::bot::commands::{
    DebugCards, HeaderRequest, HeaderTrialKind, HeaderTrials, PingRequest, TestKind,
};
use kanade::bot::delivery::cards::{CardKit, HeadingRewrite, ReminderCardStore};
use kanade::chat::nudge::{
    NudgeRewriter, RewriteDetail, RewriteFailure, RewriteOutcome, RewritePrompt, SharedRewriteSink,
    SharedRewriter, StoreRewriteSink,
};
use kanade::domain::model_log::{
    LogPage, RewriteFacets, RewriteFilter, RewriteKind, RewriteLog, RewriteLogStore, RewriteStage,
};
use kanade::domain::scheduler::StoreError;
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::cards::{before_due, created, kit, persona, pregen, seed_day_of, world};
use crate::debug::{desk, star};
use crate::scenarios::{HOME, now};

/// Scripted detailed outcomes; the last repeats.
struct Coded(Mutex<VecDeque<RewriteOutcome>>);

impl Coded {
    fn new(outcomes: Vec<RewriteOutcome>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(outcomes.into())))
    }

    fn next(&self) -> RewriteOutcome {
        let mut queue = self.0.lock().unwrap();
        match queue.len() {
            0 => panic!("no scripted outcome"),
            1 => queue[0].clone(),
            _ => queue.pop_front().expect("outcome"),
        }
    }
}

impl NudgeRewriter for Coded {
    async fn rewrite(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> Result<String, RewriteFailure> {
        self.rewrite_detailed(prompt, deadline).await.result
    }

    async fn rewrite_detailed(&self, _: &RewritePrompt, _: Duration) -> RewriteOutcome {
        self.next()
    }
}

fn reply(text: &str) -> RewriteOutcome {
    RewriteOutcome {
        result: Ok(text.to_owned()),
        detail: RewriteDetail {
            alias: Some("kanata/rewrite".into()),
            reply: Some(text.to_owned()),
            reservation: Some(287),
            max_output_tokens: Some(96),
            ..RewriteDetail::default()
        },
    }
}

/// The live failure: a 200 whose usage exceeded the reservation.
fn over_budget() -> RewriteOutcome {
    RewriteOutcome {
        result: Err(RewriteFailure::Unavailable),
        detail: RewriteDetail {
            code: Some("budget_exceeded"),
            alias: Some("kanata/rewrite".into()),
            usage: Some(kanade::infrastructure::llm::Usage {
                prompt_tokens: 300,
                completion_tokens: 112,
            }),
            reply: Some("Waku waku!".into()),
            reservation: Some(287),
            max_output_tokens: Some(96),
            ..RewriteDetail::default()
        },
    }
}

/// A reserve too large for the call budget: refused before sending.
fn over_call_budget() -> RewriteOutcome {
    RewriteOutcome {
        result: Err(RewriteFailure::Unavailable),
        detail: RewriteDetail {
            code: Some("budget_exceeded"),
            alias: Some("kanata/rewrite".into()),
            reservation: Some(16_191),
            budget: Some(16_384 - 400),
            max_output_tokens: Some(16_000),
            ..RewriteDetail::default()
        },
    }
}

fn sink(logs: &Arc<MemoryScheduleStore>) -> SharedRewriteSink {
    Arc::new(StoreRewriteSink::new(Arc::clone(logs), Arc::new(now)))
}

fn logging(rewriter: Arc<Coded>, log: SharedRewriteSink) -> CardKit {
    CardKit {
        heading: HeadingRewrite {
            rewriter: Some(SharedRewriter(rewriter)),
            persona: Some(persona()),
            words: None,
            log: Some(log),
        },
        ..kit(None)
    }
}

async fn rows(logs: &MemoryScheduleStore) -> Vec<RewriteLog> {
    let mut rows = logs
        .list_rewrites(&RewriteFilter {
            limit: 50,
            ..RewriteFilter::default()
        })
        .await
        .expect("list")
        .items;
    rows.sort_by(|a, b| a.context.cmp(&b.context));
    rows
}

/// A log store whose every write fails.
struct Broken;

impl RewriteLogStore for Broken {
    async fn record_rewrite(&self, _: RewriteLog) -> Result<(), StoreError> {
        Err(StoreError::Backend("disk full".into()))
    }

    async fn load_rewrite(&self, _: &str) -> Result<Option<RewriteLog>, StoreError> {
        Ok(None)
    }

    async fn list_rewrites(&self, _: &RewriteFilter) -> Result<LogPage<RewriteLog>, StoreError> {
        Ok(LogPage {
            items: Vec::new(),
            next: None,
        })
    }

    async fn rewrite_facets(&self) -> Result<RewriteFacets, StoreError> {
        Ok(RewriteFacets::default())
    }
}

#[tokio::test(start_paused = true)]
async fn a_pregen_pass_logs_one_row_per_rewrite_under_its_card_key() {
    let store = Arc::new(MemoryScheduleStore::new());
    let logs = Arc::new(MemoryScheduleStore::new());
    let world = world();
    seed_day_of(&*store).await;
    let kit = logging(
        Coded::new(vec![reply("Rise and shine, it's {day}!")]),
        sink(&logs),
    );
    let report = pregen(&store, &world, &kit, before_due()).pass().await;
    assert_eq!(report.stored, 1);
    let rows = rows(&logs).await;
    assert_eq!(rows.len(), 1, "one card, one rewrite, one row");
    let row = &rows[0];
    assert_eq!(
        (row.kind, row.stage, row.verdict.as_str()),
        (RewriteKind::DayOf, RewriteStage::Batch, "accepted")
    );
    let key = row.context.as_deref().expect("card key");
    let record = store.card_record(key).await.expect("read").expect("stored");
    assert_eq!(
        record.heading.as_deref(),
        Some("Rise and shine, it's Thu 10 Sep!")
    );
    assert_eq!(row.seed, "Today — {day}");
    assert_eq!(row.line.as_deref(), Some("Rise and shine, it's {day}!"));
    assert_eq!(row.reply.as_deref(), Some("Rise and shine, it's {day}!"));
    assert_eq!(row.model.as_deref(), Some("kanata/rewrite"));
    assert_eq!(
        (row.reservation, row.max_output_tokens),
        (Some(287), Some(96))
    );
    assert_eq!(row.code, None);
    assert_eq!(row.at, now());
}

#[tokio::test(start_paused = true)]
async fn a_pregen_over_budget_logs_its_code_and_usage_and_stores_nothing() {
    let store = Arc::new(MemoryScheduleStore::new());
    let logs = Arc::new(MemoryScheduleStore::new());
    let world = world();
    seed_day_of(&*store).await;
    let kit = logging(Coded::new(vec![over_budget()]), sink(&logs));
    let report = pregen(&store, &world, &kit, before_due()).pass().await;
    assert_eq!((report.stored, report.failed), (0, 1));
    let rows = rows(&logs).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.verdict, "unavailable");
    assert_eq!(row.code.as_deref(), Some("budget_exceeded"));
    assert_eq!(
        (row.prompt_tokens, row.completion_tokens, row.reservation),
        (Some(300), Some(112), Some(287)),
        "used 412 > reserved 287"
    );
    assert_eq!(row.reply.as_deref(), Some("Waku waku!"));
    assert_eq!(
        row.line.as_deref(),
        Some("Today — {day}"),
        "the seed is used"
    );
}

#[tokio::test(start_paused = true)]
async fn a_failing_log_changes_no_outcome() {
    let world = world();
    let broken: SharedRewriteSink =
        Arc::new(StoreRewriteSink::new(Arc::new(Broken), Arc::new(now)));
    let mut outcomes = Vec::new();
    for log in [None, Some(broken)] {
        let store = Arc::new(MemoryScheduleStore::new());
        seed_day_of(&*store).await;
        let mut kit = logging(
            Coded::new(vec![reply("Rise and shine, it's {day}!")]),
            sink(&Arc::new(MemoryScheduleStore::new())),
        );
        kit.heading.log = log;
        let started = tokio::time::Instant::now();
        let report = pregen(&store, &world, &kit, before_due()).pass().await;
        outcomes.push((report, started.elapsed()));
    }
    assert_eq!(outcomes[0], outcomes[1], "same report, no extra wait");
    assert_eq!(outcomes[1].0.stored, 1, "stored though its row is not");
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
async fn debug_header_logs_each_try_and_reports_its_code() {
    let store = Arc::new(MemoryScheduleStore::new());
    let logs = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let fake = Arc::new(crate::support::fake());
    star(&*store).await;
    let kit = logging(
        Coded::new(vec![reply("Waku waku!"), over_budget()]),
        sink(&logs),
    );
    let desk = desk(&store, &fake, &world, kit, &[HOME], None);
    assert_eq!(
        desk.headers(header(HeaderTrialKind::Countdown, 2)).await,
        Ok(HeaderTrials::Posted {
            channel_id: HOME.into(),
            accepted: 1
        })
    );
    let report = created(&fake).pop().and_then(|message| message.content);
    assert_eq!(
        report.as_deref(),
        Some(
            "🧪 TEST — `countdown` header rewrite · 2 tries · 1 accepted\n\
             1. ✅ accepted · 0 ms · `Waku waku!`\n\
             2. ⚠️ unavailable (budget_exceeded) · 0 ms"
        )
    );
    let rows = rows(&logs).await;
    let summary: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row.kind,
                row.stage,
                row.context.as_deref(),
                row.verdict.as_str(),
                row.code.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                RewriteKind::Countdown,
                RewriteStage::Debug,
                Some("/debug header countdown · try 1/2"),
                "accepted",
                None
            ),
            (
                RewriteKind::Countdown,
                RewriteStage::Debug,
                Some("/debug header countdown · try 2/2"),
                "unavailable",
                Some("budget_exceeded")
            ),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn debug_ping_with_a_rewrite_logs_one_row_and_says_why_it_failed() {
    let store = Arc::new(MemoryScheduleStore::new());
    let logs = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let fake = Arc::new(crate::support::fake());
    let run_id = star(&*store).await;
    let kit = logging(Coded::new(vec![over_budget()]), sink(&logs));
    let desk = desk(&store, &fake, &world, kit, &[HOME], None);
    let report = desk
        .ping(PingRequest {
            rewrite: true,
            ..PingRequest::run(run_id.clone(), TestKind::DayOf, "1002".into())
        })
        .await
        .expect("ping");
    let note = report.header.expect("header note");
    assert!(!note.rewritten);
    assert_eq!(note.reason, "unavailable (budget_exceeded)");
    let rows = rows(&logs).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].kind, rows[0].stage, rows[0].verdict.as_str()),
        (RewriteKind::DayOf, RewriteStage::Debug, "unavailable")
    );
    assert_eq!(
        rows[0].context.as_deref(),
        Some(format!("/debug ping day_of · run {run_id}").as_str())
    );
    // Without `header:rewrite` nothing is tried or logged.
    desk.ping(PingRequest::run(run_id, TestKind::DayOf, "1002".into()))
        .await
        .expect("ping");
    assert_eq!(self::rows(&logs).await.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn both_budget_cases_are_logged_apart() {
    let store = Arc::new(MemoryScheduleStore::new());
    let logs = Arc::new(MemoryScheduleStore::new());
    let world = world();
    let fake = Arc::new(crate::support::fake());
    star(&*store).await;
    let kit = logging(
        Coded::new(vec![over_budget(), over_call_budget()]),
        sink(&logs),
    );
    let desk = desk(&store, &fake, &world, kit, &[HOME], None);
    desk.headers(header(HeaderTrialKind::DayOf, 2))
        .await
        .expect("trials");
    let rows = rows(&logs).await;
    let budgets: Vec<_> = rows
        .iter()
        .map(|row| {
            (
                row.code.as_deref(),
                row.prompt_tokens.zip(row.completion_tokens),
                row.reservation,
                row.budget,
            )
        })
        .collect();
    assert_eq!(
        budgets,
        [
            // used 412 > reserved 287
            (Some("budget_exceeded"), Some((300, 112)), Some(287), None),
            // reserved 16191 > budget 15984: nothing was sent
            (Some("budget_exceeded"), None, Some(16_191), Some(15_984)),
        ]
    );
}
