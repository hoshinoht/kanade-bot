//! D-CROSS-CHANNEL-PROPOSAL: unlike v4, chat reuses another channel's
//! identical submitted run proposal, without posting or changing a card.

use std::{sync::Arc, time::Duration};

use kanade::chat::answer::{AnswerDeps, AnswerFailure, AnswerSettings, Question, answer};
use kanade::chat::tools::ToolContext;
use kanade::chat::tools::bundles::ToolOffer;
use kanade::chat::tools::propose::Proposer;

use kanade::domain::drafts::{
    DraftChange, DraftStatus, DraftStore, DraftUpdate, ProposalSource, ProposalStore,
};
use kanade::domain::history::Actor;
use kanade::domain::notify::{
    Claim, DeliveryJournal, DeliveryTarget, EffectKind, IntentContent, NotificationIntent, Receipt,
};
use kanade::domain::proposals::{
    Approver, CardDetails, ChangeKind, ProposalCardStore, ProposedChange,
};
use kanade::domain::scheduler::{Clock, ProposalRequest, SchedulerService, Supersede};
use kanade::infrastructure::llm::identity::PassthroughSession;
use kanade::infrastructure::llm::{
    CompletionResponse, FakeAction, FakeProvider, FinishReason, Message, ModelCapabilities,
    ToolCall,
};
use serde_json::{Value, json};

use crate::common::utc;
use crate::looping::{Ports, said};
use crate::model::{Scripted, client};
use crate::slow_store::SlowStore;
use crate::world::World;

const RUN: &str = "c0c0c0c0-0000-4000-8000-000000000001";
const TO: &str = "2026-10-05T22:00:00+00:00";

async fn world() -> World {
    World::new(&json!({
        "clock": "2026-10-02T12:00:00+00:00", "timezone": "UTC",
        "reset_weekday": 3, "reset_time": "00:00",
        "uuid_sequence": (1..80).map(|n| format!("d0d0d0d0-0000-4000-8000-{n:012}")).collect::<Vec<_>>(),
        "catalog": {"bosses": [{"short": "FA", "full": "First Adversary", "difficulties": ["h"], "aliases": ["fa"]}],
                    "difficulties": [{"prefix": "h", "label": "Hard"}]},
        "channels": [{"id": "900", "name": "test-home"}, {"id": "901", "name": "test-pilot"}],
        "settings": {"guild_id": "800", "watched_channel_ids": "900", "chat_pilot_channel_ids": "901", "chat_pilot_category_ids": ""},
        "bot_user": {"id": "999", "name": "TestBot"}, "self_role_id": "777", "guides": [],
        "world": {"members": [{"user_id": "101", "display_name": "Invented Mira", "has_role": true},
                               {"user_id": "102", "display_name": "Invented Sora", "has_role": true}],
                  "fixed": [], "rsvps": [],
                  "runs": [{"id": RUN, "channel_id": "900", "week_start": "2026-10-01T00:00:00+00:00",
                            "at": "2026-10-06T20:00:00+00:00", "bosses": ["HFA"], "participants": ["101", "102"], "status": "planned"}]}
    })).await
}

fn step(channel: &str, tool: &str, extra: Value) -> Value {
    let mut arguments = extra.as_object().unwrap().clone();
    arguments.insert("run_query".into(), json!(RUN));
    json!({"author_id": "101", "channel_id": channel, "tool": tool, "arguments": arguments})
}

async fn seed(world: &mut World, source: ProposalSource, kind: ChangeKind) -> String {
    let mut change = ProposedChange::new(kind);
    change.run_id = Some(RUN.into());
    change.channel_id = Some("900".into());
    change.bosses = vec!["HFA".into()];
    if kind == ChangeKind::Move {
        change.new_datetime = Some(utc(&json!(TO)));
    }
    if kind == ChangeKind::Rsvp {
        change.participants = vec!["101".into()];
        change.rsvp = Some(kanade::domain::schedule::RsvpState::Yes);
    }
    world
        .service
        .propose(
            ProposalRequest {
                change,
                source,
                source_id: "invented-source".into(),
                supersede: Supersede::Keep,
            },
            &world.policy,
            &world.guild,
        )
        .await
        .unwrap()
        .proposal
        .id
}

async fn save_card(world: &World, id: &str) {
    let details =
        CardDetails::from_json(&json!({"kind": "move", "run_id": RUN, "new_datetime": TO,
                                               "bosses": ["HFA"], "summary": "invented move"}))
        .unwrap();
    world
        .service
        .store()
        .save_card(id, "900", &details, world.clock.now())
        .await
        .unwrap();
}

async fn bind_card(world: &World, id: &str) {
    save_card(world, id).await;
    let store = world.service.store();
    let now = world.clock.now();
    let lease = store.begin_lease("test", "post-card", now).await.unwrap();
    let intent = NotificationIntent {
        effect: EffectKind::Card,
        effect_context: Vec::new(),
        channel_id: "900".into(),
        targets: vec![DeliveryTarget::Card(id.into())],
        mentions: Vec::new(),
        warnings: Vec::new(),
        content: IntentContent::ProposalCard {
            proposal_ids: vec![id.into()],
        },
    };
    let Claim::Fresh(attempt) = store.claim(&lease, &intent, None, now).await.unwrap() else {
        panic!("fresh claim")
    };
    store
        .bind(
            &lease,
            &attempt,
            &Receipt {
                channel_id: "900".into(),
                message_id: "950001".into(),
            },
            None,
            now,
        )
        .await
        .unwrap();
    store.end_lease(&lease, now).await.unwrap();
}

#[tokio::test]
async fn cross_channel_existing_extraction_and_chat_card_is_reused_with_jump_link() {
    for source in [ProposalSource::Extraction, ProposalSource::Chat] {
        let mut world = world().await;
        let id = seed(&mut world, source, ChangeKind::Move).await;
        bind_card(&world, &id).await;
        let before = world.service.store().load_proposal(&id).await.unwrap();
        let events = world.service.store().draft_events(&id).await.unwrap();
        let cards = world
            .service
            .store()
            .load_cards(std::slice::from_ref(&id))
            .await
            .unwrap();
        let snapshot = world.snapshot().await;
        let outcome = world
            .run_tool(
                &step("901", "propose_move", json!({"to_when": "mon 22:00"})),
                &mut PassthroughSession,
            )
            .await;
        assert!(outcome.ok, "{}", outcome.output);
        assert!(outcome.created.is_empty() && outcome.cards.is_empty());
        assert!(
            outcome.output.contains("Already proposed")
                && outcome.output.contains("awaiting approval")
        );
        assert!(
            outcome.output.contains("<#900>")
                && outcome
                    .output
                    .contains("https://discord.com/channels/800/900/950001")
        );
        assert_eq!(
            world
                .service
                .store()
                .list_proposals(false)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            world.service.store().load_proposal(&id).await.unwrap(),
            before
        );
        assert_eq!(
            world.service.store().draft_events(&id).await.unwrap(),
            events
        );
        assert_eq!(
            world.service.store().load_cards(&[id]).await.unwrap(),
            cards
        );
        assert_eq!(world.snapshot().await, snapshot);
    }
}

#[tokio::test]
async fn unposted_proposal_is_reused_before_and_after_card_details_are_saved() {
    let mut world = world().await;
    let id = seed(&mut world, ProposalSource::Extraction, ChangeKind::Move).await;
    for saved in [false, true] {
        if saved {
            save_card(&world, &id).await;
        }
        let outcome = world
            .run_tool(
                &step("901", "propose_move", json!({"to_when": "mon 22:00"})),
                &mut PassthroughSession,
            )
            .await;
        assert!(outcome.ok && outcome.cards.is_empty() && outcome.created.is_empty());
        assert!(outcome.output.contains("still being posted") && outcome.output.contains("<#900>"));
        assert!(!outcome.output.contains("https://discord.com/channels/"));
        assert_eq!(
            world
                .service
                .store()
                .list_proposals(false)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn different_target_cross_channel_creates_and_same_channel_supersedes_as_before() {
    for (channel, to, old_status) in [
        ("901", "mon 23:00", DraftStatus::Submitted),
        ("900", "mon 22:00", DraftStatus::Discarded),
    ] {
        let mut world = world().await;
        let id = seed(&mut world, ProposalSource::Extraction, ChangeKind::Move).await;
        let outcome = world
            .run_tool(
                &step(channel, "propose_move", json!({"to_when": to})),
                &mut PassthroughSession,
            )
            .await;
        assert!(outcome.ok, "{}", outcome.output);
        assert_eq!(outcome.cards.len(), 1);
        assert_eq!(outcome.created.len(), 1);
        assert_eq!(
            world
                .service
                .store()
                .list_proposals(false)
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            world
                .service
                .store()
                .load_proposal(&id)
                .await
                .unwrap()
                .unwrap()
                .0
                .draft
                .status,
            old_status
        );
        assert_eq!(
            outcome.cards[0].superseded,
            if channel == "900" { vec![id] } else { vec![] }
        );
    }
}

#[tokio::test]
async fn rejected_discarded_and_approved_proposals_do_not_block_new_chat_proposals() {
    for status in [
        DraftStatus::Rejected,
        DraftStatus::Discarded,
        DraftStatus::Merged,
    ] {
        let mut world = world().await;
        let original = world.snapshot().await.runs[0].clone();
        let id = seed(&mut world, ProposalSource::Extraction, ChangeKind::Move).await;
        if status == DraftStatus::Merged {
            let approver = Approver {
                user_id: "101".into(),
                has_role: true,
                is_admin: false,
                via_portal: false,
            };
            world
                .service
                .approve_proposal(&id, &approver, &world.policy, &world.guild)
                .await
                .unwrap();
            // A later legitimate move restores the target's starting state.
            world
                .put(kanade::domain::schedule::Change::PutRun(original))
                .await;
        } else {
            world
                .service
                .store()
                .update_draft(DraftUpdate {
                    draft_id: id.clone(),
                    expected_version: 1,
                    actor: Actor::system("test"),
                    at: world.clock.now(),
                    change: DraftChange::Close {
                        status,
                        reason: None,
                        notices: Vec::new(),
                    },
                })
                .await
                .unwrap();
        }
        let outcome = world
            .run_tool(
                &step("901", "propose_move", json!({"to_when": "mon 22:00"})),
                &mut PassthroughSession,
            )
            .await;
        assert!(outcome.ok, "{}", outcome.output);
        assert_eq!(outcome.cards.len(), 1);
        assert_eq!(
            world
                .service
                .store()
                .load_proposal(&id)
                .await
                .unwrap()
                .unwrap()
                .0
                .draft
                .status,
            status
        );
    }
}

#[tokio::test]
async fn cancel_and_rsvp_match_only_the_same_kind_and_rsvp_member_answer() {
    for (kind, tool, args) in [
        (ChangeKind::Cancel, "propose_cancel", json!({})),
        (ChangeKind::Rsvp, "propose_rsvp", json!({"answer": "yes"})),
    ] {
        let mut world = world().await;
        seed(&mut world, ProposalSource::Chat, kind).await;
        let outcome = world
            .run_tool(&step("901", tool, args), &mut PassthroughSession)
            .await;
        assert!(outcome.ok && outcome.cards.is_empty(), "{}", outcome.output);
        if kind == ChangeKind::Rsvp {
            let mut other = step("901", tool, json!({"answer": "yes"}));
            other["author_id"] = json!("102");
            assert_eq!(
                world
                    .run_tool(&other, &mut PassthroughSession)
                    .await
                    .cards
                    .len(),
                1
            );
        }
    }
    let mut world = world().await;
    seed(&mut world, ProposalSource::Chat, ChangeKind::Rsvp).await;
    assert_eq!(
        world
            .run_tool(
                &step("901", "propose_rsvp", json!({"answer": "no"})),
                &mut PassthroughSession
            )
            .await
            .cards
            .len(),
        1
    );
    let mut world = self::world().await;
    seed(&mut world, ProposalSource::Chat, ChangeKind::Move).await;
    assert_eq!(
        world
            .run_tool(
                &step("901", "propose_cancel", json!({})),
                &mut PassthroughSession
            )
            .await
            .cards
            .len(),
        1
    );
}

#[tokio::test]
async fn home_channel_ask_reuses_the_other_channels_chat_proposal_without_retiring_it() {
    let mut world = world().await;
    let first = world
        .run_tool(
            &step("901", "propose_move", json!({"to_when": "mon 22:00"})),
            &mut PassthroughSession,
        )
        .await;
    assert!(first.ok && first.cards.len() == 1, "{}", first.output);
    let id = &first.created[0];
    let before = world.service.store().load_proposal(id).await.unwrap();
    let outcome = world
        .run_tool(
            &step("900", "propose_move", json!({"to_when": "mon 22:00"})),
            &mut PassthroughSession,
        )
        .await;
    assert!(outcome.ok && outcome.cards.is_empty() && outcome.created.is_empty());
    assert!(outcome.output.contains("<#901>") && outcome.output.contains("still being posted"));
    assert_eq!(
        world.service.store().load_proposal(id).await.unwrap(),
        before
    );
    assert_eq!(
        world
            .service
            .store()
            .list_proposals(false)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn duplicate_lookup_does_not_bypass_authority_or_read_only_checks() {
    let mut world = world().await;
    let id = seed(&mut world, ProposalSource::Extraction, ChangeKind::Move).await;
    bind_card(&world, &id).await;
    for flag in ["outsider", "read_only"] {
        let mut ask = step("901", "propose_move", json!({"to_when": "mon 22:00"}));
        if flag == "outsider" {
            ask["author_id"] = json!("103");
        } else {
            ask["read_only"] = json!(true);
        }
        let outcome = world.run_tool(&ask, &mut PassthroughSession).await;
        assert!(!outcome.ok && outcome.cards.is_empty());
        assert!(!outcome.output.contains("950001"));
        assert_eq!(
            world
                .service
                .store()
                .list_proposals(false)
                .await
                .unwrap()
                .len(),
            1
        );
    }
}

fn move_model() -> Arc<Scripted> {
    let model = "synthetic-dedupe";
    let mut caps = ModelCapabilities::minimal();
    caps.function_tools = true;
    Arc::new(Scripted {
        caps,
        fake: FakeProvider::new([
            FakeAction::Response(CompletionResponse {
                reasoning_content: None,
                reasoning_tokens: None,
                model: model.into(),
                content: None,
                tool_calls: vec![ToolCall {
                    id: "m1".into(),
                    name: "propose_move".into(),
                    arguments: json!({"run_query": RUN, "to_when": "mon 22:00"}).to_string(),
                }],
                finish_reason: FinishReason::ToolCalls,
                usage: None,
            }),
            FakeAction::Response(said(model, "It's awaiting approval.")),
        ]),
    })
}

fn question(ctx: &ToolContext) -> Question<'_> {
    Question {
        ctx,
        conversation: vec![
            Message::System {
                content: "SYSTEM".into(),
            },
            Message::User {
                content: "Invented Mira: move my run to Monday at 22:00".into(),
            },
        ],
        profanity: None,
        reminder: "Keep the tool facts exact.".into(),
        offer: ToolOffer::full_set(false),
        settings: AnswerSettings {
            tool_rounds: 3,
            timeout: Duration::from_secs(60),
            reasoning: None,
            temperature: None,
            max_output_tokens: 1024,
            model_context_tokens: 32768,
            clean_retry: false,
        },
    }
}

async fn seed_move_at(
    world: &mut World,
    channel: &str,
    source: ProposalSource,
    to: &str,
) -> String {
    let mut change = ProposedChange::new(ChangeKind::Move);
    change.run_id = Some(RUN.into());
    change.channel_id = Some(channel.into());
    change.bosses = vec!["HFA".into()];
    change.new_datetime = Some(utc(&json!(to)));
    world
        .service
        .propose(
            ProposalRequest {
                change,
                source,
                source_id: "invented-seed".into(),
                supersede: Supersede::Keep,
            },
            &world.policy,
            &world.guild,
        )
        .await
        .unwrap()
        .proposal
        .id
}

#[tokio::test]
async fn reuse_retires_this_channels_different_or_identical_proposal_and_refreshes_its_card() {
    for (asking, other) in [("901", "900"), ("900", "901")] {
        for target in ["2026-10-05T23:00:00+00:00", TO] {
            let mut world = world().await;
            let old = seed_move_at(&mut world, asking, ProposalSource::Chat, target).await;
            let kept = seed_move_at(&mut world, other, ProposalSource::Extraction, TO).await;
            let before = world.service.store().load_proposal(&kept).await.unwrap();
            let provider = move_model();
            let (_governor, client) = client(Some("synthetic-dedupe"), provider);
            let deps = AnswerDeps {
                client: &client,
                route: None,
            };
            let ports = Ports::default();
            let ctx = world.context(&step(
                asking,
                "propose_move",
                json!({"to_when": "mon 22:00"}),
            ));
            let (guild, mut proposer) = world.question_parts();
            let generation = answer(&deps, question(&ctx), &guild, &mut proposer, &ports).await;
            assert!(generation.failure.is_none(), "{:?}", generation.failure);
            assert!(generation.created.is_empty() && generation.posted.is_empty());
            assert!(ports.posted.lock().unwrap().is_empty());
            assert_eq!(*ports.refreshed.lock().unwrap(), vec![old.clone()]);
            let discarded = world
                .service
                .store()
                .load_proposal(&old)
                .await
                .unwrap()
                .unwrap()
                .0
                .draft;
            assert_eq!(discarded.status, DraftStatus::Discarded);
            assert_eq!(discarded.close_reason.as_deref(), Some("superseded"));
            assert_eq!(
                world.service.store().load_proposal(&kept).await.unwrap(),
                before,
                "the kept match is never retired or modified"
            );
            assert_eq!(
                world
                    .service
                    .store()
                    .list_proposals(false)
                    .await
                    .unwrap()
                    .len(),
                2
            );
            assert!(
                generation.outcomes[0]
                    .outcome
                    .output
                    .contains("Already proposed")
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_timed_out_cardless_chat_proposal_does_not_block_a_later_cross_channel_ask() {
    let mut world = world().await;
    let provider = move_model();
    let (_governor, client) = client(Some("synthetic-dedupe"), provider);
    let deps = AnswerDeps {
        client: &client,
        route: None,
    };
    let ctx = world.context(&step(
        "900",
        "propose_move",
        json!({"to_when": "mon 22:00"}),
    ));
    let ports = Ports::default();
    let generation = {
        let mut slow = SchedulerService::new(
            SlowStore {
                inner: world.service.store(),
                stall: Duration::from_secs(61),
            },
            world.ids.clone(),
            world.clock.clone(),
        );
        let mut proposer = Proposer {
            service: &mut slow,
            policy: &world.policy,
        };
        answer(
            &deps,
            question(&ctx),
            &world.guild_view(),
            &mut proposer,
            &ports,
        )
        .await
    };
    assert_eq!(
        generation.failure,
        Some(AnswerFailure::Timeout { seconds: 60 })
    );
    assert_eq!(generation.created.len(), 1);
    assert!(ports.posted.lock().unwrap().is_empty());
    let id = &generation.created[0];
    assert!(
        world
            .service
            .store()
            .load_cards(std::slice::from_ref(id))
            .await
            .unwrap()
            .is_empty()
    );
    let before = world.service.store().load_proposal(id).await.unwrap();
    world
        .clock
        .set(crate::common::instant(&json!("2026-10-02T12:02:00+00:00")));
    let outcome = world
        .run_tool(
            &step("901", "propose_move", json!({"to_when": "mon 22:00"})),
            &mut PassthroughSession,
        )
        .await;
    assert!(outcome.ok, "{}", outcome.output);
    assert_eq!(outcome.created.len(), 1);
    assert_eq!(outcome.cards.len(), 1);
    assert_eq!(
        world.service.store().load_proposal(id).await.unwrap(),
        before,
        "the original remains available to the admin Inbox"
    );
}
