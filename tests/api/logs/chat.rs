use serde_json::json;

use super::{ERROR, Logs, ids};
use crate::schemas::assert_valid;

const CHAT: &str = "chat.json#/$defs/Chat";
const TURN: &str = "chat.json#/$defs/ChatTurn";

async fn list(logs: &Logs, query: &str) -> serde_json::Value {
    let path = format!("/api/admin/chat{query}");
    let reply = logs.get(&path).await;
    assert_eq!(reply.status, 200, "{path}: {}", reply.text());
    let value = reply.json();
    assert_valid(CHAT, &path, &value);
    value
}

#[tokio::test]
async fn chat_lists_every_row_newest_first_with_total_facets_and_summary() {
    let logs = Logs::new().await;
    let all = list(&logs, "").await;
    assert_eq!(
        ids(&all),
        ["c-timeout", "c-answer", "c-withheld", "c-limited"]
    );
    assert_eq!(all["total"], 4);
    assert_eq!(
        all["facets"],
        json!({
            "models": ["kanata/chat", "kanata/chat-cloud"],
            "tools": ["propose_move", "schedule_read"],
            "outcomes": ["answered", "clean_retry", "content_blocked", "rate_limited", "timeout", "withheld"],
            "channels": [
                {"id": "kalos-four", "name": "#kalos-four"},
                {"id": "limbo-trio", "name": "#limbo-trio"},
                {"id": "star", "name": "#star"},
            ],
        })
    );
    let answer = &all["rows"][1];
    assert_eq!(
        answer,
        &json!({
            "id": "c-answer",
            "at": "2026-09-28T12:00:00Z",
            "member": {"id": "1001", "name": "Alice"},
            "channel": "#kalos-four",
            "channel_id": "kalos-four",
            "model": "kanata/chat",
            "models": ["kanata/chat"],
            "latency_ms": 4000,
            "outcome": "answered",
            "asked": "When is Kalos?",
            "tools_used": ["schedule_read"],
        })
    );
    // A rate-limited question ran no model.
    assert_eq!(all["rows"][3]["model"], "—");
    assert_eq!(all["rows"][3]["latency_ms"], 0);
    assert_eq!(
        all["summary"],
        json!([
            {"model": "kanata/chat", "count": 2, "answered": 1, "refused": 0, "errors": 1,
             "p50_ms": 4000, "tool_calls": 1},
            {"model": "kanata/chat-cloud", "count": 1, "answered": 0, "refused": 0, "errors": 0,
             "p50_ms": 0, "tool_calls": 1},
        ])
    );

    // `total` stays unfiltered; the summary follows the filter.
    let filtered = list(&logs, "?member=1002").await;
    assert_eq!(ids(&filtered), ["c-withheld"]);
    assert_eq!(filtered["total"], 4);
    assert_eq!(filtered["summary"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn chat_filters_each_and_combined() {
    let logs = Logs::new().await;
    let cases: [(&str, &[&str]); 19] = [
        ("?model=kanata/chat-cloud", &["c-withheld"]),
        ("?model=kanata/chat", &["c-timeout", "c-answer"]),
        // Guild-local dates, inclusive.
        ("?from=2026-09-28", &["c-timeout", "c-answer"]),
        ("?to=2026-09-27", &["c-withheld", "c-limited"]),
        ("?from=2026-09-27&to=2026-09-27", &["c-withheld"]),
        ("?outcome=timeout,rate_limited", &["c-timeout", "c-limited"]),
        // Flags match their outcome filter too.
        ("?outcome=withheld", &["c-withheld"]),
        ("?outcome=clean_retry", &["c-timeout"]),
        ("?channel=limbo-trio", &["c-limited"]),
        ("?member=1001", &["c-timeout", "c-answer"]),
        ("?q=KALOS", &["c-answer"]),
        ("?q=help", &["c-withheld"]),
        ("?tool=schedule_read", &["c-answer"]),
        ("?min_ms=5000", &["c-timeout", "c-withheld"]),
        ("?member=1001&min_ms=5000", &["c-timeout"]),
        // Empty values are unset.
        (
            "?min_ms=&q=",
            &["c-timeout", "c-answer", "c-withheld", "c-limited"],
        ),
        ("?outcome=answered&channel=star", &[]),
        ("?q=%20summarise%20", &["c-timeout"]),
        // Parses; no v4-imported row here.
        ("?outcome=unknown", &[]),
    ];
    for (query, expected) in cases {
        assert_eq!(ids(&list(&logs, query).await), expected, "{query}");
    }
}

#[tokio::test]
async fn chat_refuses_every_invalid_filter_with_invalid_filter() {
    let logs = Logs::new().await;
    for query in [
        "?outcome=nope",
        "?outcome=answered,proposed",
        "?from=tuesday",
        "?to=2026-9-29",
        "?from=%2B2026-09-29",
        "?from=2026-09-30&to=2026-09-29",
        "?from=2026-02-30",
        "?min_ms=1e3",
        "?min_ms=-5",
        "?min_ms=5.5",
        "?min_ms=%2B5",
        "?min_ms=99999999999",
        "?week=this",
        "?cursor=abc",
        "?model=a&model=b",
        "?q=%zz",
    ] {
        let path = format!("/api/admin/chat{query}");
        let reply = logs.get(&path).await;
        assert_eq!(reply.status, 422, "{query}: {}", reply.text());
        assert_valid(ERROR, query, &reply.json());
        assert_eq!(reply.api_error(), "invalid_filter", "{query}");
    }
}

#[tokio::test]
async fn a_withheld_question_is_never_shown_or_searchable() {
    let logs = Logs::new().await;
    let all = list(&logs, "").await;
    assert_eq!(all["rows"][2]["asked"], "[message withheld]");
    assert!(ids(&list(&logs, "?q=forbidden").await).is_empty());

    let reply = logs.get("/api/admin/chat/c-withheld").await;
    assert_eq!(reply.status, 200);
    let turn = reply.json();
    assert_valid(TURN, "withheld turn", &turn);
    assert_eq!(turn["asked"], "[message withheld]");
    assert_eq!(turn["said"], "I can't help with that one.");
    assert_eq!(turn["raw"], "[message withheld]");
    assert_eq!(turn["tools"][0]["arguments"], "[message withheld]");
    assert_eq!(turn["member"], json!({"id": "1002", "name": "Bobby"}));
    assert!(!reply.text().contains("forbidden"), "{}", reply.text());
}

#[tokio::test]
async fn chat_detail_is_the_row_plus_the_turn_and_unknown_ids_are_404() {
    let logs = Logs::new().await;
    let reply = logs.get("/api/admin/chat/c-answer").await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let turn = reply.json();
    assert_valid(TURN, "turn", &turn);
    assert_eq!(turn["asked"], "When is Kalos?");
    assert_eq!(turn["said"], "Tuesday 22:00.");
    assert_eq!(
        turn["tools"],
        json!([{"name": "schedule_read", "arguments": "{\"week\":\"this\"}", "result": "",
                "took_ms": 0, "outcome": "ok"}])
    );
    assert_eq!(
        turn["rounds"],
        json!([
            {"round": 1, "requested_tools": ["schedule_read"], "finish": "tool_calls"},
            {"round": 2, "requested_tools": [], "finish": "stop"},
        ])
    );
    assert_eq!(turn["cards"], json!([]));
    assert_eq!(turn["raw"], "Tuesday 22:00.");

    for id in ["nope", "x-new"] {
        let reply = logs.get(&format!("/api/admin/chat/{id}")).await;
        assert_eq!(reply.status, 404, "{id}");
        assert_eq!(reply.api_error(), "not_found");
    }
}
