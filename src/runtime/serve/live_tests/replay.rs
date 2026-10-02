use super::*;
use crate::bot::transport::MessageId;
use crate::domain::proposals::ProposalCardStore;
use twilight_model::channel::message::ReactionType;

fn now() -> DateTime<Utc> {
    "2026-08-30T05:07:00Z".parse().unwrap()
}

async fn pending(harness: &Harness, ctx: &Ctx, source: ProposalSource) -> (String, MessageId) {
    let policy = policy(harness);
    let at = now() + chrono::Duration::hours(30);
    let run = create_run(&ctx.store, &policy, now(), HOME_C, at).await;
    let mut roster = Roster::new();
    roster.upsert(crate::domain::members::Member {
        user_id: ALICE.to_string(),
        has_role: true,
        ..Default::default()
    });
    let change = ProposedChange {
        run_id: Some(run.clone()),
        channel_id: Some(HOME_C.to_string()),
        bosses: vec!["Kalos".into()],
        participants: vec![ALICE.to_string()],
        new_datetime: Some(at + chrono::Duration::hours(1)),
        ..ProposedChange::new(ChangeKind::Move)
    };
    let proposed = SchedulerService::new(StoreRef(&*ctx.store), RandomIds, FixedClock(now()))
        .with_attendance(policy.attendance)
        .propose(
            ProposalRequest {
                change: change.clone(),
                source,
                source_id: "synthetic-offline".into(),
                supersede: Supersede::Keep,
            },
            &policy,
            &roster,
        )
        .await
        .unwrap();
    let id = proposed.proposal.id;
    let desk = CardDesk::new(
        DeskDeps {
            store: Arc::clone(&ctx.store),
            transport: Arc::clone(&harness.fake),
            ids: RandomIds,
            clock: Arc::new(FixedClock(now())),
            directory: Arc::new(roster),
            authority: Arc::new(AnyAuthority),
            alerts: Arc::new(LogAlerts),
            decline_retraction: None,
        },
        CardSettings {
            zone: policy.zone(),
            policy,
            instance_id: "seed".into(),
        },
    );
    assert_eq!(
        desk.post_card(&Card {
            channel_id: HOME_C.to_string(),
            superseded: Vec::new(),
            entries: vec![CardEntry {
                proposal_id: id.clone(),
                change,
                kind: AmendmentKind::Move,
                run_id: Some(run),
                summary: "an hour later".into(),
                is_question: false,
                needs_answer: false,
                confidence: 0.9,
                also_mentioned: Vec::new(),
                day_ref: None,
                time_ref: None,
                evidence_message_ids: vec!["101".into()],
                self_service: None,
            }],
        })
        .await,
        PostResult::Posted
    );
    let message = ctx
        .store
        .load_cards(std::slice::from_ref(&id))
        .await
        .unwrap()[0]
        .message_id
        .clone()
        .unwrap();
    (id, Id::new(message.parse().unwrap()))
}

async fn status(ctx: &Ctx, id: &str) -> DraftStatus {
    ctx.store
        .load_proposal(id)
        .await
        .unwrap()
        .unwrap()
        .0
        .draft
        .status
}

fn seed_members(harness: &Harness) {
    harness.fake.seed_members(
        Id::new(GUILD),
        vec![guild_member(ALICE, "alice", false, &[BOSSING])],
    );
}

#[tokio::test]
async fn offline_cards_replay_after_startup_and_second_ready_only_after_current_roster() {
    let harness = Harness::new();
    seed_members(&harness);
    let (mut discord, ctx) = harness.start_with_clock(Arc::new(now)).await;
    drive(&mut discord, async {
        for source in [ProposalSource::Chat, ProposalSource::Extraction] {
            let (id, message) = pending(&harness, &ctx, source).await;
            harness.fake.seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(ALICE)]);
            let reads = harness.fake.count(Op::ReactionUsers);
            let roster = harness.fake.hold(Op::ListMembers);
            ctx.events.send(ready()).unwrap();
            ctx.events.send(guild_create(&[HOME_C])).unwrap();
            roster.entered().await;
            assert_eq!(harness.fake.count(Op::ReactionUsers), reads, "waits for this READY's reconciliation, not an old snapshot");
            assert_eq!(status(&ctx, &id).await, DraftStatus::Submitted);
            roster.release();
            eventually!("offline rejection", status(&ctx, &id).await == DraftStatus::Rejected);
            eventually!("card refreshed", harness.fake.calls().iter().any(|call| matches!(call, Call::Edit { message: edited, edit, .. } if *edited == message && edit.content.as_deref().is_some_and(|text| text.contains("❌ rejected by alice")))));
        }
        assert_eq!(harness.fake.count(Op::Create), 2, "no stale chat follow-up");
        let (id, message) = pending(&harness, &ctx, ProposalSource::Chat).await;
        harness.fake.seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(ALICE)]);
        let reads = harness.fake.count(Op::ReactionUsers);
        ctx.events.send(Event::GatewayClose(None)).unwrap();
        ctx.events.send(Event::Resumed).unwrap();
        eventually!("resume", ctx.connection.get() == Connection::Ready);
        sleep(TICK * 4).await;
        assert_eq!(harness.fake.count(Op::ReactionUsers), reads, "RESUMED uses gateway replay, never another HTTP pass");
        assert_eq!(status(&ctx, &id).await, DraftStatus::Submitted);
    }).await;
    finish(&harness, ctx).await;
}

#[tokio::test]
async fn later_ready_coalesces_while_http_replay_is_in_flight() {
    let harness = Harness::new();
    seed_members(&harness);
    let (mut discord, ctx) = harness.start_with_clock(Arc::new(now)).await;
    drive(&mut discord, async {
        let (first, message) = pending(&harness, &ctx, ProposalSource::Chat).await;
        harness
            .fake
            .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(ALICE)]);
        let http = harness.fake.hold(Op::ReactionUsers);
        ctx.events.send(ready()).unwrap();
        ctx.events.send(guild_create(&[HOME_C])).unwrap();
        http.entered().await;
        let (second, message) = pending(&harness, &ctx, ProposalSource::Extraction).await;
        harness
            .fake
            .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(ALICE)]);
        ctx.events.send(ready()).unwrap();
        ctx.events.send(guild_create(&[HOME_C])).unwrap();
        eventually!(
            "second reconcile",
            harness.fake.count(Op::ListMembers) == 2 && ctx.connection.delivery_claims_allowed()
        );
        assert_eq!(
            harness.fake.count(Op::ReactionUsers),
            0,
            "no concurrent HTTP pass while the first is parked"
        );
        http.release();
        eventually!(
            "latest pass decides both",
            status(&ctx, &first).await == DraftStatus::Rejected
                && status(&ctx, &second).await == DraftStatus::Rejected
        );
    })
    .await;
    finish(&harness, ctx).await;
}

#[tokio::test]
async fn held_replay_finishes_all_cards_across_gateway_close_and_resume() {
    for resume_before_release in [false, true] {
        let harness = Harness::new();
        seed_members(&harness);
        let (mut discord, ctx) = harness.start_with_clock(Arc::new(now)).await;
        drive(&mut discord, async {
            let (first, first_message) = pending(&harness, &ctx, ProposalSource::Chat).await;
            let (second, second_message) =
                pending(&harness, &ctx, ProposalSource::Extraction).await;
            for message in [first_message, second_message] {
                harness.fake.seed_reactions(
                    message,
                    "❌",
                    ReactionType::Normal,
                    vec![Id::new(ALICE)],
                );
            }
            let http = harness.fake.hold(Op::ReactionUsers);
            ctx.events.send(ready()).unwrap();
            ctx.events.send(guild_create(&[HOME_C])).unwrap();
            http.entered().await;
            ctx.events.send(Event::GatewayClose(None)).unwrap();
            eventually!("close", ctx.connection.get() == Connection::Disconnected);
            if resume_before_release {
                ctx.events.send(Event::Resumed).unwrap();
                eventually!("resume", ctx.connection.get() == Connection::Ready);
            }
            http.release();
            eventually!(
                "both offline cards rejected",
                status(&ctx, &first).await == DraftStatus::Rejected
                    && status(&ctx, &second).await == DraftStatus::Rejected
            );
            if !resume_before_release {
                assert_eq!(
                    ctx.connection.get(),
                    Connection::Disconnected,
                    "HTTP replay keeps working while disconnected"
                );
                ctx.events.send(Event::Resumed).unwrap();
            }
        })
        .await;
        finish(&harness, ctx).await;
    }
}
