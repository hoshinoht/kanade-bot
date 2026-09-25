//! A7 logs and rescans against the seeded store of `reads.rs` (pinned clock
//! Tue 29 Sep 12:00 KL; boss weeks reset Thursday 00:00). Chat and
//! extraction rows are recorded straight into the store; rescans run on
//! [`fake::FakeRescans`]. Every response is validated against the schemas.

mod chat;
mod extractions;
pub mod fake;
mod rescan;

use std::sync::Arc;

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::{
    api::write::ApiClock,
    domain::{
        drafts::ProposalSource,
        ids::RandomIds,
        members::{MemberStore, Roster},
        model_log::{
            ChatInteraction, ChatOutcome, ChatRound, ExtractionLog, ExtractionOutcome,
            ExtractionRefusal, ModelLogStore, WatchedMessage,
        },
        proposals::{CardDetails, CardPayload, ChangeKind, ProposalCardStore, ProposedChange},
        schedule::{ReminderPolicy, SchedulePolicy},
        scheduler::{ProposalRequest, SchedulerService, Supersede},
    },
};
use serde_json::{Value, json};

use crate::{
    reads::Reads,
    support::{ADMIN_HOST, Reply, request, send},
};

pub const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
pub const ERROR: &str = "error.json#/$defs/ApiError";

pub fn utc(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
}

pub fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::Asia::Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

pub struct Logs {
    pub reads: Reads,
    /// The extractor's live proposal, with a card.
    pub proposal: String,
}

fn round(model: &str, tools: &[&str], calls: Value, response: Option<&str>) -> ChatRound {
    ChatRound {
        model: model.into(),
        reasoning: None,
        finish_reason: Some(
            if tools.is_empty() {
                "stop"
            } else {
                "tool_calls"
            }
            .into(),
        ),
        latency_ms: Some(1000),
        tool_bundles: Vec::new(),
        tools: tools.iter().map(|tool| (*tool).to_owned()).collect(),
        tool_calls: calls,
        response: response.map(str::to_owned),
    }
}

#[allow(clippy::too_many_arguments)]
fn chat(
    id: &str,
    at: DateTime<Utc>,
    member: &str,
    channel: &str,
    question: &str,
    reply: &str,
    outcome: ChatOutcome,
    latency_ms: Option<u64>,
    rounds: Vec<ChatRound>,
) -> ChatInteraction {
    ChatInteraction {
        id: id.into(),
        at,
        channel_id: Some(channel.into()),
        message_id: Some(format!("msg-{id}")),
        member_id: Some(member.into()),
        question: question.into(),
        reply: reply.into(),
        outcome,
        error: None,
        clean_retry: false,
        withheld: false,
        guardrail: json!({}),
        request_count: u32::try_from(rounds.len()).unwrap(),
        latency_ms,
        model_ms: None,
        tools_ms: None,
        prompt_tokens: None,
        completion_tokens: None,
        rounds,
    }
}

pub fn extraction(
    id: &str,
    at: DateTime<Utc>,
    channel: &str,
    outcome: ExtractionOutcome,
) -> ExtractionLog {
    ExtractionLog {
        id: id.into(),
        at,
        channel_id: Some(channel.into()),
        member_ids: Vec::new(),
        model: "kanata/extract".into(),
        reasoning: None,
        prompt: String::new(),
        raw_response: String::new(),
        latency_ms: Some(8000),
        request_count: 1,
        outcome,
        error: None,
        guardrail: json!({}),
        message_ids: Vec::new(),
        proposal_ids: Vec::new(),
        refusals: Vec::new(),
    }
}

async fn seed_chats(reads: &Reads) {
    let store = &reads.store;
    let answered = chat(
        "c-answer",
        utc(9, 28, 12, 0),
        "1001",
        "kalos-four",
        "When is Kalos?",
        "Tuesday 22:00.",
        ChatOutcome::Answered,
        Some(4000),
        vec![
            round(
                "kanata/chat",
                &["schedule_read"],
                json!([{"name": "schedule_read", "outcome": "ok", "arguments": {"week": "this"},
                        "created": [], "posted": [], "result": "Kalos: Tue 22:00", "took_ms": 12}]),
                Some(""),
            ),
            round("kanata/chat", &[], json!([]), Some("Tuesday 22:00.")),
        ],
    );
    let mut withheld = chat(
        "c-withheld",
        utc(9, 27, 12, 0),
        "1002",
        "kalos-four",
        "say the forbidden thing",
        "I can't help with that one.",
        ChatOutcome::ContentBlocked,
        Some(9000),
        vec![round(
            "kanata/chat-cloud",
            &["propose_move"],
            json!([{"name": "propose_move", "outcome": "ok",
                    "arguments": {"note": "the forbidden thing"}, "created": [], "posted": [],
                    "result": "moved: the forbidden thing", "took_ms": 5}]),
            Some("the forbidden thing, echoed"),
        )],
    );
    withheld.withheld = true;
    withheld.guardrail = json!({"content_filter": true});
    let limited = chat(
        "c-limited",
        utc(9, 20, 12, 0),
        "1004",
        "limbo-trio",
        "and normal?",
        "You've used this window's questions.",
        ChatOutcome::RateLimited,
        None,
        Vec::new(),
    );
    let mut timeout = chat(
        "c-timeout",
        utc(9, 29, 1, 0),
        "1001",
        "star",
        "summarise the week",
        "",
        ChatOutcome::Timeout,
        Some(60_000),
        vec![round("kanata/chat", &[], json!([]), None)],
    );
    timeout.clean_retry = true;
    for row in [answered, withheld, limited, timeout] {
        store.record_chat(row).await.unwrap();
    }
}

async fn seed_proposal(reads: &Reads) -> String {
    let store = &reads.store;
    let mut directory = Roster::new();
    for profile in store.list_members().await.unwrap() {
        directory.upsert(profile.member);
    }
    directory.watch("kalos-four");
    let at = utc(9, 29, 2, 0);
    let mut service =
        SchedulerService::new(store.clone(), RandomIds, ApiClock(Arc::new(move || at)));
    let proposal = service
        .propose(
            ProposalRequest {
                change: ProposedChange {
                    run_id: Some("r-kalos".into()),
                    channel_id: Some("kalos-four".into()),
                    new_datetime: Some(utc(9, 30, 13, 0)),
                    ..ProposedChange::new(ChangeKind::Move)
                },
                source: ProposalSource::Extraction,
                source_id: "x-new".into(),
                supersede: Supersede::Keep,
            },
            &policy(),
            &directory,
        )
        .await
        .unwrap()
        .proposal
        .id;
    store
        .save_card(
            &proposal,
            "kalos-four",
            &CardDetails {
                kind: ChangeKind::Move,
                run_id: Some("r-kalos".into()),
                bosses: vec!["XKalos".into()],
                participants: vec!["1001".into()],
                new_datetime: Some(utc(9, 30, 13, 0)),
                day_ref: Some("wed".into()),
                time_ref: Some("9pm".into()),
                rsvp: None,
                is_question: false,
                summary: None,
                also_mentioned: Vec::new(),
                confidence: 0.75,
                payload: CardPayload::default(),
                evidence_message_ids: Vec::new(),
                self_service: None,
            },
            at,
        )
        .await
        .unwrap();
    proposal
}

async fn seed_extractions(reads: &Reads, proposal: &str) {
    let store = &reads.store;
    store
        .upsert_message(WatchedMessage {
            id: "m-said".into(),
            channel_id: "kalos-four".into(),
            author_id: "1001".into(),
            created_at: utc(9, 29, 1, 50),
            edited_at: None,
            content: "kalos wed 9pm instead?".into(),
            processed_at: None,
        })
        .await
        .unwrap();
    let mut newest = extraction(
        "x-new",
        utc(9, 29, 2, 0),
        "kalos-four",
        ExtractionOutcome::Proposed,
    );
    newest.member_ids = vec!["1001".into()];
    newest.prompt = "Messages:\n[Alice] kalos wed 9pm instead?".into();
    newest.raw_response = r#"{"amendments": []}"#.into();
    newest.latency_ms = Some(12_000);
    newest.message_ids = vec!["m-said".into(), "m-pruned".into()];
    newest.proposal_ids = vec![proposal.to_owned(), "p-missing".into()];
    newest.refusals = vec![ExtractionRefusal {
        change: "move".into(),
        code: "past".into(),
        message: "That time has already passed.".into(),
    }];
    let mut failed = extraction(
        "x-fail",
        utc(9, 26, 2, 0),
        "star",
        ExtractionOutcome::Failed,
    );
    failed.member_ids = vec!["1002".into()];
    failed.model = "kanata/legacy".into();
    failed.prompt = "Messages:\n[Bob] carling tonight".into();
    failed.latency_ms = None;
    failed.error = Some("no answer".into());
    let mut old = extraction(
        "x-old",
        utc(9, 19, 2, 0),
        "limbo-trio",
        ExtractionOutcome::NoChange,
    );
    old.member_ids = vec!["1003".into(), "1001".into()];
    for row in [newest, failed, old] {
        store.record_extraction(row).await.unwrap();
    }
}

impl Logs {
    pub async fn new() -> Self {
        let reads = Reads::new().await;
        seed_chats(&reads).await;
        let proposal = seed_proposal(&reads).await;
        seed_extractions(&reads, &proposal).await;
        Self { reads, proposal }
    }

    pub async fn get(&self, path: &str) -> Reply {
        request(
            self.reads.admin,
            "GET",
            ADMIN_HOST,
            path,
            &[("Cookie", &self.reads.cookie)],
        )
        .await
    }

    /// A signed-in, CSRF-carrying write, optionally keyed.
    pub async fn write(
        &self,
        method: &str,
        path: &str,
        key: Option<&str>,
        body: Option<&str>,
    ) -> Reply {
        let mut headers = vec![
            ORIGIN,
            ("Cookie", self.reads.cookie.as_str()),
            ("X-Kanade-CSRF", self.reads.csrf.as_str()),
        ];
        if let Some(key) = key {
            headers.push(("Idempotency-Key", key));
        }
        send(self.reads.admin, method, ADMIN_HOST, path, &headers, body).await
    }
}

/// Row ids of a list response.
pub fn ids(value: &Value) -> Vec<String> {
    value["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_str().unwrap().to_owned())
        .collect()
}
