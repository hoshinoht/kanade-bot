//! The model-log storage every store must keep: the message cache, the
//! extraction and chat logs with their filters and keyset pages, rescan
//! jobs, allowance overrides and self-service tips. Each check runs against
//! a fresh store; failures panic with the check name.

use chrono::{DateTime, TimeZone, Utc};
use serde_json::json;

use crate::domain::model_log::{
    AllowanceOverride, ChatFilter, ChatInteraction, ChatOutcome, ChatRound, ExtractionFilter,
    ExtractionLog, ExtractionOutcome, ExtractionRefusal, LogFacets, MaskedName, MaskedRound,
    MaskedTurn, MessageUpsert, ModelLogStore, ReadMessage, RescanJob, RescanStatus, WatchedMessage,
};
use crate::domain::scheduler::StoreError;

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<S: ModelLogStore + Sync>(make: impl AsyncFn() -> S) {
    messages_cache_edits_and_windows(make().await).await;
    read_marks_skip_messages_edited_since(make().await).await;
    exact_read_marks_are_all_or_nothing(make().await).await;
    extraction_logs_round_trip_and_refuse_bad_rows(make().await).await;
    extraction_filters_combine_and_page(make().await).await;
    chat_logs_round_trip_with_rounds(make().await).await;
    token_usage_round_trips_and_pairs_are_whole(make().await).await;
    masked_chat_views_round_trip_and_prune(make().await).await;
    identity_leak_is_an_extraction_outcome(make().await).await;
    chat_filters_match_rounds_flags_and_latency(make().await).await;
    rescan_jobs_stop_changing_once_final(make().await).await;
    allowance_overrides_replace_and_clear(make().await).await;
    tips_are_claimed_once_per_member_and_week(make().await).await;
    retention_prunes_old_logs_and_processed_messages(make().await).await;
}

async fn retention_prunes_old_logs_and_processed_messages<S: ModelLogStore>(store: S) {
    let cutoff = utc(20, 0, 0);
    let just_before = cutoff - chrono::TimeDelta::microseconds(1);
    for (id, at) in [("x-old", just_before), ("x-edge", cutoff)] {
        store
            .record_extraction(extraction(id, at))
            .await
            .expect("record");
    }
    let mut old_chat = chat("c-old", just_before);
    old_chat.rounds = vec![round("kanata/old", &["old_tool"])];
    store.record_chat(old_chat).await.expect("record");
    store
        .record_chat(chat("c-edge", cutoff))
        .await
        .expect("record");
    for (id, at) in [
        ("m-old", just_before),
        ("m-pending", just_before),
        ("m-new", cutoff),
    ] {
        store
            .upsert_message(message(id, "900", at, "x"))
            .await
            .expect("message");
    }
    store
        .mark_processed(&["m-old".into(), "m-new".into()], cutoff)
        .await
        .expect("processed");
    assert_eq!(
        store.prune_model_logs(cutoff).await.expect("prune"),
        crate::domain::model_log::PruneCounts {
            extractions: 1,
            chats: 1,
            messages: 1,
            notices: 0,
        },
        "retention: strictly before the cutoff; unprocessed messages stay"
    );
    assert_eq!(store.load_extraction("x-old").await.expect("load"), None);
    assert!(
        store
            .load_extraction("x-edge")
            .await
            .expect("load")
            .is_some()
    );
    assert_eq!(store.load_chat("c-old").await.expect("load"), None);
    let facets = store.chat_facets().await.expect("facets");
    assert_eq!(facets.total, 1);
    assert_eq!(
        (facets.models, facets.tools),
        (vec!["kanata/chat".to_owned()], Vec::<String>::new()),
        "retention: a pruned chat's rounds and tools go with it"
    );
    let left = store
        .channel_messages("900", utc(1, 0, 0), false)
        .await
        .expect("messages");
    assert_eq!(ids(&left, |m| &m.id), ["m-pending", "m-new"]);
    assert_eq!(
        store.prune_model_logs(cutoff).await.expect("again"),
        crate::domain::model_log::PruneCounts::default()
    );
}

fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0)
        .single()
        .expect("valid instant")
}

fn message(id: &str, channel: &str, at: DateTime<Utc>, content: &str) -> WatchedMessage {
    WatchedMessage {
        id: id.into(),
        channel_id: channel.into(),
        author_id: "7".into(),
        created_at: at,
        edited_at: None,
        content: content.into(),
        processed_at: None,
    }
}

fn extraction(id: &str, at: DateTime<Utc>) -> ExtractionLog {
    ExtractionLog {
        id: id.into(),
        at,
        channel_id: Some("900".into()),
        member_ids: vec!["1".into(), "2".into()],
        model: "kanata/extract".into(),
        reasoning: Some("low".into()),
        prompt: "Messages: Lotus moved to Friday".into(),
        raw_response: "{\"changes\": []}".into(),
        latency_ms: Some(1200),
        request_count: 1,
        outcome: ExtractionOutcome::NoChange,
        error: None,
        guardrail: json!({}),
        message_ids: vec!["m-1".into()],
        proposal_ids: Vec::new(),
        refusals: Vec::new(),
        prompt_tokens: None,
        completion_tokens: None,
        prompt_estimate: None,
    }
}

fn round(model: &str, tools: &[&str]) -> ChatRound {
    ChatRound {
        model: model.into(),
        reasoning: None,
        finish_reason: Some("stop".into()),
        latency_ms: Some(300),
        tool_bundles: vec!["schedule".into()],
        tools: tools.iter().map(|tool| (*tool).to_owned()).collect(),
        tool_calls: json!([]),
        response: None,
        route: None,
        clean: false,
        prompt_tokens: None,
        completion_tokens: None,
        prompt_estimate: None,
    }
}

fn chat(id: &str, at: DateTime<Utc>) -> ChatInteraction {
    ChatInteraction {
        id: id.into(),
        at,
        channel_id: Some("900".into()),
        message_id: Some(format!("msg-{id}")),
        member_id: Some("1".into()),
        question: "When is Lotus?".into(),
        reply: "Friday at 20:00.".into(),
        outcome: ChatOutcome::Answered,
        error: None,
        clean_retry: false,
        withheld: false,
        guardrail: json!({}),
        request_count: 1,
        latency_ms: Some(900),
        model_ms: Some(800),
        tools_ms: Some(100),
        prompt_tokens: Some(1000),
        completion_tokens: Some(40),
        rounds: vec![round("kanata/chat", &[])],
        persona: None,
        profile: None,
        profile_source: None,
        error_code: None,
    }
}

fn ids<T>(items: &[T], id: impl Fn(&T) -> &str) -> Vec<String> {
    items.iter().map(|item| id(item).to_owned()).collect()
}

async fn read_marks_skip_messages_edited_since<S: ModelLogStore>(store: S) {
    let at = utc(20, 12, 0);
    for (id, text) in [("m-1", "Lotus 9pm?"), ("m-2", "Lotus ok")] {
        store
            .upsert_message(message(id, "900", at, text))
            .await
            .expect("insert");
    }
    let read = |id: &str, content: &str| ReadMessage {
        id: id.into(),
        content: content.into(),
    };
    // Edited while it was being read: the next read must see it.
    store
        .upsert_message(message("m-1", "900", at, "Lotus 10pm?"))
        .await
        .expect("edit");
    let marked = store
        .mark_read(
            &[
                read("m-1", "Lotus 9pm?"),
                read("m-2", "Lotus ok"),
                read("m-2", "Lotus ok"),
                read("absent", "x"),
            ],
            at,
        )
        .await
        .expect("mark");
    assert_eq!(marked, 1, "read marks: only unchanged rows, each once");
    let pending = store
        .channel_messages("900", utc(1, 0, 0), true)
        .await
        .expect("pending");
    assert_eq!(ids(&pending, |m| &m.id), ["m-1"], "read marks: edit kept");
    assert_eq!(
        store
            .mark_read(&[read("m-1", "Lotus 10pm?")], at)
            .await
            .expect("mark"),
        1,
        "read marks: the edited text, once read, is marked"
    );
}

async fn exact_read_marks_are_all_or_nothing<S: ModelLogStore>(store: S) {
    let at = utc(20, 12, 0);
    for (id, text) in [("m-1", "Lotus 9pm?"), ("m-2", "Lotus ok")] {
        store
            .upsert_message(message(id, "900", at, text))
            .await
            .expect("insert");
    }
    let read = |id: &str, content: &str| ReadMessage {
        id: id.into(),
        content: content.into(),
    };
    let pending = async || {
        let rows = store
            .channel_messages("900", utc(1, 0, 0), true)
            .await
            .expect("pending");
        ids(&rows, |m| &m.id)
    };
    store
        .upsert_message(message("m-1", "900", at, "Lotus 10pm?"))
        .await
        .expect("edit");
    // One stale version: nothing is marked, not even the unchanged row.
    assert!(
        !store
            .mark_read_exact(&[read("m-1", "Lotus 9pm?"), read("m-2", "Lotus ok")], at)
            .await
            .expect("mark"),
        "exact marks: a stale version refuses the whole set"
    );
    assert_eq!(
        pending().await,
        ["m-1", "m-2"],
        "exact marks: nothing written"
    );
    assert!(
        !store
            .mark_read_exact(&[read("m-2", "Lotus ok"), read("absent", "x")], at)
            .await
            .expect("mark"),
        "exact marks: a missing row refuses the whole set"
    );
    assert_eq!(pending().await, ["m-1", "m-2"]);
    // A repeated id counts once, its first read checked.
    assert!(
        store
            .mark_read_exact(
                &[
                    read("m-1", "Lotus 10pm?"),
                    read("m-2", "Lotus ok"),
                    read("m-2", "Lotus ok"),
                    read("m-1", "Lotus 9pm?"),
                ],
                at,
            )
            .await
            .expect("mark"),
        "exact marks: every current version marks"
    );
    assert!(pending().await.is_empty(), "exact marks: all written");
    // Marking again (a manual re-read) is allowed.
    assert!(
        store
            .mark_read_exact(&[read("m-2", "Lotus ok")], at)
            .await
            .expect("again")
    );
}

async fn messages_cache_edits_and_windows<S: ModelLogStore>(store: S) {
    let at = utc(20, 12, 0);
    assert_eq!(
        store
            .upsert_message(message("m-1", "900", at, "Lotus Friday?"))
            .await
            .expect("insert"),
        MessageUpsert::Inserted
    );
    store
        .upsert_message(message("m-2", "900", utc(20, 12, 5), "ok"))
        .await
        .expect("insert");
    store
        .upsert_message(message("m-3", "901", at, "elsewhere"))
        .await
        .expect("insert");
    store
        .upsert_message(message("m-0", "900", utc(19, 12, 0), "too old"))
        .await
        .expect("insert");
    assert_eq!(
        store
            .mark_processed(
                &["m-1".into(), "m-1".into(), "m-2".into(), "absent".into()],
                at
            )
            .await
            .expect("mark"),
        2,
        "messages: duplicates and unknown ids count once / not at all"
    );
    assert_eq!(
        store
            .upsert_message(message("m-1", "900", at, "Lotus Friday?"))
            .await
            .expect("same"),
        MessageUpsert::Unchanged
    );
    let mut edit = message("m-1", "999", utc(21, 0, 0), "Lotus Saturday?");
    edit.edited_at = Some(utc(20, 13, 0));
    assert_eq!(
        store.upsert_message(edit).await.expect("edit"),
        MessageUpsert::Edited
    );
    let window = store
        .channel_messages("900", utc(20, 0, 0), false)
        .await
        .expect("window");
    assert_eq!(ids(&window, |m| &m.id), ["m-1", "m-2"], "messages: window");
    let edited = &window[0];
    assert_eq!(edited.content, "Lotus Saturday?");
    assert_eq!(edited.edited_at, Some(utc(20, 13, 0)));
    assert_eq!(edited.created_at, at, "messages: an edit keeps created_at");
    assert_eq!(
        edited.channel_id, "900",
        "messages: an edit keeps the channel"
    );
    assert_eq!(edited.processed_at, None, "messages: an edit is read again");
    assert_eq!(window[1].processed_at, Some(at));
    let pending = store
        .channel_messages("900", utc(1, 0, 0), true)
        .await
        .expect("pending");
    assert_eq!(
        ids(&pending, |m| &m.id),
        ["m-0", "m-1"],
        "messages: pending"
    );
    let picked = store
        .messages_by_ids(&["m-3".into(), "absent".into(), "m-1".into(), "m-3".into()])
        .await
        .expect("by ids");
    assert_eq!(
        ids(&picked, |m| &m.id),
        ["m-3", "m-1"],
        "messages: by ids in the asked order, across channels, unknown ids and repeats skipped"
    );
    assert_eq!(picked[1].content, "Lotus Saturday?");
    assert!(store.messages_by_ids(&[]).await.expect("none").is_empty());
    assert!(store.delete_message("m-1").await.expect("delete"));
    assert!(!store.delete_message("m-1").await.expect("delete again"));
}

async fn extraction_logs_round_trip_and_refuse_bad_rows<S: ModelLogStore>(store: S) {
    let mut log = extraction("x-1", utc(20, 12, 0) + chrono::TimeDelta::microseconds(5));
    log.guardrail = json!({"content_filter": false, "prescreen": "clean"});
    log.proposal_ids = vec!["p-1".into()];
    log.outcome = ExtractionOutcome::Proposed;
    log.refusals = vec![ExtractionRefusal {
        change: "fix".into(),
        code: "no_recurring_slot".into(),
        message: "no recurring day and time were agreed - use `/fixed add`".into(),
    }];
    store.record_extraction(log.clone()).await.expect("record");
    assert_eq!(
        store.load_extraction("x-1").await.expect("load"),
        Some(log.clone()),
        "extractions: round trip"
    );
    assert!(
        matches!(
            store.record_extraction(log.clone()).await,
            Err(StoreError::Constraint(_))
        ),
        "extractions: a duplicate id is refused"
    );
    let mut bad = extraction("x-2", utc(20, 12, 0));
    bad.guardrail = json!(["not", "an", "object"]);
    assert!(
        matches!(
            store.record_extraction(bad).await,
            Err(StoreError::Constraint(_))
        ),
        "extractions: guardrail must be an object"
    );
    assert_eq!(store.load_extraction("x-2").await.expect("load"), None);
    assert_eq!(store.load_extraction("absent").await.expect("load"), None);
}

async fn extraction_filters_combine_and_page<S: ModelLogStore>(store: S) {
    let rows = [
        (
            "x-a",
            utc(20, 9, 0),
            "kanata/extract",
            ExtractionOutcome::Proposed,
            "900",
            "1",
        ),
        (
            "x-b",
            utc(21, 9, 0),
            "kanata/small",
            ExtractionOutcome::NoChange,
            "900",
            "2",
        ),
        (
            "x-c",
            utc(22, 9, 0),
            "kanata/extract",
            ExtractionOutcome::Failed,
            "901",
            "3",
        ),
        (
            "x-d",
            utc(22, 9, 0),
            "kanata/extract",
            ExtractionOutcome::TurnedAway,
            "900",
            "1",
        ),
        (
            "x-e",
            utc(23, 9, 0),
            "kanata/extract",
            ExtractionOutcome::Unknown,
            "902",
            "4",
        ),
    ];
    for (id, at, model, outcome, channel, member) in rows {
        let mut log = extraction(id, at);
        log.model = model.into();
        log.outcome = outcome;
        log.channel_id = Some(channel.into());
        log.member_ids = vec![member.into()];
        if id == "x-c" {
            log.raw_response = "HELLO Lotus".into();
        }
        store.record_extraction(log).await.expect("record");
    }
    let list = |filter: ExtractionFilter| {
        let store = &store;
        async move {
            let page = store.list_extractions(&filter).await.expect("list");
            (ids(&page.items, |log| &log.id), page.next)
        }
    };
    let all = ExtractionFilter {
        limit: 50,
        ..ExtractionFilter::default()
    };
    assert_eq!(
        list(all.clone()).await.0,
        ["x-e", "x-d", "x-c", "x-b", "x-a"],
        "extractions: newest first, id breaks ties"
    );
    let model = ExtractionFilter {
        model: Some("kanata/small".into()),
        ..all.clone()
    };
    assert_eq!(list(model).await.0, ["x-b"]);
    let range = ExtractionFilter {
        from: Some(utc(21, 9, 0)),
        to: Some(utc(23, 9, 0)),
        ..all.clone()
    };
    assert_eq!(
        list(range).await.0,
        ["x-d", "x-c", "x-b"],
        "extractions: from inclusive, to exclusive"
    );
    let outcomes = ExtractionFilter {
        outcomes: vec![ExtractionOutcome::Failed, ExtractionOutcome::Proposed],
        ..all.clone()
    };
    assert_eq!(
        list(outcomes).await.0,
        ["x-c", "x-a"],
        "extractions: any of"
    );
    let combined = ExtractionFilter {
        channel: Some("900".into()),
        member: Some("1".into()),
        ..all.clone()
    };
    assert_eq!(list(combined).await.0, ["x-d", "x-a"], "extractions: AND");
    let q = ExtractionFilter {
        q: Some("hello lotus".into()),
        ..all.clone()
    };
    assert_eq!(
        list(q.clone()).await.0,
        ["x-c"],
        "extractions: q folds ASCII case"
    );
    let light = store
        .list_extractions(&ExtractionFilter {
            omit_bodies: true,
            ..q
        })
        .await
        .expect("list");
    let mut full = store
        .load_extraction("x-c")
        .await
        .expect("load")
        .expect("x-c");
    assert_eq!(full.raw_response, "HELLO Lotus");
    full.prompt.clear();
    full.raw_response.clear();
    assert_eq!(
        light.items,
        [full],
        "extractions: the list projection drops only the bodies, and q still searches them"
    );
    let injection = ExtractionFilter {
        q: Some("') OR 1=1 --".into()),
        model: Some("x' OR '1'='1".into()),
        ..all.clone()
    };
    assert!(
        list(injection).await.0.is_empty(),
        "extractions: filter text is bound, never SQL"
    );
    // Keyset pages across the x-c/x-d tie.
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let (page, next) = list(ExtractionFilter {
            limit: 2,
            cursor: cursor.clone(),
            ..all.clone()
        })
        .await;
        assert!(page.len() <= 2);
        seen.extend(page);
        match next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(
        seen,
        ["x-e", "x-d", "x-c", "x-b", "x-a"],
        "extractions: pages cover every row once"
    );
    let (exact, next) = list(ExtractionFilter { limit: 5, ..all }).await;
    assert_eq!(exact.len(), 5);
    assert_eq!(next, None, "extractions: no cursor when nothing follows");
    assert_eq!(
        store.extraction_facets().await.expect("facets"),
        LogFacets {
            total: 5,
            models: vec!["kanata/extract".into(), "kanata/small".into()],
            tools: Vec::new(),
            outcomes: vec![
                "failed".into(),
                "no_change".into(),
                "proposed".into(),
                "turned_away".into(),
                "unknown".into(),
            ],
            channels: vec!["900".into(), "901".into(), "902".into()],
        }
    );
}

async fn chat_logs_round_trip_with_rounds<S: ModelLogStore>(store: S) {
    let mut interaction = chat("c-1", utc(20, 12, 0));
    interaction.rounds = vec![
        round("kanata/chat", &["runs_this_week", "runs_this_week"]),
        ChatRound {
            reasoning: Some("high".into()),
            finish_reason: Some("content_filter".into()),
            tool_calls: json!([{"name": "runs_this_week", "ms": 40}]),
            response: Some("{\"choices\": []}".into()),
            ..round("kanata/chat-big", &[])
        },
    ];
    interaction.guardrail = json!({"refusal": true});
    interaction.clean_retry = true;
    interaction.request_count = 3;
    interaction.persona = Some("kanade".into());
    interaction.profile = Some("gentle".into());
    interaction.profile_source = Some("saved".into());
    interaction.error_code = Some("timeout".into());
    interaction.rounds[0].route = Some("homelab".into());
    interaction.rounds[1].route = Some("external_masked".into());
    interaction.rounds[1].clean = true;
    store
        .record_chat(interaction.clone())
        .await
        .expect("record");
    assert_eq!(
        store.load_chat("c-1").await.expect("load"),
        Some(interaction.clone()),
        "chat: round trip keeps rounds in order"
    );
    assert!(matches!(
        store.record_chat(interaction.clone()).await,
        Err(StoreError::Constraint(_))
    ));
    let mut bad = chat("c-2", utc(20, 12, 0));
    bad.rounds[0].tool_calls = json!({});
    assert!(
        matches!(store.record_chat(bad).await, Err(StoreError::Constraint(_))),
        "chat: tool_calls must be an array"
    );
    assert_eq!(store.load_chat("c-2").await.expect("load"), None);
    let mut bad = chat("c-3", utc(20, 12, 0));
    bad.profile_source = Some("guessed".into());
    assert!(
        matches!(store.record_chat(bad).await, Err(StoreError::Constraint(_))),
        "chat: profile_source is saved, role or default"
    );
    let mut bad = chat("c-4", utc(20, 12, 0));
    bad.rounds[0].route = Some("cloud".into());
    assert!(
        matches!(store.record_chat(bad).await, Err(StoreError::Constraint(_))),
        "chat: a round's route is a known route"
    );
}

async fn token_usage_round_trips_and_pairs_are_whole<S: ModelLogStore>(store: S) {
    let mut reported = extraction("x-usage", utc(20, 12, 0));
    reported.prompt_tokens = Some(1200);
    reported.completion_tokens = Some(80);
    reported.prompt_estimate = Some(1100);
    let unreported = extraction("x-none", utc(20, 11, 0));
    let mut estimated = extraction("x-estimate", utc(20, 10, 0));
    estimated.prompt_estimate = Some(700);
    for log in [&reported, &unreported, &estimated] {
        store.record_extraction(log.clone()).await.expect("record");
        assert_eq!(
            store.load_extraction(&log.id).await.expect("load"),
            Some(log.clone()),
            "usage: extraction load round trip"
        );
    }
    let usage = |log: &ExtractionLog| {
        (
            log.prompt_tokens,
            log.completion_tokens,
            log.prompt_estimate,
        )
    };
    for omit_bodies in [false, true] {
        let page = store
            .list_extractions(&ExtractionFilter {
                omit_bodies,
                limit: 10,
                ..ExtractionFilter::default()
            })
            .await
            .expect("list");
        assert_eq!(
            page.items.iter().map(usage).collect::<Vec<_>>(),
            [
                (Some(1200), Some(80), Some(1100)),
                (None, None, None),
                (None, None, Some(700)),
            ],
            "usage: extraction list keeps usage (omit_bodies {omit_bodies})"
        );
    }
    for (n, (prompt, completion)) in [(Some(5), None), (None, Some(5))].into_iter().enumerate() {
        let mut half = extraction(&format!("x-half-{n}"), utc(20, 13, 0));
        half.prompt_tokens = prompt;
        half.completion_tokens = completion;
        assert!(
            matches!(
                store.record_extraction(half.clone()).await,
                Err(StoreError::Constraint(_))
            ),
            "usage: a half extraction pair is refused"
        );
        assert_eq!(store.load_extraction(&half.id).await.expect("load"), None);
    }
    assert_eq!(
        store.extraction_facets().await.expect("facets").total,
        3,
        "usage: a refused extraction writes nothing"
    );

    let mut interaction = chat("c-usage", utc(20, 12, 0));
    interaction.rounds = vec![
        ChatRound {
            prompt_tokens: Some(900),
            completion_tokens: Some(40),
            prompt_estimate: Some(950),
            ..round("kanata/chat", &[])
        },
        round("kanata/chat", &[]),
        ChatRound {
            prompt_estimate: Some(300),
            ..round("kanata/chat", &[])
        },
    ];
    store
        .record_chat(interaction.clone())
        .await
        .expect("record");
    assert_eq!(
        store.load_chat("c-usage").await.expect("load"),
        Some(interaction.clone()),
        "usage: chat round load round trip"
    );
    let page = store
        .list_chats(&ChatFilter {
            limit: 10,
            ..ChatFilter::default()
        })
        .await
        .expect("list");
    assert_eq!(
        page.items,
        [interaction],
        "usage: chat round list round trip"
    );
    for (n, (prompt, completion)) in [(Some(5), None), (None, Some(5))].into_iter().enumerate() {
        let mut half = chat(&format!("c-half-{n}"), utc(20, 13, 0));
        half.rounds.push(ChatRound {
            prompt_tokens: prompt,
            completion_tokens: completion,
            ..round("kanata/chat", &[])
        });
        assert!(
            matches!(
                store.record_chat(half.clone()).await,
                Err(StoreError::Constraint(_))
            ),
            "usage: a half round pair is refused"
        );
        assert_eq!(store.load_chat(&half.id).await.expect("load"), None);
    }
    assert_eq!(
        store.chat_facets().await.expect("facets").total,
        1,
        "usage: a refused chat writes nothing"
    );
    // Interaction totals are not a pair: v4 imports carry half of one.
    let mut totals = chat("c-totals", utc(20, 14, 0));
    totals.prompt_tokens = Some(1000);
    totals.completion_tokens = None;
    store
        .record_chat(totals.clone())
        .await
        .expect("half interaction totals are accepted");
    assert_eq!(
        store.load_chat("c-totals").await.expect("load"),
        Some(totals)
    );
}

fn masked() -> MaskedTurn {
    MaskedTurn {
        rounds: vec![
            MaskedRound {
                round: 1,
                clean: false,
                request: json!([{"role": "user", "content": "Haruka: when is Lotus?"}]),
                reply: None,
                tool_calls: json!([{"name": "get_schedule", "arguments": "{}"}]),
            },
            MaskedRound {
                round: 2,
                clean: true,
                request: json!([]),
                reply: Some("Haruka, it is Friday.".into()),
                tool_calls: json!([]),
            },
        ],
        reply: "Alice, it is Friday.".into(),
        mapping: vec![
            MaskedName {
                token: "Haruka".into(),
                user_id: "1".into(),
                display_name: Some("Alice".into()),
            },
            MaskedName {
                token: "Sora".into(),
                user_id: "2".into(),
                display_name: None,
            },
        ],
    }
}

async fn masked_chat_views_round_trip_and_prune<S: ModelLogStore>(store: S) {
    let cutoff = utc(20, 0, 0);
    let old = cutoff - chrono::TimeDelta::microseconds(1);
    store
        .record_masked_chat(chat("c-masked", old), masked())
        .await
        .expect("record");
    store
        .record_chat(chat("c-plain", cutoff))
        .await
        .expect("record");
    assert_eq!(
        store.load_masked_chat("c-masked").await.expect("load"),
        Some(masked()),
        "masked chat: round trip"
    );
    assert_eq!(
        store.load_chat("c-masked").await.expect("load"),
        Some(chat("c-masked", old)),
        "masked chat: the interaction is written with it"
    );
    assert_eq!(
        store.load_masked_chat("c-plain").await.expect("load"),
        None,
        "masked chat: a passthrough turn stores nothing extra"
    );
    let mut bad = masked();
    bad.rounds[0].request = json!({});
    assert!(
        matches!(
            store.record_masked_chat(chat("c-bad", old), bad).await,
            Err(StoreError::Constraint(_))
        ),
        "masked chat: the request must be an array"
    );
    assert_eq!(store.load_chat("c-bad").await.expect("load"), None);
    store.prune_model_logs(cutoff).await.expect("prune");
    assert_eq!(
        store.load_masked_chat("c-masked").await.expect("load"),
        None,
        "masked chat: pruned with its interaction"
    );
    assert_eq!(store.load_chat("c-masked").await.expect("load"), None);
    assert!(store.load_chat("c-plain").await.expect("load").is_some());
}

async fn identity_leak_is_an_extraction_outcome<S: ModelLogStore>(store: S) {
    let mut log = extraction("x-leak", utc(20, 12, 0));
    log.outcome = ExtractionOutcome::IdentityLeak;
    log.guardrail =
        json!({"identity_leak_blocked": {"role": "extraction", "kinds": ["name"], "count": 1}});
    store.record_extraction(log.clone()).await.expect("record");
    assert_eq!(
        store.load_extraction("x-leak").await.expect("load"),
        Some(log),
        "extractions: identity_leak round trip"
    );
    let page = store
        .list_extractions(&ExtractionFilter {
            outcomes: vec![ExtractionOutcome::IdentityLeak],
            ..ExtractionFilter::default()
        })
        .await
        .expect("list");
    assert_eq!(ids(&page.items, |log| &log.id), ["x-leak"]);
}

async fn chat_filters_match_rounds_flags_and_latency<S: ModelLogStore>(store: S) {
    let mut a = chat("c-a", utc(20, 9, 0));
    a.rounds = vec![
        round("kanata/chat", &["bosses"]),
        round("kanata/big", &["runs"]),
    ];
    a.latency_ms = Some(5000);
    let mut b = chat("c-b", utc(21, 9, 0));
    b.outcome = ChatOutcome::Answered;
    b.clean_retry = true;
    b.latency_ms = None;
    let mut c = chat("c-c", utc(21, 9, 0));
    c.outcome = ChatOutcome::ContentBlocked;
    c.withheld = true;
    c.member_id = Some("2".into());
    c.question = "Something RUDE".into();
    let mut d = chat("c-d", utc(22, 9, 0));
    d.outcome = ChatOutcome::TurnedAway;
    d.channel_id = None;
    d.rounds = Vec::new();
    d.latency_ms = Some(10);
    for interaction in [a, b, c, d] {
        store.record_chat(interaction).await.expect("record");
    }
    let list = |filter: ChatFilter| {
        let store = &store;
        async move {
            let page = store.list_chats(&filter).await.expect("list");
            ids(&page.items, |chat| &chat.id)
        }
    };
    let all = ChatFilter {
        limit: 10,
        ..ChatFilter::default()
    };
    assert_eq!(list(all.clone()).await, ["c-d", "c-c", "c-b", "c-a"]);
    assert_eq!(
        list(ChatFilter {
            model: Some("kanata/big".into()),
            ..all.clone()
        })
        .await,
        ["c-a"],
        "chat: model matches any round"
    );
    assert_eq!(
        list(ChatFilter {
            tool: Some("runs".into()),
            ..all.clone()
        })
        .await,
        ["c-a"],
        "chat: tool matches any round"
    );
    assert_eq!(
        list(ChatFilter {
            min_ms: Some(900),
            ..all.clone()
        })
        .await,
        ["c-c", "c-a"],
        "chat: min_ms skips unknown latency"
    );
    assert_eq!(
        list(ChatFilter {
            outcomes: vec![ChatOutcome::CleanRetry, ChatOutcome::Withheld],
            ..all.clone()
        })
        .await,
        ["c-c", "c-b"],
        "chat: flags match their outcome names"
    );
    assert_eq!(
        list(ChatFilter {
            outcomes: vec![ChatOutcome::TurnedAway],
            ..all.clone()
        })
        .await,
        ["c-d"]
    );
    assert_eq!(
        list(ChatFilter {
            member: Some("2".into()),
            q: Some("rude".into()),
            ..all.clone()
        })
        .await,
        ["c-c"]
    );
    assert_eq!(
        list(ChatFilter {
            channel: Some("900".into()),
            from: Some(utc(21, 9, 0)),
            ..all.clone()
        })
        .await,
        ["c-c", "c-b"]
    );
    let first = store
        .list_chats(&ChatFilter {
            limit: 2,
            ..all.clone()
        })
        .await
        .expect("page");
    assert_eq!(ids(&first.items, |chat| &chat.id), ["c-d", "c-c"]);
    let second = store
        .list_chats(&ChatFilter {
            limit: 2,
            cursor: first.next.clone(),
            ..all.clone()
        })
        .await
        .expect("page");
    assert_eq!(
        ids(&second.items, |chat| &chat.id),
        ["c-b", "c-a"],
        "chat: the cursor resumes inside a tie"
    );
    assert_eq!(second.next, None);
    assert_eq!(
        second.items[1].rounds.len(),
        2,
        "chat: listed items carry their rounds"
    );
    for listed in first.items.iter().chain(&second.items) {
        assert_eq!(
            Some(listed),
            store.load_chat(&listed.id).await.expect("load").as_ref(),
            "chat: a listed interaction is the stored one, rounds in order"
        );
    }
    assert_eq!(
        store.chat_facets().await.expect("facets"),
        LogFacets {
            total: 4,
            models: vec!["kanata/big".into(), "kanata/chat".into()],
            tools: vec!["bosses".into(), "runs".into()],
            outcomes: vec![
                "answered".into(),
                "clean_retry".into(),
                "content_blocked".into(),
                "turned_away".into(),
                "withheld".into(),
            ],
            channels: vec!["900".into()],
        }
    );
}

fn rescan(id: &str, at: DateTime<Utc>) -> RescanJob {
    RescanJob {
        id: id.into(),
        channels: vec!["900".into(), "901".into()],
        window: "week".into(),
        source: "manual".into(),
        automated: false,
        requested_by: Some("root".into()),
        status: RescanStatus::Queued,
        created_at: at,
        started_at: None,
        finished_at: None,
        results: json!([]),
        error: None,
    }
}

async fn rescan_jobs_stop_changing_once_final<S: ModelLogStore>(store: S) {
    store
        .insert_rescan_job(rescan("r-1", utc(20, 9, 0)))
        .await
        .expect("insert");
    store
        .insert_rescan_job(rescan("r-2", utc(21, 9, 0)))
        .await
        .expect("insert");
    assert!(matches!(
        store.insert_rescan_job(rescan("r-1", utc(22, 9, 0))).await,
        Err(StoreError::Constraint(_))
    ));
    let mut running = rescan("r-1", utc(20, 9, 0));
    running.status = RescanStatus::Running;
    running.started_at = Some(utc(20, 9, 1));
    assert!(store.update_rescan_job(running.clone()).await.expect("run"));
    let mut done = running.clone();
    done.status = RescanStatus::Done;
    done.finished_at = Some(utc(20, 9, 5));
    done.results = json!([{"channel": "900", "messages": 12}]);
    assert!(store.update_rescan_job(done.clone()).await.expect("done"));
    let mut again = done.clone();
    again.status = RescanStatus::Running;
    assert!(
        !store.update_rescan_job(again).await.expect("final"),
        "rescan: a final job never changes"
    );
    assert!(
        !store
            .update_rescan_job(rescan("absent", utc(20, 9, 0)))
            .await
            .expect("missing")
    );
    assert_eq!(
        store.load_rescan_job("r-1").await.expect("load"),
        Some(done)
    );
    let recent = store.recent_rescan_jobs(1).await.expect("recent");
    assert_eq!(ids(&recent, |job| &job.id), ["r-2"], "rescan: newest first");
}

async fn allowance_overrides_replace_and_clear<S: ModelLogStore>(store: S) {
    let entry = |member: &str, count| AllowanceOverride {
        member_id: member.into(),
        count,
        window_ms: 300_000,
        updated_at: utc(20, 9, 0),
    };
    store
        .set_allowance_override(entry("2", 4))
        .await
        .expect("set");
    store
        .set_allowance_override(entry("1", 4))
        .await
        .expect("set");
    store
        .set_allowance_override(entry("2", 10))
        .await
        .expect("replace");
    assert_eq!(
        store.allowance_overrides().await.expect("list"),
        [entry("1", 4), entry("2", 10)]
    );
    let mut zero = entry("3", 1);
    zero.window_ms = 0;
    assert!(matches!(
        store.set_allowance_override(zero).await,
        Err(StoreError::Constraint(_))
    ));
    assert!(store.clear_allowance_override("1").await.expect("clear"));
    assert!(
        !store
            .clear_allowance_override("1")
            .await
            .expect("clear again")
    );
    assert_eq!(
        store.allowance_overrides().await.expect("list"),
        [entry("2", 10)]
    );
}

async fn tips_are_claimed_once_per_member_and_week<S: ModelLogStore + Sync>(store: S) {
    let week = utc(17, 0, 0);
    let claims = concurrent_claims(&store, week).await;
    assert_eq!(
        claims.iter().filter(|granted| **granted).count(),
        1,
        "tips: concurrent claims grant exactly one"
    );
    assert!(
        store
            .claim_tip("2", week, utc(20, 9, 0))
            .await
            .expect("other member")
    );
    assert!(
        store
            .claim_tip("1", utc(24, 0, 0), utc(24, 9, 0))
            .await
            .expect("next week")
    );
    assert!(
        !store
            .claim_tip("1", week, utc(21, 9, 0))
            .await
            .expect("again")
    );
    assert!(store.release_tip("1", week).await.expect("release"));
    assert!(
        !store.release_tip("1", week).await.expect("release again"),
        "tips: a release gives back only a held tip"
    );
    assert!(
        store
            .claim_tip("1", week, utc(21, 9, 0))
            .await
            .expect("after release"),
        "tips: a released tip can be claimed again"
    );
}

async fn concurrent_claims<S: ModelLogStore + Sync>(store: &S, week: DateTime<Utc>) -> Vec<bool> {
    let (a, b, c) = tokio::join!(
        store.claim_tip("1", week, utc(20, 9, 0)),
        store.claim_tip("1", week, utc(20, 9, 1)),
        store.claim_tip("1", week, utc(20, 9, 2)),
    );
    vec![a.expect("claim"), b.expect("claim"), c.expect("claim")]
}
