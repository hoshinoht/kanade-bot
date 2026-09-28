//! Extraction through the real runner and a governed session: the reply comes
//! back unvalidated (`CallerValidates`), extraction's own parse/coerce logic
//! judges it, and one answer retry follows a rejected reply.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use kanade::extract::AmendmentKind;
use kanade::extract::prompt::extraction_request;
use kanade::extract::schema::{AttemptOutcome, ExtractionAttempts, ExtractionCall, Next};
use kanade::infrastructure::llm::governor::{
    Governor, GovernorConfig, GovernorPolicy, GroupConfig, ModelClient, Random, Role, RoleConfig,
};
use kanade::infrastructure::llm::{
    CompletionResponse, ExecutionLimits, FakeAction, FakeProvider, FinishReason, Message,
    OutputValidation, RetryPolicy, Usage,
};

const ALIAS: &str = "extractor";
const TIMEOUT: Duration = Duration::from_secs(60);

struct Fixed;

impl Random for Fixed {
    fn next_u64(&self) -> u64 {
        1 << 63
    }
}

/// One permit, no rate waits, over a provider answering `replies` in order.
fn governed(replies: &[&str]) -> (Arc<Governor>, Arc<FakeProvider>, ModelClient<FakeProvider>) {
    let config = GovernorConfig {
        groups: vec![GroupConfig {
            name: "local".into(),
            backend: "local backend".into(),
            permits: 1,
            requests_per_min: 6_000,
            burst: Some(1_000),
            aliases: vec![ALIAS.into()],
        }],
        roles: [Role::Chat, Role::Extraction, Role::Rewrite]
            .into_iter()
            .map(|role| {
                let route = RoleConfig {
                    alias: ALIAS.into(),
                    external: false,
                };
                (role, route)
            })
            .collect::<BTreeMap<_, _>>(),
        policy: GovernorPolicy::default(),
    };
    let governor = Arc::new(Governor::new(&config, Arc::new(Fixed)).expect("valid config"));
    let actions = replies.iter().map(|content| {
        FakeAction::Response(CompletionResponse {
            model: ALIAS.into(),
            content: Some((*content).to_owned()),
            tool_calls: Vec::new(),
            finish_reason: FinishReason::Stop,
            usage: Some(Usage {
                prompt_tokens: 1,
                completion_tokens: 1,
            }),
        })
    });
    let provider = Arc::new(FakeProvider::new(actions));
    let retry = RetryPolicy {
        total_deadline: Duration::from_secs(30),
        max_attempts: 3,
        backoff: Duration::from_millis(100),
    };
    let client = ModelClient::new(
        governor.clone(),
        provider.clone(),
        ExecutionLimits::default(),
        retry,
    )
    .expect("valid client");
    (governor, provider, client)
}

fn messages() -> Vec<Message> {
    vec![
        Message::System {
            content: "SYSTEM".into(),
        },
        Message::User {
            content: "[1] [2026-08-30 13:01 Sun] [Alvin tan <@11>] hstar wed?".into(),
        },
    ]
}

/// What the orchestrator does: `complete`, then `answer_retry` on `Next::Retry`.
async fn extract(client: &ModelClient<FakeProvider>) -> ExtractionCall {
    let mut session = client
        .open_extraction("burst", TIMEOUT, TIMEOUT)
        .await
        .expect("permit");
    let mut attempts = ExtractionAttempts::new(messages());
    let mut first = true;
    loop {
        let request = extraction_request(ALIAS, attempts.messages().to_vec(), None);
        let sent = if first {
            session.complete(&request).await
        } else {
            session.answer_retry(&request).await
        };
        first = false;
        let outcome = match sent {
            Ok(response) => AttemptOutcome::Reply {
                content: response.content,
                reasoning: None,
            },
            Err(error) => AttemptOutcome::Failed {
                detail: error.to_string(),
            },
        };
        match attempts.record(outcome) {
            Next::Retry => continue,
            Next::Done(call) => return call,
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_v4_coercible_reply_is_accepted_through_the_runner() {
    let reply = r#"{"amendments": {"kind": " Move ", "bosses": "HStar", "confidence": 82,
        "is_question": "yes", "participants": ["<@11>"], "day_ref": "wed"}}"#;
    let (_, provider, client) = governed(&[reply]);
    let call = extract(&client).await;
    assert_eq!(call.attempts, 1, "{:?}", call.error);
    let amendment = &call.extraction.expect("accepted").amendments[0];
    assert_eq!(amendment.kind, AmendmentKind::Move);
    assert_eq!(amendment.bosses, ["HStar"]);
    assert_eq!(amendment.confidence, 0.82);
    assert!(amendment.is_question);
    assert_eq!(amendment.participants, ["11"]);

    let sent = provider.requests();
    let output = sent[0].output_schema.as_ref().expect("schema sent");
    assert!(output.strict);
    assert_eq!(output.validation, OutputValidation::CallerValidates);
}

#[tokio::test(start_paused = true)]
async fn a_malformed_reply_gets_one_answer_retry_in_the_same_session() {
    let good = r#"{"amendments":[{"kind":"move","bosses":["HMaleficStar"],"day_ref":"wed"}],"summary":"move"}"#;
    let (governor, provider, client) = governed(&["not json", good]);
    let call = extract(&client).await;
    assert!(call.ok(), "{:?}", call.error);
    assert_eq!(call.attempts, 2);

    let sent = provider.requests();
    assert_eq!(sent.len(), 2);
    let retry = &sent[1].messages;
    assert_eq!(retry.len(), 4);
    assert_eq!(
        retry[2],
        Message::Assistant {
            content: Some("not json".into()),
            tool_calls: Vec::new(),
        }
    );
    let Message::User { content } = &retry[3] else {
        panic!("the retry instruction is a user turn");
    };
    assert!(content.starts_with("Your previous answer did not fit the schema:\nnot JSON: "));
    let wall = chrono::DateTime::from_timestamp(1_790_000_000, 0).expect("timestamp");
    let counters = &governor.snapshot(wall)[0].counters;
    assert_eq!(
        (counters.requests, counters.retries),
        (2, 0),
        "a content retry spends no retry budget"
    );
}
