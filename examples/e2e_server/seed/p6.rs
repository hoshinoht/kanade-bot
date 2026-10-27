//! Scenarios over a live generation (`controls_p6`): changes made outside
//! the page under test, as the actor who would make them, written through
//! the scheduler and the stores' own APIs. Every write
//! commits through the live store, so its write hook sends the hints a real
//! change sends; nothing here notifies by hand except where no store write
//! exists (the chat window).
//!
//! The mock's scenarios named runs that have ended by the pinned instant
//! (Tue 29 12:00, guild time); the real scheduler refuses moving or adding
//! those, so these use runs still ahead (HCarling + HMaleficStar, XBM,
//! next week's runs) and Wednesday slots.

use std::{fmt::Debug, sync::Arc};

use chrono::{Duration, Weekday};
use kanade::api::{dto::events::Topic, state::ApiState};
use kanade_domain::{
    drafts::{DraftOp, DraftStore, ProposalSource},
    history::{Actor, Origin, Surface},
    members::MemberStore,
    model_log::{
        ChatInteraction, ChatOutcome, ExtractionLog, ExtractionOutcome, ModelLogStore,
        WatchedMessage,
    },
    proposals::{CardDetails, CardPayload, ChangeKind, ProposalCardStore, ProposedChange},
    schedule::{Change, NewRun, Rsvp, RsvpSource, RsvpState, RunSource, RunStatus, utc_instant},
    scheduler::{ProposalRequest, ScheduleStore, Scope, Supersede},
};
use kanade_store::SqliteStore;
use serde_json::{Map, Value};

use super::{Seed, ids::*};
use crate::Fail;

/// The seed's builders over a live generation, at its clock's now.
pub fn live(store: Arc<SqliteStore>, state: &ApiState) -> Result<Seed<'_>, Fail> {
    let now = state.now();
    let week = utc_instant(&state.policy.week_of(&now)?)?;
    Ok(Seed {
        store,
        state,
        week,
        now,
    })
}

fn refused<E: Debug>(what: &'static str) -> impl FnOnce(E) -> Fail {
    move |error| format!("{what}: {error:?}").into()
}

fn admin(id: &str) -> Actor {
    Actor::admin(format!("discord:{id}"))
}

impl Seed<'_> {
    /// `user`'s answer on `run` now, if any.
    async fn answer(&self, run: &str, user: &str) -> Result<Option<RsvpState>, Fail> {
        let snapshot = self.store.load(&Scope::Run(run.into())).await?;
        Ok(snapshot
            .rsvps
            .iter()
            .find(|rsvp| rsvp.user_id == user)
            .map(|rsvp| rsvp.state))
    }

    /// `user` reacts on `run`'s card in Discord with `state` at `at`.
    async fn react(
        &self,
        run: &str,
        user: &str,
        state: RsvpState,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), Fail> {
        self.commit(
            Actor::member(user),
            Surface::Discord,
            at,
            vec![Change::PutRsvp(Rsvp {
                run_id: run.into(),
                user_id: user.into(),
                state,
                source: RsvpSource::Reaction,
                at,
            })],
            Vec::new(),
        )
        .await
        .map(drop)
    }

    /// `user` flips their answer on `run` between in and maybe.
    async fn flip(
        &self,
        run: &str,
        user: &str,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), Fail> {
        let state = match self.answer(run, user).await? {
            Some(RsvpState::Yes) => RsvpState::Maybe,
            _ => RsvpState::Yes,
        };
        self.react(run, user, state, at).await
    }

    /// Admin `who` moves `run` to `to` from the portal at `at`.
    async fn amend(
        &self,
        who: &str,
        run: &str,
        to: chrono::DateTime<chrono::Utc>,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), Fail> {
        self.scheduler(at, &[])
            .as_origin(Origin::new(admin(who), Surface::AdminPortal))
            .amend_run(run, to, &self.state.policy)
            .await
            .map(drop)
            .map_err(refused("moving the run"))
    }
}

/// `POST /__e2e/arrive {kind}`: something changes outside the portal.
/// `None` for an unknown kind.
pub async fn arrive(seed: &Seed<'_>, kind: &str) -> Option<Result<(), Fail>> {
    Some(match kind {
        // Tsubame's ✅ on XKalos.
        "reaction" => {
            seed.react("r-kalos", TSUBAME, RsvpState::Yes, seed.now)
                .await
        }
        // Another admin (Ren) moves XBM from Tue 23:30 to Wed 21:00.
        "move" => match seed.at(0, 6, Some("21:00")) {
            Ok(to) => seed.amend(REN, "r-bm", to, seed.now).await,
            Err(error) => Err(error),
        },
        "run" => arrive_run(seed).await,
        "proposal" => arrive_proposal(seed).await,
        "chat" => arrive_chat(seed).await,
        "extraction" => arrive_extraction(seed).await,
        "member" => arrive_alias(seed).await,
        _ => return None,
    })
}

/// Another admin (Ren) adds a Normal Limbo run on Wed 30 20:00.
async fn arrive_run(seed: &Seed<'_>) -> Result<(), Fail> {
    let new = NewRun {
        fixed_run_id: None,
        channel_id: Some(LIMBO_TRIO.into()),
        week_start: seed.week,
        datetime: seed.at(0, 6, Some("20:00"))?,
        bosses: vec!["NLimbo".into()],
        participants: vec![MIKA.into(), NAGI.into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
    };
    seed.scheduler(seed.now, &["r-arrived"])
        .as_origin(Origin::new(admin(REN), Surface::AdminPortal))
        .create_run(new)
        .await
        .map(drop)
        .map_err(refused("adding the run"))
}

/// The extractor reads a new question from #limbo-trio: Mika floats a
/// Normal Limbo on Wednesday.
async fn arrive_proposal(seed: &Seed<'_>) -> Result<(), Fail> {
    let read_at = seed.now - Duration::minutes(1);
    let said = read_at - Duration::minutes(2);
    let message = Seed::snowflake(said, 0x4000);
    seed.store
        .upsert_message(WatchedMessage {
            id: message.clone(),
            channel_id: LIMBO_TRIO.into(),
            author_id: MIKA.into(),
            created_at: said,
            edited_at: None,
            content: "nlimbo wed 9pm?".into(),
            processed_at: Some(read_at),
        })
        .await?;
    let new_datetime = Some(seed.at(0, 6, Some("21:00"))?);
    let change = ProposedChange {
        run_id: None,
        channel_id: Some(LIMBO_TRIO.into()),
        bosses: vec!["NLimbo".into()],
        participants: vec![MIKA.into()],
        new_datetime,
        ..ProposedChange::new(ChangeKind::Add)
    };
    let directory = seed.directory().await?;
    seed.scheduler(read_at, &["p-arrived"])
        .propose(
            ProposalRequest {
                change,
                source: ProposalSource::Extraction,
                source_id: "x-arrived".into(),
                supersede: Supersede::Keep,
            },
            &seed.state.policy,
            &directory,
        )
        .await
        .map_err(refused("proposing"))?;
    let details = CardDetails {
        kind: ChangeKind::Add,
        run_id: None,
        bosses: vec!["NLimbo".into()],
        participants: vec![MIKA.into()],
        new_datetime,
        day_ref: None,
        time_ref: None,
        rsvp: None,
        is_question: true,
        summary: Some("Mika asks whether anyone is up for Normal Limbo on Wednesday.".into()),
        also_mentioned: Vec::new(),
        confidence: 0.52,
        payload: CardPayload::default(),
        evidence_message_ids: vec![message],
        self_service: None,
    };
    seed.store
        .save_card("p-arrived", LIMBO_TRIO, &details, read_at)
        .await?;
    Ok(())
}

/// The chatbot answers Mika in #limbo-trio.
async fn arrive_chat(seed: &Seed<'_>) -> Result<(), Fail> {
    seed.store
        .record_chat(ChatInteraction {
            id: "c-arrived".into(),
            at: seed.now - Duration::minutes(1),
            channel_id: Some(LIMBO_TRIO.into()),
            message_id: None,
            member_id: Some(MIKA.into()),
            question: format!("<@{BOT}> is limbo still on tonight?"),
            reply: "Hard Limbo is tonight at 23:30, 2 of 3 on so far.".into(),
            outcome: ChatOutcome::Answered,
            error: None,
            clean_retry: false,
            withheld: false,
            guardrail: Value::Object(Map::new()),
            request_count: 0,
            latency_ms: Some(1_950),
            model_ms: None,
            tools_ms: None,
            prompt_tokens: None,
            completion_tokens: None,
            cached_tokens: None,
            rounds: Vec::new(),
            persona: Some("kanade".into()),
            profile: None,
            profile_source: Some("default".into()),
            error_code: None,
            session_id: None,
        })
        .await?;
    Ok(())
}

/// The extractor reads #limbo-trio and finds nothing to change.
async fn arrive_extraction(seed: &Seed<'_>) -> Result<(), Fail> {
    let at = seed.now - Duration::minutes(1);
    let mut ids = Vec::new();
    for (n, (who, text)) in (0_u64..).zip([(MIKA, "limbo still 11:30 tonight?"), (NAGI, "yes")]) {
        let sent = at - Duration::minutes(3 - n as i64);
        let id = Seed::snowflake(sent, 0x4100 + n);
        seed.store
            .upsert_message(WatchedMessage {
                id: id.clone(),
                channel_id: LIMBO_TRIO.into(),
                author_id: who.into(),
                created_at: sent,
                edited_at: None,
                content: text.into(),
                processed_at: Some(at),
            })
            .await?;
        ids.push(id);
    }
    seed.store
        .record_extraction(ExtractionLog {
            id: "x-arrived".into(),
            at,
            channel_id: Some(LIMBO_TRIO.into()),
            member_ids: vec![MIKA.into(), NAGI.into()],
            model: "kanata/extract".into(),
            reasoning: Some("low".into()),
            reasoning_content: None,
            reasoning_tokens: None,
            prompt: "System: You extract boss-schedule amendments from party chat. Reply with JSON only.\n\nMessages:\n[Mika] limbo still 11:30 tonight?\n[Nagi] yes".into(),
            raw_response: r#"{"amendments":[],"summary":"…"}"#.into(),
            latency_ms: Some(9_420),
            request_count: 1,
            outcome: ExtractionOutcome::NoChange,
            error: None,
            guardrail: Value::Object(Map::new()),
            message_ids: ids,
            proposal_ids: Vec::new(),
            refusals: Vec::new(),
            prompt_tokens: Some(1_410),
            completion_tokens: Some(22),
            cached_tokens: None,
            prompt_estimate: Some(1_350),
            session_id: None,
            request_ids: Vec::new(),
        })
        .await?;
    Ok(())
}

/// Mika's roster row gains the alias `mikan`.
async fn arrive_alias(seed: &Seed<'_>) -> Result<(), Fail> {
    let mut profile = seed.store.load_member(MIKA).await?.ok_or("no Mika")?;
    if profile.aliases.iter().any(|alias| alias == "mikan") {
        return Err("Mika already has the alias".into());
    }
    profile.aliases.push("mikan".into());
    seed.store.put_member(profile).await?;
    Ok(())
}

/// `POST /__e2e/public/hint {topic, member?}`: a change elsewhere that the
/// member stream turns into `topic` for `member`. Member hints are decided
/// from what changed, so a bare hub hint would tell them nothing:
/// `schedule` is Sora's answer on XBM (neither Asahi's nor Ren's run),
/// `mine` the member's own reaction on HCarling + HMaleficStar from another
/// device (only a row of theirs is `mine`: someone else's answer on their run
/// is `schedule` alone), and `allowance` one chatbot
/// answer spent from the member's chat window (a pilot's: staff have no
/// allowance to move), which lives in the chat driver's memory, not the
/// store, so the hint is sent here as a logged interaction would send it.
/// `None` for an unknown topic.
pub async fn member_hint(seed: &Seed<'_>, topic: &str, member: &str) -> Option<Result<(), Fail>> {
    Some(match topic {
        "schedule" => seed.flip("r-bm", SORA, seed.now).await,
        "mine" => seed.flip("r-carling", member, seed.now).await,
        "allowance" => match &seed.state.chat {
            Some(chat) => match chat.spend_allowance(member) {
                Some(true) => {
                    seed.state.events.notify(Topic::Chat);
                    Ok(())
                }
                Some(false) => Err("the member's chat window is full".into()),
                None => Err("chat is not running".into()),
            },
            None => Err("chat is not running".into()),
        },
        _ => return None,
    })
}

/// `POST /__e2e/public/remove {run}`: an admin (Ren) takes Asahi off `run`.
pub async fn remove_asahi(seed: &Seed<'_>, run: &str) -> Result<(), Fail> {
    let directory = seed.directory().await?;
    seed.scheduler(seed.now, &[])
        .as_origin(Origin::new(admin(REN), Surface::AdminPortal))
        .swap_participants(run, &[ASAHI.into()], &[], true, &directory)
        .await
        .map(drop)
        .map_err(refused("taking Asahi off the run"))
}

/// `POST /__e2e/inbox/kalos-v4`: `p-kalos-fixed` changes under an admin
/// editing it: Tsubame's Saturday becomes Sunday (22:30 kept). Requesters
/// never edit their requests on the real server (they withdraw and ask
/// again), so the change is another admin's edit (Ren), the one write that
/// moves a request's version while it stays open.
pub async fn kalos_v4(seed: &Seed<'_>) -> Result<u64, Fail> {
    let id = "p-kalos-fixed";
    let loaded = seed.store.load_draft(id).await?.ok_or("no p-kalos-fixed")?;
    let mut ops = loaded.draft_ops();
    let edit = ops
        .iter_mut()
        .find_map(|op| match op {
            DraftOp::ApplyFixedEdit { edit, .. } => Some(edit),
            _ => None,
        })
        .ok_or("p-kalos-fixed changes no day")?;
    edit.weekday = Some(Weekday::Sun);
    let directory = seed.directory().await?;
    let edited = seed
        .scheduler(seed.now, &[])
        .edit_request(
            &admin(REN),
            id,
            loaded.draft.version,
            ops,
            &seed.state.policy,
            &directory,
        )
        .await
        .map_err(refused("editing p-kalos-fixed"))?;
    Ok(edited.draft.version)
}
