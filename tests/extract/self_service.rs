//! Self-service redirect wired end to end (N1): the governed rewrite adapter
//! over the fake provider, and the pipeline posting links, the weekly lead-in
//! and its log label under each mode, with today's cards-only behaviour while
//! the public portal is closed.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use kanade::chat::nudge::{
    GovernedRewriter, LineSource, NudgeFacts, NudgeRewriter, Nudger, RewriteFailure, RewritePrompt,
    SeedReason,
};
use kanade::chat::persona::{NudgeMood, NudgePurpose};
use kanade::domain::drafts::ProposalSource;
use kanade::domain::model_log::{ExtractionOutcome, ModelLogStore};
use kanade::domain::notify::WeekReset;
use kanade::domain::proposals::{ChangeKind, ProposedChange};
use kanade::domain::scheduler::{ProposalRequest, Supersede};
use kanade::extract::AmendmentKind;
use kanade::extract::pipeline::MessageEvent;
use kanade::extract::pipeline::Proposer;
use kanade::extract::redirect::SelfServiceMode;
use kanade::infrastructure::llm::governor::{
    BreakerState, CallKind, Governor, GovernorConfig, GovernorPolicy, GroupConfig, ModelClient,
    Outcome, Random, Role, RoleConfig,
};
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

fn adapter<P: kanade::infrastructure::llm::LlmProvider>(
    client: Arc<ModelClient<P>>,
) -> GovernedRewriter<P> {
    GovernedRewriter::new(client)
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
    let prompt = rewrite_prompt();
    let rewritten = adapter(client).rewrite(&prompt, DEADLINE).await;
    assert_eq!(rewritten.as_deref(), Ok("Fine. Fix it yourself."));
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.model, ALIAS);
    assert!(request.output_schema.is_none() && request.tools.is_empty());
    assert!(matches!(
        request.messages.as_slice(),
        [Message::System { .. }, Message::User { content }]
            if content == "Line to rewrite: Hmph. Everything you need is right here."
    ));
    assert_eq!(request.messages, prompt.messages());
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
    let nudge = Nudger::new(Arc::new(Fixed), GovernedRewriter::new(client))
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
async fn an_ungrouped_rewrite_route_is_flagged_with_nothing_sent() {
    let (provider, client) = rewrite_client(false, false, vec![reply("unused")]);
    assert_eq!(
        adapter(client.clone())
            .rewrite(&rewrite_prompt(), DEADLINE)
            .await,
        Err(RewriteFailure::Misconfigured)
    );
    let nudge = Nudger::new(Arc::new(Fixed), adapter(client))
        .lead_in(&kanade(), &facts())
        .await;
    assert_eq!(nudge.line, LineSource::Seed(SeedReason::Misconfigured));
    assert!(provider.requests().is_empty());
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
    assert_eq!(
        log.guardrail,
        json!({"context": {"window": 8192, "reserve": 2500, "source": "local_default"}})
    );
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
    assert_eq!(
        log.guardrail,
        json!({"context": {"window": 8192, "reserve": 2500, "source": "local_default"}, "nudges": ["rewritten"]})
    );
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
async fn a_link_that_never_posted_gives_the_weekly_tip_back() {
    let world = World::self_service(vec![moved("101"), reply(REWRITTEN)], |config| {
        config.self_service.mode = SelfServiceMode::LinkFirst;
        config.self_service.public_portal_open = true;
    })
    .await;
    world.outbox.fail_posts.store(true, Ordering::SeqCst);
    let (events, _loop) = world.pipeline();
    events.send(post("101")).await.expect("send");
    after(91).await;

    let week = reset().current_week(now()).unwrap();
    let redirects = world.outbox.redirects.lock().unwrap().clone();
    assert_eq!(redirects[0].tip.claimed, Some((MY.to_owned(), week)));
    assert!(
        world.store.claim_tip(MY, week, now()).await.unwrap(),
        "the failed post released the tip"
    );
}

#[tokio::test(start_paused = true)]
async fn a_card_that_never_posted_gives_the_weekly_tip_back() {
    let world = World::self_service(vec![moved("101"), reply(REWRITTEN)], |config| {
        config.self_service.public_portal_open = true;
    })
    .await;
    world.outbox.fail_posts.store(true, Ordering::SeqCst);
    let (events, _loop) = world.pipeline();
    events.send(post("101")).await.expect("send");
    after(91).await;

    let cards = world.outbox.cards.lock().unwrap().clone();
    assert!(cards[0].entries[0].self_service.is_some());
    let week = reset().current_week(now()).unwrap();
    assert!(world.store.claim_tip(MY, week, now()).await.unwrap());
}

#[tokio::test(start_paused = true)]
async fn a_saved_card_waiting_for_a_repost_keeps_its_tip() {
    let world = World::self_service(vec![moved("101"), reply(REWRITTEN)], |config| {
        config.self_service.public_portal_open = true;
    })
    .await;
    world.outbox.pending_posts.store(true, Ordering::SeqCst);
    let (events, _loop) = world.pipeline();
    events.send(post("101")).await.expect("send");
    after(91).await;

    let cards = world.outbox.cards.lock().unwrap().clone();
    assert!(cards[0].entries[0].self_service.is_some());
    let week = reset().current_week(now()).unwrap();
    assert!(
        !world.store.claim_tip(MY, week, now()).await.unwrap(),
        "the stranded repost still carries the link, so the tip stays spent"
    );
}

#[tokio::test(start_paused = true)]
async fn a_link_first_move_retires_the_older_card_for_its_run() {
    let world = World::self_service(vec![moved("101"), reply(REWRITTEN)], |config| {
        config.self_service.mode = SelfServiceMode::LinkFirst;
        config.self_service.public_portal_open = true;
    })
    .await;
    let older = world
        .scheduler
        .propose(ProposalRequest {
            change: ProposedChange {
                run_id: Some(world.runs[0].clone()),
                channel_id: Some(CHANNEL.into()),
                bosses: vec!["HMaleficStar".into(), "HFA".into()],
                participants: vec![MY.into()],
                new_datetime: Some(local(9, 2, 22, 0)),
                ..ProposedChange::new(ChangeKind::Move)
            },
            source: ProposalSource::Extraction,
            source_id: "x-older".into(),
            supersede: Supersede::Older,
        })
        .await
        .expect("older proposal")
        .proposal
        .id;
    let (events, _loop) = world.pipeline();
    events.send(post("101")).await.expect("send");
    after(91).await;

    assert_eq!(world.outbox.redirects.lock().unwrap().len(), 1);
    assert!(
        world.live_proposals().await.is_empty(),
        "the older card retired"
    );
    let cards = world.outbox.cards.lock().unwrap().clone();
    assert_eq!(cards.len(), 1);
    assert!(cards[0].entries.is_empty());
    assert_eq!(
        cards[0].superseded,
        [older],
        "its card is marked superseded"
    );
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
    assert_eq!(
        logs[0].guardrail,
        json!({"context": {"window": 8192, "reserve": 2500, "source": "local_default"}, "nudges": ["rewritten"]})
    );
    assert_eq!(
        logs[1].guardrail,
        json!({"context": {"window": 8192, "reserve": 2500, "source": "local_default"}})
    );
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
        json!({"context": {"window": 8192, "reserve": 2500, "source": "local_default"}, "nudges": ["seed_refused"]})
    );
}

#[tokio::test(start_paused = true)]
async fn a_refused_proposal_posts_no_card_or_link_and_spends_no_tip() {
    // The same self-service move that gets a link beside its card in
    // `cards_and_link_keeps_the_card_and_gives_the_lead_in_once_a_week`, but
    // the scheduler refuses the proposal up front.
    let world = World::self_service(vec![moved("101"), reply(REWRITTEN)], |config| {
        config.self_service.public_portal_open = true;
    })
    .await;
    world.scheduler.refuse.store(true, Ordering::SeqCst);
    let (events, _loop) = world.pipeline();
    events.send(post("101")).await.expect("send");
    after(91).await;

    let log = &world.logs().await[0];
    assert_eq!(log.error, None);
    assert_eq!(log.refusals.len(), 1);
    assert_eq!(
        (
            log.refusals[0].change.as_str(),
            log.refusals[0].code.as_str()
        ),
        ("move", "no_effect")
    );
    assert!(world.live_proposals().await.is_empty());
    assert!(world.outbox.cards.lock().unwrap().is_empty());
    assert!(world.outbox.redirects.lock().unwrap().is_empty());
    assert_eq!(world.requests(), 1, "no rewrite for a link never posted");
    assert_eq!(
        log.guardrail,
        json!({"context": {"window": 8192, "reserve": 2500, "source": "local_default"}})
    );
    let week = reset().current_week(now()).unwrap();
    assert!(world.store.claim_tip(MY, week, now()).await.unwrap());
}

fn group(
    client: &ModelClient<FakeProvider>,
) -> kanade::infrastructure::llm::governor::GroupSnapshot {
    let wall = chrono::DateTime::<chrono::Utc>::from_timestamp(1_790_000_000, 0).unwrap();
    client.governor().snapshot(wall).remove(0)
}

fn hanging() -> FakeAction {
    FakeAction::Delayed {
        delay: Duration::from_secs(60),
        action: Box::new(reply("far too late")),
    }
}

#[tokio::test(start_paused = true)]
async fn a_rewrite_cut_off_mid_request_frees_its_permit_and_leaves_the_breaker_alone() {
    let (provider, client) = rewrite_client(true, false, vec![hanging(), hanging()]);
    let adapter = adapter(client.clone());
    // The caller's deadline, not the adapter's, drops the request mid-flight.
    let started = Instant::now();
    let prompt = rewrite_prompt();
    let cut = tokio::time::timeout(DEADLINE, adapter.rewrite(&prompt, DEADLINE * 30));
    assert!(cut.await.is_err(), "dropped by the outer timeout");
    assert_eq!(started.elapsed(), DEADLINE);
    assert_eq!(provider.requests().len(), 1, "the request was in flight");
    let after_cut = group(&client);
    assert_eq!(after_cut.permits.in_use, 0);
    assert_eq!(after_cut.breaker.state, BreakerState::Closed);
    assert_eq!(after_cut.breaker.failures, 0);

    // Through the nudge: the seed is used and the group is left as found.
    let started = Instant::now();
    let nudge = Nudger::new(Arc::new(Fixed), adapter)
        .lead_in(&kanade(), &facts())
        .await;
    assert_eq!(started.elapsed(), DEADLINE);
    assert!(
        matches!(nudge.line, LineSource::Seed(_)),
        "{:?}",
        nudge.line
    );
    let seeds = kanade();
    let pool = seeds.nudge_seeds(NudgePurpose::SelfService, NudgeMood::Playful);
    assert!(pool.lines.contains(&nudge.lead_in.as_str()));
    let after_nudge = group(&client);
    assert_eq!(after_nudge.permits.in_use, 0);
    assert_eq!(after_nudge.breaker.state, BreakerState::Closed);
    assert_eq!(after_nudge.breaker.failures, 0);
}

#[tokio::test(start_paused = true)]
async fn a_cut_off_half_open_probe_frees_the_probe_slot() {
    let (provider, client) = rewrite_client(true, false, vec![hanging()]);
    let governor = client.governor().clone();
    // Trip the breaker from the rewrite role, then wait for the half-open probe.
    let permit = governor
        .try_acquire(Role::Rewrite, CallKind::Rewrite, "tripper")
        .unwrap();
    for _ in 0..5 {
        permit
            .begin_request(Duration::from_secs(60))
            .await
            .unwrap()
            .finish(Outcome::TransientFailure);
    }
    drop(permit);
    assert_eq!(group(&client).breaker.state, BreakerState::Open);
    tokio::time::advance(Duration::from_secs(46)).await;
    assert_eq!(group(&client).breaker.state, BreakerState::HalfOpen);

    // The nudge's rewrite takes the single slot and its request is the probe;
    // cut off at 2 s, the probe is abandoned, not failed.
    let nudge = Nudger::new(Arc::new(Fixed), adapter(client.clone()))
        .lead_in(&kanade(), &facts())
        .await;
    assert!(
        matches!(nudge.line, LineSource::Seed(_)),
        "{:?}",
        nudge.line
    );
    assert_eq!(provider.requests().len(), 1, "the probe was sent");
    let snapshot = group(&client);
    assert_eq!(snapshot.permits.in_use, 0);
    assert_eq!(snapshot.breaker.state, BreakerState::HalfOpen);
    // The probe slot is free again: the next holder's request is the probe.
    let next = governor
        .try_acquire(Role::Rewrite, CallKind::Rewrite, "next")
        .expect("probe slot freed");
    assert!(next.try_begin_request().expect("admitted").is_probe());
}

#[tokio::test(start_paused = true)]
async fn an_external_rewrite_sends_raw_names_ids_and_urls_without_opt_in() {
    let url = "https://synthetic.invalid/persona/credit?tag=fixture#public";
    let seed = format!("SyntheticMember 999000111222333444 says {{boss}} is right here. See {url}");
    let (provider, client) = rewrite_client(
        true,
        true,
        vec![reply("A small synthetic example — {boss} {day} {time}!")],
    );
    assert!(client.governor().route(Role::Rewrite).unwrap().external);
    let rewriter = GovernedRewriter::new(client);
    let prompt = RewritePrompt::build(&kanade(), NudgeMood::Playful, &seed);
    let rewritten = rewriter.rewrite(&prompt, DEADLINE).await;
    assert_eq!(
        rewritten,
        Ok("A small synthetic example — {boss} {day} {time}!".into())
    );
    let requests = provider.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].messages, prompt.messages());
    let serialized = serde_json::to_string(&requests[0].messages).unwrap();
    for raw in ["SyntheticMember", "999000111222333444", url] {
        assert!(serialized.contains(raw), "{raw} missing from {serialized}");
    }
}

#[tokio::test(start_paused = true)]
async fn each_rewrite_reads_the_live_reserve_and_a_rewrite_in_flight_keeps_its_own() {
    use std::sync::atomic::AtomicU32;
    let late = FakeAction::Delayed {
        delay: Duration::from_millis(500),
        action: Box::new(reply("Late line.")),
    };
    let (provider, client) = client(
        vec![reply("Default line."), late, reply("Next line.")],
        true,
    );
    // No resolver: the default reserve.
    GovernedRewriter::new(client.clone())
        .rewrite(&rewrite_prompt(), DEADLINE)
        .await
        .unwrap();
    assert_eq!(provider.requests()[0].max_output_tokens, 96);

    let saved = Arc::new(AtomicU32::new(64));
    let reading = saved.clone();
    let rewriter = Arc::new(GovernedRewriter::new(client).with_reserve(Arc::new(
        move |alias: &str| {
            assert_eq!(alias, ALIAS);
            reading.load(Ordering::SeqCst)
        },
    )));
    let running = rewriter.clone();
    let in_flight = tokio::spawn(async move { running.rewrite(&rewrite_prompt(), DEADLINE).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(provider.requests().len(), 2, "sent before the save");
    saved.store(120, Ordering::SeqCst);
    assert_eq!(in_flight.await.unwrap().as_deref(), Ok("Late line."));
    assert_eq!(provider.requests()[1].max_output_tokens, 64);
    rewriter.rewrite(&rewrite_prompt(), DEADLINE).await.unwrap();
    assert_eq!(
        provider.requests()[2].max_output_tokens,
        120,
        "the next rewrite reads the save"
    );
}

fn answered(content: &str, prompt_tokens: u32, completion_tokens: u32) -> FakeAction {
    FakeAction::Response(kanade::infrastructure::llm::CompletionResponse {
        reasoning_content: Some("Keep it short.".into()),
        reasoning_tokens: Some(3),
        model: ALIAS.into(),
        content: Some(content.to_owned()),
        tool_calls: Vec::new(),
        finish_reason: kanade::infrastructure::llm::FinishReason::Stop,
        usage: Some(kanade::infrastructure::llm::Usage {
            prompt_tokens,
            completion_tokens,
        }),
    })
}

/// The detailed rewrite reports the route, `max_tokens`, the reservation the
/// usage was checked against, the reply and its reasoning.
#[tokio::test(start_paused = true)]
async fn the_detailed_rewrite_reports_route_usage_and_reservation() {
    use kanade::chat::nudge::REWRITE_MAX_OUTPUT_TOKENS;

    let (_, client) = client(vec![answered("Fine. Fix it yourself.", 40, 6)], true);
    let outcome = adapter(client)
        .rewrite_detailed(&rewrite_prompt(), DEADLINE)
        .await;
    assert_eq!(outcome.result.as_deref(), Ok("Fine. Fix it yourself."));
    let detail = outcome.detail;
    assert_eq!(detail.code, None);
    assert_eq!(detail.alias.as_deref(), Some(ALIAS));
    assert_eq!(detail.max_output_tokens, Some(REWRITE_MAX_OUTPUT_TOKENS));
    let reservation = detail.reservation.expect("reservation");
    assert!(reservation > REWRITE_MAX_OUTPUT_TOKENS, "{reservation}");
    assert_eq!(
        detail
            .usage
            .map(|usage| (usage.prompt_tokens, usage.completion_tokens)),
        Some((40, 6))
    );
    assert_eq!(detail.reasoning.as_deref(), Some("Keep it short."));
    assert_eq!(detail.reasoning_tokens, Some(3));
    assert_eq!(detail.reply.as_deref(), Some("Fine. Fix it yourself."));
}

/// The live failure: the gateway answered, but the reported usage exceeds
/// the runner's reservation. The rejection is unchanged (`unavailable`, the
/// seed is used); the detail names `budget_exceeded` and keeps what the
/// model reported, so the log shows "used > reserved".
#[tokio::test(start_paused = true)]
async fn a_reply_over_its_reservation_is_unavailable_with_budget_exceeded_and_its_usage() {
    let (provider, client) = client(vec![answered("Waku waku!", 5_000, 412)], true);
    let outcome = adapter(client)
        .rewrite_detailed(&rewrite_prompt(), DEADLINE)
        .await;
    assert_eq!(outcome.result, Err(RewriteFailure::Unavailable));
    assert_eq!(provider.requests().len(), 1, "no retry");
    let detail = outcome.detail;
    assert_eq!(detail.code, Some("budget_exceeded"));
    let reservation = detail.reservation.expect("reservation");
    assert!(5_412 > reservation, "{reservation}");
    assert_eq!(
        detail
            .usage
            .map(|usage| (usage.prompt_tokens, usage.completion_tokens)),
        Some((5_000, 412))
    );
    assert_eq!(detail.reply.as_deref(), Some("Waku waku!"));
    assert_eq!(detail.reasoning.as_deref(), Some("Keep it short."));
    assert_eq!(detail.budget, None, "sent within budget");
}

/// Refusals before anything is sent carry the governor's code.
#[tokio::test(start_paused = true)]
async fn a_refused_rewrite_carries_the_governor_code() {
    let (_, client) = rewrite_client(false, false, vec![reply("unused")]);
    let outcome = adapter(client)
        .rewrite_detailed(&rewrite_prompt(), DEADLINE)
        .await;
    assert_eq!(outcome.result, Err(RewriteFailure::Misconfigured));
    assert_eq!(outcome.detail.code, Some("ungrouped"));
    assert_eq!(outcome.detail.reservation, None);
}

/// A reserve that leaves the reservation over the call token budget is
/// refused before anything is sent; the detail names the budget, so the log
/// shows "reserved N > budget 16384".
#[tokio::test(start_paused = true)]
async fn a_reservation_over_the_call_budget_is_refused_before_sending() {
    use kanade::infrastructure::llm::CALL_TOKEN_BUDGET;

    let (provider, client) = client(vec![reply("unused")], true);
    let rewriter = GovernedRewriter::new(client).with_reserve(Arc::new(|_: &str| 16_384));
    let outcome = rewriter.rewrite_detailed(&rewrite_prompt(), DEADLINE).await;
    assert_eq!(outcome.result, Err(RewriteFailure::Unavailable));
    assert!(provider.requests().is_empty(), "nothing sent");
    let detail = outcome.detail;
    assert_eq!(detail.code, Some("budget_exceeded"));
    assert_eq!(detail.budget, Some(CALL_TOKEN_BUDGET));
    let reservation = detail.reservation.expect("reservation");
    assert!(reservation > CALL_TOKEN_BUDGET, "{reservation}");
    assert_eq!(detail.usage, None);
}
