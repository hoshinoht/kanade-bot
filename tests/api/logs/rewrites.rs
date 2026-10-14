use kanade::domain::model_log::{RewriteKind, RewriteLog, RewriteLogStore, RewriteStage};
use serde_json::json;

use super::{ERROR, Logs, ids, utc};
use crate::schemas::assert_valid;

const REWRITES: &str = "rewrites.json#/$defs/Rewrites";
const REWRITE: &str = "rewrites.json#/$defs/Rewrite";

fn row(id: &str, at: chrono::DateTime<chrono::Utc>) -> RewriteLog {
    RewriteLog {
        id: id.into(),
        at,
        kind: RewriteKind::DayOf,
        stage: RewriteStage::Batch,
        context: Some("day_of:r-kalos".into()),
        verdict: "accepted".into(),
        rule: None,
        code: None,
        latency_ms: Some(640),
        model: Some("kanata/rewrite".into()),
        reasoning: Some("low".into()),
        prompt_tokens: Some(200),
        completion_tokens: Some(8),
        reasoning_tokens: None,
        reservation: Some(287),
        budget: None,
        max_output_tokens: Some(96),
        seed: "Today — {day}".into(),
        reply: Some("Waku waku — {day}".into()),
        reasoning_content: Some("Keep {day}.".into()),
        line: Some("Waku waku — {day}".into()),
        request_id: Some("kanade-rewrite-0000abcd-1-1".into()),
        prompt: Some(
            "[system]\nRewrite the line.\n\n[user]\nLine to rewrite: Today — {day}".into(),
        ),
    }
}

async fn seeded() -> Logs {
    let logs = Logs::new().await;
    let store = &logs.reads.store;
    store
        .record_rewrite(row("r-accepted", utc(9, 29, 1, 0)))
        .await
        .unwrap();
    let mut over = row("r-over", utc(9, 29, 2, 0));
    over.kind = RewriteKind::Countdown;
    over.verdict = "unavailable".into();
    over.code = Some("budget_exceeded".into());
    over.prompt_tokens = Some(300);
    over.completion_tokens = Some(112);
    over.reply = Some("Waku waku!".into());
    over.line = Some("Onward!".into());
    over.seed = "Onward!".into();
    over.context = Some("countdown:r-kalos:60".into());
    store.record_rewrite(over).await.unwrap();
    let mut debug = row("r-debug", utc(9, 28, 2, 0));
    debug.kind = RewriteKind::Digest;
    debug.stage = RewriteStage::Debug;
    debug.verdict = "rejected".into();
    debug.rule = Some("factual term".into());
    debug.reply = Some("Ready at 9!".into());
    debug.line = Some("Let's go!".into());
    debug.seed = "Let's go!".into();
    debug.context = Some("/debug header digest · try 1/1".into());
    debug.model = Some("kanata/other".into());
    store.record_rewrite(debug).await.unwrap();
    let mut nudge = row("r-nudge", utc(9, 27, 2, 0));
    nudge.kind = RewriteKind::Nudge;
    nudge.stage = RewriteStage::Nudge;
    nudge.verdict = "no_persona".into();
    nudge.model = None;
    nudge.reasoning = None;
    nudge.prompt_tokens = None;
    nudge.completion_tokens = None;
    nudge.reservation = None;
    nudge.max_output_tokens = None;
    nudge.latency_ms = None;
    nudge.reply = None;
    nudge.reasoning_content = None;
    nudge.request_id = None;
    nudge.prompt = None;
    nudge.context = Some("self_service · playful".into());
    store.record_rewrite(nudge).await.unwrap();
    logs
}

async fn list(logs: &Logs, query: &str) -> serde_json::Value {
    let path = format!("/api/admin/rewrites{query}");
    let reply = logs.get(&path).await;
    assert_eq!(reply.status, 200, "{path}: {}", reply.text());
    let value = reply.json();
    assert_valid(REWRITES, &path, &value);
    value
}

#[tokio::test]
async fn rewrites_list_newest_first_with_total_facets_and_summary() {
    let logs = seeded().await;
    let all = list(&logs, "").await;
    assert_eq!(ids(&all), ["r-over", "r-accepted", "r-debug", "r-nudge"]);
    assert_eq!(all["total"], 4);
    assert_eq!(
        all["facets"],
        json!({
            "models": ["kanata/other", "kanata/rewrite"],
            "kinds": ["countdown", "day_of", "digest", "nudge"],
            "stages": ["batch", "debug", "nudge"],
            "verdicts": ["accepted", "no_persona", "rejected", "unavailable"],
        })
    );
    let over = &all["rows"][0];
    assert_eq!(over["verdict"], "unavailable");
    assert_eq!(over["code"], "budget_exceeded");
    assert_eq!(
        (
            &over["prompt_tokens"],
            &over["completion_tokens"],
            &over["reservation"]
        ),
        (&json!(300), &json!(112), &json!(287))
    );
    assert_eq!(over["at"], "2026-09-29T02:00:00Z");
    assert!(
        over.get("reasoning_content").is_none(),
        "list rows omit bodies"
    );
    assert!(over.get("reply").is_none());
    assert!(over.get("prompt").is_none(), "list rows omit the prompt");
    let nudge = &all["rows"][3];
    assert_eq!(nudge["model"], json!(null));
    assert_eq!(nudge["prompt_tokens"], json!(null), "unreported is null");
    assert_eq!(
        all["summary"],
        json!([
            {"model": "kanata/other", "count": 1, "accepted": 0, "prompt_tokens": 200,
             "completion_tokens": 8, "reported": 1, "est_ratio": 1.05},
            {"model": "kanata/rewrite", "count": 2, "accepted": 1, "prompt_tokens": 500,
             "completion_tokens": 120, "reported": 2, "est_ratio": 1.31},
        ])
    );
}

#[tokio::test]
async fn rewrite_filters_combine_and_refuse_unknown_values() {
    let logs = seeded().await;
    let cases: [(&str, &[&str]); 9] = [
        ("?kind=countdown", &["r-over"]),
        ("?stage=batch", &["r-over", "r-accepted"]),
        ("?verdict=unavailable,rejected", &["r-over", "r-debug"]),
        ("?model=kanata/other", &["r-debug"]),
        ("?q=BUDGET", &["r-over"]),
        ("?q=ready%20at", &["r-debug"]),
        ("?from=2026-09-28&to=2026-09-28", &["r-debug"]),
        (
            "?kind=&stage=&verdict=",
            &["r-over", "r-accepted", "r-debug", "r-nudge"],
        ),
        ("?stage=nudge&verdict=accepted", &[]),
    ];
    for (query, expected) in cases {
        let value = list(&logs, query).await;
        assert_eq!(ids(&value), expected, "{query}");
        assert_eq!(value["total"], 4, "{query}: unfiltered total");
    }
    for query in [
        "?kind=send",
        "?stage=tick",
        "?verdict=accepted,bogus",
        "?outcome=accepted",
        "?channel=star",
        "?from=2026-09-29&to=2026-09-28",
        "?kind=day_of&kind=digest",
    ] {
        let reply = logs.get(&format!("/api/admin/rewrites{query}")).await;
        assert_eq!(reply.status, 422, "{query}: {}", reply.text());
        assert_valid(ERROR, query, &reply.json());
        assert_eq!(reply.api_error(), "invalid_filter", "{query}");
    }
}

#[tokio::test]
async fn rewrite_detail_carries_the_reply_reasoning_and_reservation() {
    let logs = seeded().await;
    let reply = logs.get("/api/admin/rewrites/r-accepted").await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let detail = reply.json();
    assert_valid(REWRITE, "detail", &detail);
    assert_eq!(detail["reply"], "Waku waku — {day}");
    assert_eq!(detail["reasoning_content"], "Keep {day}.");
    assert_eq!(detail["max_output_tokens"], 96);
    assert_eq!(detail["prompt_estimate"], 191);
    assert_eq!(detail["request_id"], "kanade-rewrite-0000abcd-1-1");
    assert_eq!(detail["short_id"], "raccepte");
    assert_eq!(detail["seed"], "Today — {day}");
    assert_eq!(
        detail["prompt"],
        "[system]\nRewrite the line.\n\n[user]\nLine to rewrite: Today — {day}"
    );
    let unsent = logs.get("/api/admin/rewrites/r-nudge").await.json();
    assert_valid(REWRITE, "unsent", &unsent);
    assert_eq!(unsent["prompt"], json!(null), "no call, no prompt");

    let missing = logs.get("/api/admin/rewrites/r-missing").await;
    assert_eq!(missing.status, 404);
    assert_valid(ERROR, "missing", &missing.json());
    assert_eq!(missing.api_error(), "not_found");
}
