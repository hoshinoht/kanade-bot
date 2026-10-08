//! The `propose_*` tools (v4 `tools/propose_*.py`): validate what the model
//! asked for, check authority, then stage a proposal through the scheduler's
//! propose API. They never write a schedule row. A change the scheduler
//! cannot apply is refused up front with its reason and no card
//! (`D-PROPOSE-REFUSES`); v4 posted a card that failed at ✅.

mod card;
pub mod when;

use chrono::{DateTime, Datelike, NaiveTime, Utc};
use serde_json::{Map, Value, json};

pub use card::{FIX_EDIT, FIX_REMOVE, ProposalCard, card_ready, kind_label};

use crate::chat::authority::{Asker, Subject, require_authority};
use crate::chat::tools::read::ToolWorld;
use crate::chat::tools::read::format::{
    boss_labels, fixed_when, hhmm, local, weekday_name, when_label,
};
use crate::chat::tools::read::participants::{
    is_true, new_party, py_text, validate_bosses, validate_participants,
};
use crate::chat::tools::read::resolve::{require_heard, resolve_fixed, resolve_run};
use crate::chat::tools::{CallError, ToolContext, ToolError};
use crate::domain::drafts::{ProposalSource, ProposalStore};
use crate::domain::ids::short_id;
use crate::domain::members::member_name;
use crate::domain::proposals::{ChangeKind, Payload, ProposedChange};
use crate::domain::pytext::strip;
use crate::domain::schedule::{RsvpState, Run, RunStatus, SchedulePolicy, utc_instant};
use crate::domain::scheduler::{
    ChatProposed, Clock, IdSource, ProposalError, ProposalRequest, ScheduleStore, SchedulerService,
    Supersede, SupersedeScope,
};
use crate::domain::weeks::{parse_hhmm, parse_weekday, week_start};
use card::{card_party, card_when, names};

/// The scheduler refused the change (v4 would have carded it and failed at ✅).
const REFUSED_UP_FRONT: &str = "That cannot be proposed: {reason}. No card went up -- tell them why in your own words, and never say a card is up.";
const NO_EFFECT: &str =
    "Nothing would change -- that is already the case. No card went up; tell them so.";
const EXPIRED: &str = "That falls in a boss week that is already over, so it cannot be proposed. No card went up; tell them so.";

/// The scheduler a proposal is staged through.
pub struct Proposer<'a, S, I, C> {
    pub service: &'a mut SchedulerService<S, I, C>,
    pub policy: &'a SchedulePolicy,
}

pub enum ProposalReply {
    Created(Box<ProposalCard>),
    Existing {
        output: String,
        superseded: Vec<String>,
    },
}

type Proposed = Result<ProposalReply, CallError>;

/// What one tool wants staged, before the scheduler sees it.
struct Plan<'r> {
    kind: ChangeKind,
    run: Option<&'r Run>,
    at: Option<DateTime<Utc>>,
    bosses: Vec<String>,
    participants: Vec<String>,
    rsvp: Option<RsvpState>,
    payload: Payload,
    card_payload: Map<String, Value>,
    summary: String,
    /// The card's week when it has neither a time nor a run.
    week: Option<DateTime<Utc>>,
}

impl<'r> Plan<'r> {
    fn new(kind: ChangeKind, run: Option<&'r Run>, summary: String) -> Self {
        Self {
            kind,
            run,
            at: None,
            bosses: run.map(|run| run.bosses.clone()).unwrap_or_default(),
            participants: Vec::new(),
            rsvp: None,
            payload: Payload::None,
            card_payload: Map::new(),
            summary,
            week: None,
        }
    }
}

fn failed(error: impl ToString) -> CallError {
    CallError::Failed(error.to_string())
}

fn asker<'a>(world: &ToolWorld<'a>, ctx: &'a ToolContext) -> Asker<'a> {
    Asker {
        author_id: &ctx.author_id,
        channel_id: &ctx.channel_id,
        is_admin: ctx.is_admin,
        channels: world.channels,
        pilot: world.pilot,
    }
}

fn arg(args: &Map<String, Value>, name: &str) -> String {
    strip(&py_text(args.get(name))).to_owned()
}

fn current_week(world: &ToolWorld<'_>, now: DateTime<Utc>) -> Result<DateTime<Utc>, CallError> {
    let start =
        week_start(&now, world.zone, world.reset_weekday, world.reset_time).map_err(failed)?;
    utc_instant(&start).map_err(failed)
}

/// A model-supplied day and time that is still ahead.
fn future_when(
    world: &ToolWorld<'_>,
    ctx: &ToolContext,
    raw: &str,
) -> Result<DateTime<Utc>, CallError> {
    let at = when::parse_when(raw, world.zone, ctx.now)
        .map_err(|message| ToolError(format!("{message}. Ask them for the day and time again.")))?;
    if at <= ctx.now {
        return Err(ToolError(format!(
            "`{raw}` is in the past. Ask them which day they mean."
        ))
        .into());
    }
    Ok(at)
}

impl<S, I, C> Proposer<'_, S, I, C>
where
    S: ScheduleStore + ProposalStore + Sync,
    I: IdSource,
    C: Clock,
{
    async fn submit(
        &mut self,
        world: &ToolWorld<'_>,
        ctx: &ToolContext,
        plan: Plan<'_>,
    ) -> Proposed {
        let week = match (plan.at, plan.run) {
            (Some(at), _) => {
                let start = week_start(&at, world.zone, world.reset_weekday, world.reset_time)
                    .map_err(failed)?;
                utc_instant(&start).map_err(failed)?
            }
            (None, Some(run)) => run.week_start,
            (None, None) => plan.week.ok_or_else(|| {
                CallError::from(ToolError::new("Ask them which day and time they mean."))
            })?,
        };
        let change = ProposedChange {
            kind: plan.kind,
            run_id: plan.run.map(|run| run.id.clone()),
            channel_id: Some(ctx.channel_id.clone()),
            bosses: plan.bosses.clone(),
            participants: plan.participants.clone(),
            new_datetime: plan.at,
            rsvp: plan.rsvp,
            payload: plan.payload,
        };
        let proposed = self
            .service
            .propose_chat(
                ProposalRequest {
                    change,
                    source: ProposalSource::Chat,
                    source_id: ctx.source_id.clone(),
                    supersede: Supersede::Keep,
                },
                self.policy,
                world.directory,
            )
            .await
            .map_err(|error| match error {
                ProposalError::Refused(reason) => {
                    ToolError(REFUSED_UP_FRONT.replace("{reason}", &reason.to_string())).into()
                }
                ProposalError::NoEffect => ToolError::new(NO_EFFECT).into(),
                ProposalError::Expired => ToolError::new(EXPIRED).into(),
                other => failed(other),
            })?;
        let (id, existing) = match proposed {
            ChatProposed::Created(proposed) => (proposed.proposal.id, None),
            ChatProposed::Existing(existing) => (existing.proposal_id.clone(), Some(existing)),
        };
        // v4 retired the older live cards about the same run (scoped to this
        // channel unless it is the run's home) or the same new boss set here.
        let retired = self
            .service
            .supersede_proposals(SupersedeScope {
                run_id: plan.run.map(|run| run.id.as_str()),
                channel_id: Some(&ctx.channel_id),
                bosses: &plan.bosses,
                keep: Some(&id),
                from_channel: Some(&ctx.channel_id),
                by: ProposalSource::Chat,
            })
            .await
            .map_err(failed)?;
        if let Some(existing) = existing {
            return Ok(ProposalReply::Existing {
                output: card::already_proposed(&existing, &world.pilot.guild_id),
                superseded: retired,
            });
        }
        Ok(ProposalReply::Created(Box::new(ProposalCard {
            kind_label: kind_label(plan.kind, &plan.card_payload),
            when: card_when(world, plan.kind, plan.at, plan.run, &plan.card_payload),
            party: card_party(world, &plan.participants, plan.run, &plan.card_payload),
            proposal_id: id,
            kind: plan.kind,
            run_id: plan.run.map(|run| run.id.clone()),
            channel_id: ctx.channel_id.clone(),
            bosses: plan.bosses,
            participants: plan.participants,
            new_datetime: plan.at,
            rsvp: plan.rsvp,
            payload: plan.card_payload,
            summary: plan.summary,
            week_start: week,
            evidence_message_ids: vec![ctx.message_id.clone()],
            superseded: retired,
        })))
    }

    /// `propose_move`: one dated run to a new day and time.
    pub async fn propose_move(
        &mut self,
        world: &ToolWorld<'_>,
        ctx: &ToolContext,
        args: &Map<String, Value>,
    ) -> Proposed {
        let run = resolve_run(world, &py_text(args.get("run_query")), ctx.now)?;
        // Where it moves to is not how they described it; an unreadable
        // target is refused below, after the run is settled.
        let to = when::parse_when(&arg(args, "to_when"), world.zone, ctx.now).ok();
        require_heard(world, run, to, ctx.now, |_| true)?;
        require_authority(asker(world, ctx), Subject::Run(run), world.snapshot)?;
        let raw = arg(args, "to_when");
        if raw.is_empty() {
            return Err(ToolError::new("Ask them what day and time it should move to.").into());
        }
        let at = future_when(world, ctx, &raw)?;
        if at == run.datetime {
            return Err(
                ToolError::new("That run is already at that time; nothing to propose.").into(),
            );
        }
        let summary = format!(
            "move {} to {}",
            boss_labels(&run.bosses),
            when_label(&at, world.zone)
        );
        let mut plan = Plan::new(ChangeKind::Move, Some(run), summary);
        plan.at = Some(at);
        self.submit(world, ctx, plan).await
    }

    /// `propose_add`: a new one-off run, or a new weekly when `weekly`.
    pub async fn propose_add(
        &mut self,
        world: &ToolWorld<'_>,
        ctx: &ToolContext,
        args: &Map<String, Value>,
    ) -> Proposed {
        let bosses = validate_bosses(world, &py_text(args.get("boss")))?;
        let raw = arg(args, "when");
        if raw.is_empty() {
            return Err(ToolError::new("Ask them what day and time the run should be.").into());
        }
        let at = future_when(world, ctx, &raw)?;
        let people = validate_participants(world, ctx, args.get("participants"))?;
        let labels = boss_labels(&bosses);
        let mut plan = if is_true(args.get("weekly")) {
            let wall = local(&at, world.zone);
            let hhmm = hhmm(wall.time());
            let mut plan = Plan::new(
                ChangeKind::Fix,
                None,
                format!(
                    "new weekly: {labels} every {} {hhmm}",
                    weekday_name(wall.weekday())
                ),
            );
            plan.payload = Payload::Fix {
                weekday: Some(wall.weekday()),
                time: Some(wall.time()),
            };
            plan.card_payload.insert(
                "weekday".into(),
                json!(wall.weekday().num_days_from_monday()),
            );
            plan.card_payload.insert("time".into(), json!(hhmm));
            plan
        } else {
            Plan::new(
                ChangeKind::Add,
                None,
                format!("new run: {labels} on {}", when_label(&at, world.zone)),
            )
        };
        plan.at = Some(at);
        plan.bosses = bosses;
        plan.participants = people;
        self.submit(world, ctx, plan).await
    }

    /// `propose_cancel`: one dated run off.
    pub async fn propose_cancel(
        &mut self,
        world: &ToolWorld<'_>,
        ctx: &ToolContext,
        args: &Map<String, Value>,
    ) -> Proposed {
        let run = resolve_run(world, &py_text(args.get("run_query")), ctx.now)?;
        require_heard(world, run, None, ctx.now, |_| true)?;
        require_authority(asker(world, ctx), Subject::Run(run), world.snapshot)?;
        if run.status == RunStatus::Cancelled {
            return Err(ToolError::new("That run is already cancelled.").into());
        }
        let summary = format!("cancel {}", boss_labels(&run.bosses));
        self.submit(
            world,
            ctx,
            Plan::new(ChangeKind::Cancel, Some(run), summary),
        )
        .await
    }

    /// `propose_rsvp`: the asker's own answer, never anybody else's.
    pub async fn propose_rsvp(
        &mut self,
        world: &ToolWorld<'_>,
        ctx: &ToolContext,
        args: &Map<String, Value>,
    ) -> Proposed {
        let run = resolve_run(world, &py_text(args.get("run_query")), ctx.now)?;
        // Only runs they are on are theirs to answer for.
        require_heard(world, run, None, ctx.now, |candidate| {
            candidate.participants.contains(&ctx.author_id)
        })?;
        require_authority(asker(world, ctx), Subject::Run(run), world.snapshot)?;
        let answer = arg(args, "answer").to_lowercase();
        let state = match answer.as_str() {
            "yes" => RsvpState::Yes,
            "no" => RsvpState::No,
            _ => {
                return Err(ToolError::new(
                    "answer must be 'yes' or 'no'. Ask them whether they can make it.",
                )
                .into());
            }
        };
        if !run.participants.contains(&ctx.author_id) {
            return Err(ToolError(format!(
                "They are not on run {}, so they have nothing to answer. Only somebody on a run can RSVP for it.",
                short_id(&run.id)
            ))
            .into());
        }
        let summary = format!(
            "{} says {answer}",
            member_name(world.directory, &ctx.author_id)
        );
        let mut plan = Plan::new(ChangeKind::Rsvp, Some(run), summary);
        plan.rsvp = Some(state);
        plan.participants = vec![ctx.author_id.clone()];
        self.submit(world, ctx, plan).await
    }

    /// `propose_remove_fixed`: retire a weekly baseline.
    pub async fn propose_remove_fixed(
        &mut self,
        world: &ToolWorld<'_>,
        ctx: &ToolContext,
        args: &Map<String, Value>,
    ) -> Proposed {
        let fixed = resolve_fixed(world, &py_text(args.get("query")))?;
        require_authority(asker(world, ctx), Subject::Fixed(fixed), world.snapshot)?;
        let was = fixed_when(fixed);
        let mut plan = Plan::new(
            ChangeKind::Fix,
            None,
            format!("stop scheduling {} every {was}", boss_labels(&fixed.bosses)),
        );
        plan.bosses = fixed.bosses.clone();
        plan.participants = fixed.participants.clone();
        plan.week = Some(current_week(world, ctx.now)?);
        plan.payload = Payload::FixRemove {
            fixed_run_id: Some(fixed.id.clone()),
        };
        plan.card_payload.insert("op".into(), json!(FIX_REMOVE));
        plan.card_payload
            .insert("fixed_run_id".into(), json!(fixed.id));
        plan.card_payload.insert("weekly_when".into(), json!(was));
        self.submit(world, ctx, plan).await
    }

    /// `propose_change_fixed`: a weekly's day, time and/or whole party.
    pub async fn propose_change_fixed(
        &mut self,
        world: &ToolWorld<'_>,
        ctx: &ToolContext,
        args: &Map<String, Value>,
    ) -> Proposed {
        let fixed = resolve_fixed(world, &py_text(args.get("query")))?;
        require_authority(asker(world, ctx), Subject::Fixed(fixed), world.snapshot)?;
        let (raw_day, raw_time) = (arg(args, "day"), arg(args, "time"));
        let weekday = if raw_day.is_empty() {
            fixed.weekday
        } else {
            parse_weekday(&raw_day).map_err(|error| {
                ToolError(format!(
                    "{error}. Ask them which day of the week it should be."
                ))
            })?
        };
        let time: NaiveTime = if raw_time.is_empty() {
            fixed.time
        } else {
            parse_hhmm(&raw_time).map_err(|error| {
                ToolError(format!("{error}. Ask them what time it should start."))
            })?
        };
        let clock = hhmm(time);
        let party = new_party(world, ctx, args.get("participants"))?;
        let people = fixed.participants.clone();
        let moves = (weekday, &clock) != (fixed.weekday, &hhmm(fixed.time));
        let reparties = party.as_ref().is_some_and(|party| *party != people);
        let labels = boss_labels(&fixed.bosses);
        let was = fixed_when(fixed);
        if !moves && !reparties {
            return Err(ToolError(format!(
                "Nothing about the weekly {labels} ({was}) would change. Ask them what should change about it -- the day, the time, or who is on it."
            ))
            .into());
        }
        let mut card_payload = Map::new();
        card_payload.insert("op".into(), json!(FIX_EDIT));
        card_payload.insert("fixed_run_id".into(), json!(fixed.id));
        card_payload.insert("weekly_when".into(), json!(was));
        let mut changes = Vec::new();
        if moves {
            card_payload.insert("weekday".into(), json!(weekday.num_days_from_monday()));
            card_payload.insert("time".into(), json!(clock));
            changes.push(format!("{was} → {} {clock}", weekday_name(weekday)));
        }
        let joining = if reparties {
            party.unwrap_or_default()
        } else {
            Vec::new()
        };
        if reparties {
            card_payload.insert("participants".into(), json!(joining));
            changes.push(format!(
                "party {} → {}",
                names(world, &people),
                names(world, &joining)
            ));
        }
        let mut plan = Plan::new(
            ChangeKind::Fix,
            None,
            format!("change the weekly {labels}: {}", changes.join("; ")),
        );
        plan.bosses = fixed.bosses.clone();
        // The party it has now: who may press ✅ is who the timing affects.
        plan.participants = people;
        plan.week = Some(current_week(world, ctx.now)?);
        plan.payload = Payload::FixEdit {
            fixed_run_id: Some(fixed.id.clone()),
            weekday: moves.then_some(weekday),
            time: moves.then_some(time),
            participants: joining,
        };
        plan.card_payload = card_payload;
        self.submit(world, ctx, plan).await
    }
}
