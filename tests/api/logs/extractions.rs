use serde_json::json;

use super::{ERROR, Logs, ids};
use crate::schemas::assert_valid;

const EXTRACTIONS: &str = "extractions.json#/$defs/Extractions";
const EXTRACTION: &str = "extractions.json#/$defs/Extraction";

async fn list(logs: &Logs, query: &str) -> serde_json::Value {
    let path = format!("/api/admin/extractions{query}");
    let reply = logs.get(&path).await;
    assert_eq!(reply.status, 200, "{path}: {}", reply.text());
    let value = reply.json();
    assert_valid(EXTRACTIONS, &path, &value);
    value
}

#[tokio::test]
async fn extractions_list_every_call_with_total_facets_and_the_current_model() {
    let logs = Logs::new().await;
    let all = list(&logs, "").await;
    assert_eq!(ids(&all), ["x-new", "x-fail", "x-old"]);
    assert_eq!(all["model"], "kanata/extract");
    assert_eq!(all["total"], 3);
    assert_eq!(
        all["facets"],
        json!({
            "models": ["kanata/extract", "kanata/legacy"],
            "tools": [],
            "outcomes": ["failed", "no_change", "proposed"],
            "channels": [
                {"id": "kalos-four", "name": "#kalos-four"},
                {"id": "limbo-trio", "name": "#limbo-trio"},
                {"id": "star", "name": "#star"},
            ],
        })
    );
    let newest = &all["rows"][0];
    assert_eq!(newest["short_id"], "xnew");
    assert_eq!(newest["at"], "2026-09-29T02:00:00Z");
    assert_eq!(newest["messages"], 2);
    assert_eq!(newest["changes"], 2);
    assert_eq!(newest["channel"], "#kalos-four");
    let failed = &all["rows"][1];
    assert_eq!(failed["latency_ms"], serde_json::Value::Null);
    assert_eq!(failed["error"], "no answer");
    assert_eq!(failed["outcome"], "failed");

    let filtered = list(&logs, "?model=kanata/legacy").await;
    assert_eq!(filtered["total"], 3, "unfiltered");
}

#[tokio::test]
async fn extraction_filters_each_and_refuse_chat_only_ones() {
    let logs = Logs::new().await;
    let cases: [(&str, &[&str]); 9] = [
        ("?model=kanata/legacy", &["x-fail"]),
        ("?outcome=proposed,failed", &["x-new", "x-fail"]),
        ("?member=1001", &["x-new", "x-old"]),
        ("?channel=star", &["x-fail"]),
        ("?q=CARLING", &["x-fail"]),
        ("?from=2026-09-26&to=2026-09-29", &["x-new", "x-fail"]),
        ("?to=2026-09-19", &["x-old"]),
        // Empty Chat-only keys are unset, as every empty value.
        ("?tool=&min_ms=", &["x-new", "x-fail", "x-old"]),
        ("?outcome=no_change&member=1002", &[]),
    ];
    for (query, expected) in cases {
        assert_eq!(ids(&list(&logs, query).await), expected, "{query}");
    }
    for query in [
        "?tool=schedule_read",
        "?min_ms=5",
        "?outcome=answered",
        "?outcome=proposed,bogus",
        "?from=2026-09-29T00:00",
        "?from=2026-09-29&to=2026-09-28",
        "?limit=5",
        "?channel=a&channel=a",
    ] {
        let reply = logs.get(&format!("/api/admin/extractions{query}")).await;
        assert_eq!(reply.status, 422, "{query}: {}", reply.text());
        assert_valid(ERROR, query, &reply.json());
        assert_eq!(reply.api_error(), "invalid_filter", "{query}");
    }
}

#[tokio::test]
async fn extraction_detail_carries_the_call_its_proposals_and_refusals() {
    let logs = Logs::new().await;
    let reply = logs.get("/api/admin/extractions/x-new").await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let detail = reply.json();
    assert_valid(EXTRACTION, "detail", &detail);
    assert_eq!(
        detail["prompt"],
        "Messages:\n[Alice] kalos wed 9pm instead?"
    );
    assert_eq!(detail["raw_response"], r#"{"amendments": []}"#);
    assert_eq!(detail["latency_ms"], 12_000);
    assert_eq!(
        detail["refusals"],
        json!([{"change": "move", "code": "past", "message": "That time has already passed."}])
    );
    // The pruned message is left out.
    assert_eq!(
        detail["messages"],
        json!([{"id": "m-said", "author": "Alice", "at": "2026-09-29T01:50:00Z",
                "content": "kalos wed 9pm instead?"}])
    );
    assert_eq!(
        detail["amendments"],
        json!([
            {"kind": "move", "bosses": "XKalos", "when": "Wed 30 Sep 21:00", "confidence": 0.75,
             "status": "proposed"},
            {"kind": "change", "bosses": "", "when": "", "confidence": 0.0, "status": "missing"},
        ])
    );
    assert!(!logs.proposal.is_empty());

    let failed = logs.get("/api/admin/extractions/x-fail").await.json();
    assert_valid(EXTRACTION, "failed", &failed);
    assert_eq!(failed["refusals"], json!([]));
    assert_eq!(failed["messages"], json!([]));

    for id in ["nope", "c-answer"] {
        let reply = logs.get(&format!("/api/admin/extractions/{id}")).await;
        assert_eq!(reply.status, 404, "{id}");
        assert_eq!(reply.api_error(), "not_found");
    }
}
