//! Scenarios over a live generation (`controls_p6`): changes made outside
//! the page under test, as the actor who would make them, written through
//! the scheduler, the drafts service and the stores' own APIs. Every write
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
use kanade::{
    api::{dto::events::Topic, state::ApiState},
    infrastructure::store::SqliteStore,
};
use kanade_domain::{
    drafts::{DraftOp, DraftStore, ProposalSource, Target},
    history::{Actor, ChangeHistory, Origin, Surface},
    members::MemberStore,
    model_log::{
        ChatInteraction, ChatOutcome, ExtractionLog, ExtractionOutcome, ModelLogStore,
        WatchedMessage,
    },
    proposals::{CardDetails, CardPayload, ChangeKind, ProposalCardStore, ProposedChange},
    schedule::{
        Change, FixedEdit, FixedEditChoices, NewRun, Rsvp, RsvpSource, RsvpState, RunSource,
        RunStatus, StatusChange, utc_instant,
    },
    scheduler::{ProposalRequest, ScheduleStore, Scope, Supersede},
};
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

/// The admin every spec browses as: drafts are their author's own.
fn me() -> String {
    format!("discord:{ASAHI}")
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

    async fn version(&self, draft: &str) -> Result<u64, Fail> {
        Ok(self
            .store
            .load_draft(draft)
            .await?
            .ok_or("no such draft")?
            .draft
            .version)
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

    /// Admin `who` cancels `run` from the portal at `at`.
    async fn cancel(
        &self,
        who: &str,
        run: &str,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), Fail> {
        let change = StatusChange {
            status: RunStatus::Cancelled,
            announce: false,
            via_portal: true,
        };
        self.scheduler(at, &[])
            .as_origin(Origin::new(admin(who), Surface::AdminPortal))
            .set_status(run, change, &self.state.policy.reminders)
            .await
            .map(drop)
            .map_err(refused("cancelling the run"))
    }

    /// Asahi starts draft `id` titled `title` at `at`, staging `ops`; its version.
    async fn draft(
        &self,
        id: &str,
        title: &str,
        at: chrono::DateTime<chrono::Utc>,
        ops: Vec<DraftOp>,
    ) -> Result<u64, Fail> {
        let me = me();
        let mut version = self
            .scheduler(at, &[id])
            .create_draft(&me, title, None)
            .await
            .map_err(refused("starting the draft"))?
            .version;
        for op in ops {
            version = self.stage(id, version, op, at).await?;
        }
        Ok(version)
    }

    async fn stage(
        &self,
        id: &str,
        version: u64,
        op: DraftOp,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64, Fail> {
        let directory = self.directory().await?;
        Ok(self
            .scheduler(at, &[])
            .add_draft_op(&me(), id, version, op, &self.state.policy, &directory)
            .await
            .map_err(refused("staging in the draft"))?
            .draft
            .version)
    }

    async fn discard(&self, id: &str, at: chrono::DateTime<chrono::Utc>) -> Result<(), Fail> {
        let version = self.version(id).await?;
        self.scheduler(at, &[])
            .discard_draft(&me(), id, version, None)
            .await
            .map(drop)
            .map_err(refused("discarding the draft"))
    }

    async fn merge(&self, id: &str, at: chrono::DateTime<chrono::Utc>) -> Result<(), Fail> {
        let version = self.version(id).await?;
        let directory = self.directory().await?;
        self.scheduler(at, &[])
            .merge_draft(&me(), id, version, &self.state.policy, &directory, None)
            .await
            .map(drop)
            .map_err(refused("merging the draft"))
    }

    async fn rebase(&self, id: &str, at: chrono::DateTime<chrono::Utc>) -> Result<(), Fail> {
        let version = self.version(id).await?;
        let directory = self.directory().await?;
        self.scheduler(at, &[])
            .rebase_draft(&me(), id, version, &self.state.policy, &directory)
            .await
            .map(drop)
            .map_err(refused("rebasing the draft"))
    }
}

fn amend_op(run: &str, to: chrono::DateTime<chrono::Utc>) -> DraftOp {
    DraftOp::AmendRun {
        run: Target::Existing(run.into()),
        to,
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

/// `POST /__e2e/drafts/board`: the mock's drafts on the seeded week. The
/// closed Scratch (discarded) and Tuesday Kalos (merged, next week's XKalos
/// to Tue 22:00: the record after the seed's head); Carling retime three
/// records behind with two conflicts (Ren moved HCarling + HMaleficStar
/// since, and cancelled next week's HFA, which it re-parties and confirms);
/// Weekend reshuffle rebased onto the head, so up to date.
pub async fn drafts_board(seed: &Seed<'_>) -> Result<(), Fail> {
    let at = |minutes: i64| seed.now - Duration::minutes(60 - minutes);
    seed.draft("d-scratch", "Scratch", at(0), Vec::new())
        .await?;
    seed.discard("d-scratch", at(1)).await?;
    seed.draft(
        "d-tuesday",
        "Tuesday Kalos",
        at(2),
        vec![amend_op("n-kalos", seed.at(1, 5, Some("22:00"))?)],
    )
    .await?;
    seed.merge("d-tuesday", at(3)).await?;
    let mut version = seed.version("d-carling").await?;
    for op in [
        DraftOp::SwapParticipants {
            run: Target::Existing("n-fa".into()),
            remove: Vec::new(),
            add: vec![MIKA.into()],
            via_portal: true,
        },
        DraftOp::SetStatus {
            run: Target::Existing("n-fa".into()),
            change: StatusChange {
                status: RunStatus::Confirmed,
                announce: false,
                via_portal: true,
            },
        },
    ] {
        version = seed.stage("d-carling", version, op, at(4)).await?;
    }
    seed.amend(REN, "r-carling", seed.at(0, 6, Some("20:00"))?, at(5))
        .await?;
    seed.cancel(REN, "n-fa", at(6)).await?;
    seed.rebase("d-weekend", at(7)).await
}

/// `POST /__e2e/graph/board`: the History graph boards. History grows from
/// the seed's head (#10) to #39 with members' answers and the board's
/// changes, and the drafts take the boards' lanes: an untitled draft from
/// #12 (discarded between #24 and #25), Tuesday Kalos (#31, merged as #33),
/// Scratch (#32, discarded after #33), Kalos to Saturdays (#34, weekly
/// timings), Weekend reshuffle (a HCarling + HMaleficStar move staged, then
/// rebased onto #36; Kaito cancels XBM in #38 and Ren moves HCarling +
/// HMaleficStar in #39: two conflicts) and Bellona clean-up (#38; #39: one
/// conflict). The store keeps
/// every draft, so the seeded Carling retime plays Bellona clean-up
/// (retitled, its change re-aimed, rebased onto #38) rather than leaving
/// a lane open from #10.
pub async fn graph_board(seed: &Seed<'_>) -> Result<(), Fail> {
    // Records from Mon 28 19:00 (after the seed's last), nine minutes apart.
    let start = seed.at(0, 4, Some("19:00"))?;
    let tick = |n: i64| start + Duration::minutes(9 * n);
    let head = || async { Ok::<u64, Fail>(seed.store.history_head().await?.seq) };
    // Members answering in Discord, on runs no draft stages.
    const FILLER: [(&str, &str); 4] = [
        ("r-fa", HOTARU),
        ("r-jupiter", SORA),
        ("r-limbo", SORA),
        ("r-bellona", KAITO),
    ];
    let mut n = 0;
    let filler = |n: &mut i64| {
        let (run, who) = FILLER[(*n as usize) % FILLER.len()];
        *n += 1;
        (run, who, tick(*n))
    };
    while head().await? < 12 {
        let (run, who, at) = filler(&mut n);
        seed.flip(run, who, at).await?;
    }
    seed.draft(
        "d-untitled",
        "Untitled draft",
        tick(n) + Duration::minutes(1),
        vec![amend_op("n-fa", seed.at(1, 3, Some("20:00"))?)],
    )
    .await?;
    while head().await? < 30 {
        let (run, who, at) = filler(&mut n);
        seed.flip(run, who, at).await?;
        if head().await? == 24 {
            seed.discard("d-untitled", at + Duration::minutes(4))
                .await?;
        }
    }
    let board = seed.at(0, 4, Some("22:00"))?;
    let at = |minutes: i64| board + Duration::minutes(minutes);
    seed.flip("r-kalos", TSUBAME, at(0)).await?; // #31
    seed.draft(
        "d-tuesday",
        "Tuesday Kalos",
        at(5),
        vec![amend_op("n-kalos", seed.at(1, 5, Some("22:30"))?)],
    )
    .await?;
    seed.flip("r-kalos", REN, at(30)).await?; // #32
    seed.draft("d-scratch", "Scratch", at(35), Vec::new())
        .await?;
    seed.merge("d-tuesday", at(80)).await?; // #33
    seed.discard("d-scratch", at(120)).await?;
    seed.flip("r-bellona", KAITO, at(185)).await?; // #34
    seed.draft(
        "d-kalos",
        "Kalos to Saturdays",
        at(190),
        vec![
            DraftOp::ApplyFixedEdit {
                fixed: Target::Existing("f-kalos".into()),
                edit: FixedEdit {
                    weekday: Some(Weekday::Sat),
                    ..FixedEdit::default()
                },
                choices: FixedEditChoices::default(),
            },
            DraftOp::FixedParticipants {
                fixed: Target::Existing("f-kalos".into()),
                add: vec![YUZU.into()],
                remove: Vec::new(),
            },
        ],
    )
    .await?;
    seed.flip("r-jupiter", MINATO, at(250)).await?; // #35
    seed.flip("r-jupiter", ASAHI, at(340)).await?; // #36
    // The board's Weekend reshuffle also moves HCarling + HMaleficStar, which
    // Ren moves after it (#39): with XBM cancelled (#38), two conflicts.
    let version = seed.version("d-weekend").await?;
    seed.stage(
        "d-weekend",
        version,
        amend_op("r-carling", seed.at(0, 6, Some("21:30"))?),
        at(342),
    )
    .await?;
    seed.rebase("d-weekend", at(345)).await?;
    seed.amend(REN, "r-bm", seed.at(0, 6, Some("22:00"))?, at(690))
        .await?; // #37
    seed.cancel(KAITO, "r-bm", at(700)).await?; // #38
    let version = seed.version("d-carling").await?;
    let version = seed
        .scheduler(at(705), &[])
        .retitle_draft(&me(), "d-carling", version, "Bellona clean-up")
        .await
        .map_err(refused("retitling the draft"))?
        .version;
    let directory = seed.directory().await?;
    seed.scheduler(at(705), &[])
        .edit_draft_op(
            &me(),
            "d-carling",
            version,
            0,
            amend_op("r-carling", seed.at(0, 6, Some("22:30"))?),
            &seed.state.policy,
            &directory,
        )
        .await
        .map_err(refused("re-aiming the draft"))?;
    seed.rebase("d-carling", at(706)).await?;
    // The tips list the most recently edited first: the board's order.
    let version = seed.version("d-weekend").await?;
    seed.scheduler(at(708), &[])
        .retitle_draft(&me(), "d-weekend", version, "Weekend reshuffle")
        .await
        .map_err(refused("touching the draft"))?;
    seed.amend(REN, "r-carling", seed.at(0, 6, Some("21:00"))?, at(720))
        .await?; // #39
    if head().await? != 39 {
        return Err(format!("the board ended at #{}, not #39", head().await?).into());
    }
    Ok(())
}
