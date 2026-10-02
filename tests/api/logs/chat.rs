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
            "member_id": "1001",
            "channel": "#kalos-four",
            "channel_id": "kalos-four",
            "model": "kanata/chat",
            "models": ["kanata/chat"],
            "latency_ms": 4000,
            "outcome": "answered",
            "asked": "When is Kalos?",
            "tools_used": ["schedule_read"],
            "prompt_tokens": 1200,
            "completion_tokens": 30,
            "reasoning_tokens": 12,
        })
    );
    // Counts only: a withheld turn shows them; turn totals pass through as logged.
    assert_eq!(
        (
            &all["rows"][2]["prompt_tokens"],
            &all["rows"][2]["completion_tokens"]
        ),
        (&json!(500), &json!(20))
    );
    assert_eq!(
        (
            &all["rows"][3]["prompt_tokens"],
            &all["rows"][3]["completion_tokens"]
        ),
        (&json!(10), &json!(null))
    );
    // A rate-limited question ran no model.
    assert_eq!(all["rows"][3]["model"], "—");
    assert_eq!(all["rows"][3]["latency_ms"], 0);
    assert_eq!(
        all["summary"],
        json!([
            // Round rows with a pair: c-answer round 1 (1200/1000) and
            // c-timeout (900/1000); median of 1.2 and 0.9.
            {"model": "kanata/chat", "count": 2, "answered": 1, "refused": 0, "errors": 1,
             "p50_ms": 4000, "tool_calls": 1, "prompt_tokens": 2100, "completion_tokens": 40,
             "reported": 2, "est_ratio": 1.05},
            {"model": "kanata/chat-cloud", "count": 1, "answered": 0, "refused": 0, "errors": 0,
             "p50_ms": 0, "tool_calls": 1, "prompt_tokens": 500, "completion_tokens": 20,
             "reported": 1, "est_ratio": 1.25},
        ])
    );

    // `total` stays unfiltered; the summary follows the filter.
    let filtered = list(&logs, "?member=1002").await;
    assert_eq!(ids(&filtered), ["c-withheld"]);
    assert_eq!(filtered["total"], 4);
    assert_eq!(filtered["summary"].as_array().unwrap().len(), 1);
    let only = list(&logs, "?member=1004").await;
    assert_eq!(only["summary"], json!([]), "no model ran");
}

/// Summary usage comes from each model's own round rows: unreported rounds
/// leave null sums (never 0), turn totals are never used, and the ratio is
/// the median over rounds with both a pair and a non-zero estimate.
#[test]
fn chat_summary_usage_sums_round_rows_per_model() {
    use kanade::api::dto::logs::chat_summary;
    use kanade::domain::model_log::ChatOutcome;

    let round = |model: &str, pair: Option<(u64, u64)>, estimate: Option<u64>| {
        let mut round = super::round(model, &[], json!([]), None);
        round.prompt_tokens = pair.map(|pair| pair.0);
        round.completion_tokens = pair.map(|pair| pair.1);
        round.prompt_estimate = estimate;
        round
    };
    let mut mixed = super::chat(
        "c-1",
        super::utc(9, 28, 12, 0),
        "1001",
        "star",
        "q",
        "r",
        ChatOutcome::Answered,
        Some(10),
        vec![
            round("a", Some((100, 1)), Some(100)),
            round("a", Some((300, 3)), Some(100)),
            round("a", Some((200, 2)), Some(100)),
            round("a", Some((50, 5)), None),
            round("b", None, Some(70)),
        ],
    );
    // Turn totals that disagree with the rounds are ignored by the summary.
    (mixed.prompt_tokens, mixed.completion_tokens) = (Some(1), Some(1));
    let summary = chat_summary(&[mixed]);
    let usage: Vec<_> = summary
        .iter()
        .map(|model| {
            (
                model["model"].clone(),
                model["prompt_tokens"].clone(),
                model["completion_tokens"].clone(),
                model["reported"].clone(),
                model["est_ratio"].clone(),
            )
        })
        .collect();
    assert_eq!(
        usage,
        [
            (json!("a"), json!(650), json!(11), json!(4), json!(2.0)),
            (json!("b"), json!(null), json!(null), json!(0), json!(null)),
        ]
    );
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
    assert_eq!(turn["tools"][0]["result"], "[message withheld]");
    assert_eq!(turn["tools"][0]["took_ms"], 5);
    assert_eq!(turn["rounds"][0]["reasoning_content"], "[message withheld]");
    assert_eq!(turn["rounds"][0]["reasoning_tokens"], 8);
    assert_eq!(turn["reasoning_tokens"], 8);
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
        json!([{"round": 1, "name": "schedule_read", "arguments": "{\"week\":\"this\"}",
                "result": "Kalos: Tue 22:00", "took_ms": 12, "outcome": "ok"}])
    );
    let clean = json!({"clean": false, "content_filter": false});
    assert_eq!(
        turn["rounds"],
        json!([
            {"round": 1, "requested_tools": ["schedule_read"], "finish": "tool_calls",
             "model": "kanata/chat", "effort": "low", "route": "homelab", "latency_ms": 1000,
             "prompt_tokens": 1200, "completion_tokens": 30, "prompt_estimate": 1000,
             "reasoning_content": "Look up Kalos before answering.", "reasoning_tokens": 12,
             "guardrail": clean},
            {"round": 2, "requested_tools": [], "finish": "stop", "model": "kanata/chat",
             "effort": null, "route": "external_masked", "latency_ms": null,
             "prompt_tokens": null, "completion_tokens": null, "prompt_estimate": 1100,
             "reasoning_content": "Use the read result.", "reasoning_tokens": null,
             "guardrail": {"clean": true, "content_filter": false}},
        ])
    );
    assert_eq!(turn["persona"], "kanade");
    assert_eq!(turn["profile"], "gentle");
    assert_eq!(turn["profile_source"], "saved");
    assert_eq!(turn["route"], "external_masked");
    assert_eq!(turn["error"], json!(null));
    assert_eq!(turn["error_code"], json!(null));
    assert_eq!(turn["guardrail"], json!({"pseudonymized": true}));
    assert_eq!(turn["masked"], false, "no Model view stored");
    assert_eq!(turn["model_view"], json!(null));
    assert_eq!(turn["cards"], json!([]));
    assert_eq!(turn["raw"], "Tuesday 22:00.");

    for id in ["nope", "x-new"] {
        let reply = logs.get(&format!("/api/admin/chat/{id}")).await;
        assert_eq!(reply.status, 404, "{id}");
        assert_eq!(reply.api_error(), "not_found");
    }
}

#[test]
fn an_imported_v4_turn_shows_its_output_and_ms() {
    use std::collections::BTreeMap;

    use kanade::api::dto::logs::{Names, chat_turn};
    use kanade::domain::{members::Roster, model_log::ChatOutcome};

    let calls = json!([
        {"name": "get_boss_strategy", "round": 1, "arguments": "{\"boss\":\"Kalos\"}",
         "output": "Kalos strategy", "ms": 12, "outcome": "ok"},
        {"name": "get_schedule", "round": 1, "arguments": "", "output": "x", "ms": -3, "outcome": "ok"},
        {"name": "get_schedule", "round": 1, "arguments": "", "ms": 2.5, "outcome": "ok"},
    ]);
    let mut row = super::chat(
        "v4-c1",
        super::utc(9, 28, 12, 0),
        "1001",
        "kalos-four",
        "How do I do Kalos?",
        "Like this.",
        ChatOutcome::Answered,
        Some(1500),
        vec![super::round(
            "chat-model",
            &["get_boss_strategy", "get_schedule"],
            calls,
            Some("x"),
        )],
    );
    let (roster, channels) = (Roster::new(), BTreeMap::new());
    let names = Names {
        roster: &roster,
        channels: &channels,
    };
    let turn = chat_turn(&names, &row, &[], None, None);
    assert_valid(TURN, "v4 turn", &turn);
    let shown: Vec<_> = turn["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| (tool["result"].clone(), tool["took_ms"].clone()))
        .collect();
    assert_eq!(
        shown,
        [
            (json!("Kalos strategy"), json!(12)),
            (json!("x"), json!(null)),
            (json!(""), json!(null)),
        ]
    );

    row.withheld = true;
    let turn = chat_turn(&names, &row, &[], None, None);
    assert!(
        turn["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool["result"] == "[message withheld]"),
        "{turn}"
    );
    assert_eq!(turn["tools"][0]["took_ms"], 12);
}

/// A masked turn's detail carries its Model view: requests as sent, raw
/// replies and tool arguments, the final reply, and token → display name
/// (never a user id). Signed-in admin only; withheld turns hide it.
#[tokio::test]
async fn a_masked_turn_shows_its_model_view_with_names_never_ids() {
    use kanade::domain::model_log::{
        ChatOutcome, MaskedName, MaskedRound, MaskedTurn, ModelLogStore,
    };

    let logs = Logs::new().await;
    let view = MaskedTurn {
        rounds: vec![
            MaskedRound {
                round: 1,
                clean: false,
                request: json!([
                    {"role": "system", "content": "You are Kanade."},
                    {"role": "user", "content": "Haruka: when is kalos with <@Sora>?"},
                ]),
                reply: None,
                tool_calls: json!([{"name": "get_schedule", "arguments": "{\"participant\":\"<@Sora>\"}"}]),
            },
            // A clean retry stored with the loop's round number: the detail
            // numbers it by position, as `rounds[]` and `tools[]` do.
            MaskedRound {
                round: 1,
                clean: true,
                request: json!([{"role": "user", "content": "Haruka: when is kalos?"}]),
                reply: Some("Haruka, Sora is on Kalos.".into()),
                tool_calls: json!([]),
            },
        ],
        reply: "Alice, Bob is on Kalos.".into(),
        mapping: vec![
            MaskedName {
                token: "Haruka".into(),
                user_id: "1001".into(),
                display_name: Some("Alice".into()),
            },
            MaskedName {
                token: "Sora".into(),
                user_id: "1002".into(),
                display_name: None,
            },
            MaskedName {
                token: "Kaede".into(),
                user_id: "114200000000000099".into(),
                display_name: None,
            },
        ],
    };
    let mut row = super::chat(
        "c-masked",
        super::utc(9, 29, 3, 0),
        "1001",
        "kalos-four",
        "when is kalos with bob?",
        "Alice, Bob is on Kalos.",
        ChatOutcome::Answered,
        Some(2000),
        Vec::new(),
    );
    row.guardrail = json!({"pseudonymized": true});
    logs.reads
        .store
        .record_masked_chat(row.clone(), view.clone())
        .await
        .unwrap();

    let reply = logs.get("/api/admin/chat/c-masked").await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let turn = reply.json();
    assert_valid(TURN, "masked turn", &turn);
    assert_eq!(turn["masked"], true);
    let shown = &turn["model_view"];
    assert_eq!(shown["reply"], "Alice, Bob is on Kalos.");
    assert_eq!(shown["rounds"][0]["request"], view.rounds[0].request);
    assert_eq!(shown["rounds"][0]["tool_calls"], view.rounds[0].tool_calls);
    assert_eq!(shown["rounds"][1]["reply"], "Haruka, Sora is on Kalos.");
    assert_eq!(
        (
            shown["rounds"][1]["round"].clone(),
            shown["rounds"][1]["clean"].clone()
        ),
        (json!(2), json!(true))
    );
    assert_eq!(
        shown["mapping"],
        json!([
            {"token": "Haruka", "name": "Alice"},
            {"token": "Sora", "name": "Bobby"},
            {"token": "Kaede", "name": "someone"},
        ])
    );
    let text = shown.to_string();
    for id in ["1001", "1002", "114200000000000099", "user_id"] {
        assert!(!text.contains(id), "{id} in {text}");
    }

    // Signed out: no turn at all.
    let anonymous = crate::support::request(
        logs.reads.admin,
        "GET",
        crate::support::ADMIN_HOST,
        "/api/admin/chat/c-masked",
        &[],
    )
    .await;
    assert_eq!(anonymous.status, 401);

    // A withheld masked turn keeps the flag but not the view (it quotes the question).
    let mut withheld = row;
    withheld.id = "c-masked-withheld".into();
    withheld.withheld = true;
    logs.reads
        .store
        .record_masked_chat(withheld, view)
        .await
        .unwrap();
    let turn = logs.get("/api/admin/chat/c-masked-withheld").await.json();
    assert_valid(TURN, "withheld masked turn", &turn);
    assert_eq!(
        (turn["masked"].clone(), turn["model_view"].clone()),
        (json!(true), json!(null))
    );
}
