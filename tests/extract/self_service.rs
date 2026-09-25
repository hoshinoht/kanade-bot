//! Self-service redirect wired end to end (N1): the governed rewrite adapter
//! over the fake provider, and the pipeline posting links, the weekly lead-in
//! and its log label under each mode, with today's cards-only behaviour while
//! the public portal is closed.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use kanade::chat::nudge::{
    GovernedRewriter, LineSource, NudgeFacts, NudgeRewriter, Nudger, RewriteFailure, RewritePrompt,
    SeedReason,
};
use kanade::chat::persona::{NudgeMood, NudgePurpose};
use kanade::domain::model_log::{ExtractionOutcome, ModelLogStore};
use kanade::domain::notify::WeekReset;
use kanade::extract::AmendmentKind;
use kanade::extract::pipeline::MessageEvent;
use kanade::extract::redirect::SelfServiceMode;
use kanade::infrastructure::llm::governor::{
    CallKind, Governor, GovernorConfig, GovernorPolicy, GroupConfig, ModelClient, Random, Role,
    RoleConfig,
};
use kanade::infrastructure::llm::identity::{Member, Passthrough, find_request_leaks};
use kanade::infrastructure::llm::{
    ExecutionLimits, FakeAction, FakeProvider, Message, RetryPolicy,
};
use serde_json::json;
use tokio::time::Instant;

use crate::fakes::{
    ALIAS, CHANNEL, MY, PORTAL, World, after, client, filtered, kanade, local, message, now, reply,
};

struct Fixed;

impl Random for Fixed {
    fn next_u64(&self) -> u64 {
        0
    }
}

fn rewrite_prompt() -> RewritePrompt {
    RewritePrompt::build(
        &kanade(),
        NudgeMood::Playful,
        "Hmph. Everything you need is right here.",
    )
}

fn adapter(client: Arc<ModelClient<FakeProvider>>) -> GovernedRewriter<FakeProvider> {
    GovernedRewriter::new(client, Arc::new(Passthrough))
}

/// A client whose rewrite role is `rewrite`: grouped or not, local or external.
fn rewrite_client(
    grouped: bool,
    external: bool,
    actions: Vec<FakeAction>,
) -> (Arc<FakeProvider>, Arc<ModelClient<FakeProvider>>) {
    let rewrite_alias = if grouped { ALIAS } else { "nowhere" };
    let config = GovernorConfig {
        groups: vec![GroupConfig {
            name: "local".into(),
            backend: "local backend".into(),
            permits: 1,
            requests_per_min: 6_000,
            burst: Some(1_000),
            aliases: vec![ALIAS.into()],
        }],
        roles: [
            (Role::Chat, ALIAS, false),
            (Role::Extraction, ALIAS, false),
            (Role::Rewrite, rewrite_alias, external),
        ]
        .into_iter()
        .map(|(role, alias, external)| {
            let route = RoleConfig {
                alias: alias.into(),
                external,
            };
            (role, route)
        })
        .collect::<BTreeMap<_, _>>(),
        policy: GovernorPolicy::default(),
    };
    let governor = Arc::new(Governor::new(&config, Arc::new(Fixed)).expect("config"));
    let provider = Arc::new(FakeProvider::new(actions));
    let retry = RetryPolicy {
        total_deadline: Duration::from_secs(30),
        max_attempts: 3,
        backoff: Duration::from_millis(100),
    };
    let client = ModelClient::new(
        governor,
        provider.clone(),
        ExecutionLimits::default(),
        retry,
    )
    .expect("client");
    (provider, Arc::new(client))
}

const DEADLINE: Duration = Duration::from_secs(2);

#[tokio::test(start_paused = true)]
async fn the_adapter_sends_one_plain_request_and_returns_the_line() {
    let (provider, client) = client(vec![reply("Fine. Fix it yourself.")], true);
    let rewritten = adapter(client).rewrite(&rewrite_prompt(), DEADLINE).await;
    assert_eq!(rewritten.as_deref(), Ok("Fine. Fix it yourself."));
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.model, ALIAS);
    assert!(request.output_schema.is_none() && request.tools.is_empty());
    assert!(matches!(
        request.messages.as_slice(),
        [Message::System { .. }, Message::User { content }]
            if content == "Hmph. Everything you need is right here."
    ));
    let roster = [Member {
        user_id: MY.into(),
        display_name: "MY".into(),
        nickname: None,
        aliases: Vec::new(),
    }];
    assert!(find_request_leaks(request, &roster).is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_content_filter_or_an_empty_reply_is_a_refusal_and_gives_the_seed() {
    let (provider, client) = client(vec![filtered(), reply("   ")], true);
    let adapter = adapter(client);
    assert_eq!(
        adapter.rewrite(&rewrite_prompt(), DEADLINE).await,
        Err(RewriteFailure::Refused)
    );
    assert_eq!(
        adapter.rewrite(&rewrite_prompt(), DEADLINE).await,
        Err(RewriteFailure::Refused)
    );
    assert_eq!(provider.requests().len(), 2, "one request each, no retry");

    let (_, client) = crate::fakes::client(vec![filtered()], true);
    let nudge = Nudger::new(
        Arc::new(Fixed),
        GovernedRewriter::new(client, Arc::new(Passthrough)),
    )
    .lead_in(&kanade(), &facts())
    .await;
    assert_eq!(nudge.line, LineSource::Seed(SeedReason::Refused));
}

fn facts() -> NudgeFacts<'static> {
    NudgeFacts {
        channel_id: CHANNEL,
        purpose: NudgePurpose::SelfService,
        mood: NudgeMood::Playful,
        boss: "HMaleficStar, HFA",
        day: "Wed",
        time: "21:30",
    }
}

#[tokio::test(start_paused = true)]
async fn a_busy_rewrite_group_is_unavailable_at_once_with_nothing_sent() {
    let (provider, client) = client(vec![reply("unused")], true);
    let _held = client
        .governor()
        .try_acquire(Role::Rewrite, CallKind::Rewrite, "someone else")
        .expect("free permit");
    let started = Instant::now();
    let nudge = Nudger::new(Arc::new(Fixed), adapter(client.clone()))
        .lead_in(&kanade(), &facts())
        .await;
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(nudge.line, LineSource::Seed(SeedReason::Unavailable));
    assert!(provider.requests().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_misconfigured_or_external_rewrite_route_is_flagged_with_nothing_sent() {
    for (grouped, external) in [(false, false), (true, true)] {
        let (provider, client) = rewrite_client(grouped, external, vec![reply("unused")]);
        assert_eq!(
            adapter(client.clone())
                .rewrite(&rewrite_prompt(), DEADLINE)
                .await,
            Err(RewriteFailure::Misconfigured),
            "grouped {grouped}, external {external}"
        );
        let nudge = Nudger::new(Arc::new(Fixed), adapter(client))
            .lead_in(&kanade(), &facts())
            .await;
        assert_eq!(nudge.line, LineSource::Seed(SeedReason::Misconfigured));
        assert!(provider.requests().is_empty());
    }
}

fn moved(evidence: &str) -> FakeAction {
    reply(&format!(
        r#"{{"amendments": [{{"kind": "move", "bosses": ["HMaleficStar", "HFA"],
            "day_ref": "wed", "time_ref": "9:30pm", "participants": ["{MY}"],
            "confidence": 0.9, "evidence_message_ids": ["{evidence}"]}}],
          "summary": "proposed for wed"}}"#
    ))
}

fn post(id: &str) -> MessageEvent {
    MessageEvent::Posted(message(
        id,
        MY,
        local(8, 30, 13, 1),
        "mon cannot, change to wed 9:30pm?",
    ))
}

const REWRITTEN: &str = "Eh? Just move it yourself, it's right there.";

fn move_link(world: &World) -> String {
    // Wed 2 Sep 21:30 in Kuala Lumpur.
    format!(
        "{PORTAL}/runs/{}?move_to=2026-09-02T13:30:00Z",
        world.runs[0]
    )
}

fn reset() -> WeekReset {
    crate::fakes::config().week_reset()
}

#[tokio::test(start_paused = true)]
async fn a_closed_portal_keeps_today_behaviour_and_spends_no_tip() {
    let world = World::self_service(vec![moved("101")], |config| {
        config.self_service.mode = SelfServiceMode::LinkFirst;
    })
    .await;
    assert!(!world.extractor.config().self_service.public_portal_open);
    let (events, _loop) = world.pipeline();
    events.send(post("101")).await.expect("send");
    after(91).await;

    assert_eq!(world.requests(), 1, "no rewrite call");
    assert_eq!(world.live_proposals().await.len(), 1);
    assert!(world.outbox.redirects.lock().unwrap().is_empty());
    let cards = world.outbox.cards.lock().unwrap().clone();
    assert_eq!(cards[0].entries[0].self_service, None);
    let log = &world.logs().await[0];
    assert_eq!(log.outcome, ExtractionOutcome::Proposed);
    assert_eq!(log.guardrail, json!({}));
    // The week's tip is still unclaimed.
    let week = reset().current_week(now()).unwrap();
    assert!(world.store.claim_tip(MY, week, now()).await.unwrap());
}

#[tokio::test(start_paused = true)]
async fn link_first_with_the_portal_open_sends_only_the_link_and_one_lead_in() {
    let world = World::self_service(vec![moved("101"), reply(REWRITTEN)], |config| {
        config.self_service.mode = SelfServiceMode::LinkFirst;
        config.self_service.public_portal_open = true;
    })
    .await;
    let (events, _loop) = world.pipeline();
    events.send(post("101")).await.expect("send");
    after(91).await;

    assert!(world.live_proposals().await.is_empty());
    assert!(world.outbox.cards.lock().unwrap().is_empty());
    let redirects = world.outbox.redirects.lock().unwrap().clone();
    assert_eq!(redirects.len(), 1);
    let redirected = &redirects[0];
    assert_eq!(redirected.author_id, MY);
    assert_eq!(redirected.channel_id, CHANNEL);
    assert_eq!(redirected.tip.link.purpose, NudgePurpose::SelfService);
    assert_eq!(redirected.tip.link.url, move_link(&world));
    assert_eq!(redirected.tip.lead_in.as_deref(), Some(REWRITTEN));
    assert_eq!(redirected.tip.line, Some(LineSource::Rewritten));

    let log = &world.logs().await[0];
    assert_eq!(log.outcome, ExtractionOutcome::SelfServiceLink);
    assert_eq!(log.guardrail, json!({"nudges": ["rewritten"]}));
    assert!(!log.guardrail.to_string().contains(REWRITTEN));
    // The rewrite request carried no member ids.
    let requests = world.provider.requests();
    assert_eq!(requests.len(), 2);
    assert!(!format!("{:?}", requests[1].messages).contains(MY));
    let rewrite_text: String = requests[1]
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::System { content } | Message::User { content } => Some(content.clone()),
            _ => None,
        })
        .collect();
    assert!(!rewrite_text.contains(MY) && !rewrite_text.contains(CHANNEL));
}

#[tokio::test(start_paused = true)]
async fn cards_and_link_keeps_the_card_and_gives_the_lead_in_once_a_week() {
    let world = World::self_service(
        vec![moved("101"), reply(REWRITTEN), moved("102")],
        |config| {
            config.self_service.public_portal_open = true;
        },
    )
    .await;
    assert_eq!(
        world.extractor.config().self_service.mode,
        SelfServiceMode::CardsAndLink
    );
    let (events, _loop) = world.pipeline();
    events.send(post("101")).await.expect("send");
    after(91).await;
    events.send(post("102")).await.expect("send");
    after(91).await;

    assert!(world.outbox.redirects.lock().unwrap().is_empty());
    let cards = world.outbox.cards.lock().unwrap().clone();
    assert_eq!(cards.len(), 2);
    let first = cards[0].entries[0].self_service.clone().expect("link");
    assert_eq!(cards[0].entries[0].kind, AmendmentKind::Move);
    assert_eq!(first.link.url, move_link(&world));
    assert_eq!(first.lead_in.as_deref(), Some(REWRITTEN));
    // Same member, same boss week: the link again, no second lead-in or rewrite.
    let second = cards[1].entries[0].self_service.clone().expect("link");
    assert_eq!(second.link.url, move_link(&world));
    assert_eq!(second.lead_in, None);
    assert_eq!(world.requests(), 3);
    let logs = world.logs().await;
    assert_eq!(logs[0].outcome, ExtractionOutcome::Proposed);
    assert_eq!(logs[0].guardrail, json!({"nudges": ["rewritten"]}));
    assert_eq!(logs[1].guardrail, json!({}));
}

#[tokio::test(start_paused = true)]
async fn a_refused_rewrite_is_logged_by_label_and_the_seed_is_used() {
    let world = World::self_service(vec![moved("101"), filtered()], |config| {
        config.self_service.mode = SelfServiceMode::LinkFirst;
        config.self_service.public_portal_open = true;
    })
    .await;
    let (events, _loop) = world.pipeline();
    events.send(post("101")).await.expect("send");
    after(91).await;
    let redirects = world.outbox.redirects.lock().unwrap().clone();
    let tip = &redirects[0].tip;
    assert_eq!(tip.line, Some(LineSource::Seed(SeedReason::Refused)));
    let persona = kanade();
    let seed = persona.nudge_seeds(NudgePurpose::SelfService, NudgeMood::Playful);
    assert!(seed.lines.contains(&tip.lead_in.as_deref().unwrap()));
    assert_eq!(
        world.logs().await[0].guardrail,
        json!({"nudges": ["seed_refused"]})
    );
}
