//! Live reroutes: a role's next permit follows its new alias and group,
//! permits already held (and their requeues) keep the old ones, a new
//! alias starts external, and only the open group grows.

use std::{sync::Arc, time::Duration};

use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, Effort, ExecutionLimits, FakeAction, FakeProvider,
    FinishReason, Message, RetryPolicy,
    governor::{GovernorConfig, ModelClient, Priority, QuestionLimits, Refused, Role},
};

use crate::support::{ALIAS, LONG, build, group, roles, single, ticket, wall};

#[tokio::test(start_paused = true)]
async fn the_next_permit_follows_the_new_alias_and_held_ones_keep_theirs() {
    let mut config = single(2, 6_000);
    config
        .groups
        .push(group("cloud", 1, 6_000, &["cloud-model"]));
    let governor = build(&config);
    let held = governor
        .acquire(Role::Chat, ticket(Priority::ChatNew, "a"), LONG)
        .await
        .unwrap();
    assert!(
        governor
            .reroute(Role::Chat, Some("cloud-model"), None)
            .unwrap()
    );
    assert!(
        !governor
            .reroute(Role::Chat, Some("cloud-model"), None)
            .unwrap()
    );
    let route = governor.route(Role::Chat).unwrap();
    assert_eq!(route.group.as_deref(), Some("cloud"));
    assert!(route.external, "fail closed until a listing");
    let next = governor
        .acquire(Role::Chat, ticket(Priority::ChatNew, "b"), LONG)
        .await
        .unwrap();
    assert_eq!((held.alias(), held.group()), (ALIAS, "local"));
    assert_eq!((next.alias(), next.group()), ("cloud-model", "cloud"));
    // Other roles are untouched; unrouting refuses the role's next permit.
    assert_eq!(governor.route(Role::Extraction).unwrap().alias, ALIAS);
    governor.reroute(Role::Chat, None, None).unwrap();
    assert_eq!(
        governor
            .acquire(Role::Chat, ticket(Priority::ChatNew, "c"), LONG)
            .await
            .unwrap_err(),
        Refused::UnknownRole
    );
}

#[tokio::test(start_paused = true)]
async fn only_the_open_group_takes_an_unlisted_alias() {
    let config = single(2, 6_000);
    let governor = build(&config);
    governor.reroute(Role::Chat, Some("stray"), None).unwrap();
    assert_eq!(
        governor
            .acquire(Role::Chat, ticket(Priority::ChatNew, "a"), LONG)
            .await
            .unwrap_err(),
        Refused::Ungrouped
    );
    let open = group("local", 2, 6_000, &[]);
    governor.reroute(Role::Chat, None, Some(&open)).unwrap();
    governor
        .reroute(Role::Chat, Some("stray"), Some(&open))
        .unwrap();
    let permit = governor
        .acquire(Role::Chat, ticket(Priority::ChatNew, "b"), LONG)
        .await
        .unwrap();
    assert_eq!(permit.group(), "local");
    let groups = governor.snapshot(wall());
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].models, [ALIAS, "stray"]);

    // A fresh governor with no groups creates the open one on first use.
    let empty = build(&GovernorConfig {
        groups: Vec::new(),
        roles: Default::default(),
        policy: Default::default(),
    });
    let open = group("gateway", 3, 6_000, &[]);
    empty
        .reroute(Role::Rewrite, Some("new"), Some(&open))
        .unwrap();
    let groups = empty.snapshot(wall());
    assert_eq!(
        (groups[0].name.as_str(), groups[0].permits.total),
        ("gateway", 3)
    );
    assert_eq!(groups[0].models, ["new"]);
    assert!(empty.reroute(Role::Chat, Some("  "), Some(&open)).is_err());
}

#[test]
fn zones_and_efforts_follow_each_routes_current_alias() {
    let mut config = single(1, 6_000);
    config.roles = roles(ALIAS, true);
    let governor = build(&config);
    governor.set_effort(Role::Chat, Some(Effort::High));
    governor.reroute(Role::Chat, Some("other"), None).unwrap();
    assert_eq!(
        governor.route(Role::Chat).unwrap().effort,
        Some(Effort::High),
        "kept until the setup pushes the new level"
    );
    governor.rederive_external(|alias| alias != ALIAS);
    assert!(!governor.route(Role::Extraction).unwrap().external);
    assert!(governor.route(Role::Chat).unwrap().external);
    assert!(governor.set_effort(Role::Chat, None));
    assert_eq!(governor.route(Role::Chat).unwrap().effort, None);
}

#[tokio::test(start_paused = true)]
async fn a_requeue_returns_to_the_alias_the_session_opened_with() {
    let mut config = single(1, 6_000);
    config.groups[0].burst = Some(1_000);
    config.policy.retry_floor = 10;
    config
        .groups
        .push(group("cloud", 1, 6_000, &["cloud-model"]));
    let governor = build(&config);
    let ok = FakeAction::Response(CompletionResponse {
        model: ALIAS.into(),
        content: Some("soon".into()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    });
    let provider = Arc::new(FakeProvider::new([
        FakeAction::AdmissionRefused(Some(Duration::from_secs(1))),
        ok,
    ]));
    let client = ModelClient::new(
        governor.clone(),
        provider.clone(),
        ExecutionLimits::default(),
        RetryPolicy {
            total_deadline: Duration::from_secs(30),
            max_attempts: 3,
            backoff: Duration::from_millis(100),
        },
    )
    .unwrap();
    let mut question = client
        .open_question(
            "member",
            false,
            QuestionLimits::new(Duration::from_secs(60)),
        )
        .await
        .unwrap();
    governor
        .reroute(Role::Chat, Some("cloud-model"), None)
        .unwrap();
    let request = ChatRequest {
        model: ALIAS.into(),
        messages: vec![Message::User {
            content: "when?".into(),
        }],
        tools: Vec::new(),
        output_schema: None,
        max_output_tokens: 16,
        reasoning: None,
        sampling: None,
    };
    question.complete(&request).await.unwrap();
    assert!(question.requeued());
    assert_eq!(question.alias(), Some(ALIAS));
    assert!(provider.requests().iter().all(|sent| sent.model == ALIAS));
}
