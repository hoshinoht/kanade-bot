//! The guarded, non-failing boundary between the model and the tools (v4
//! `tools/dispatching.py`). Tool arguments and results cross this boundary raw; read-only
//! turns refuse writes, unknown and unoffered tools are refused with a note,
//! and nothing a tool does can take the answer down.

use serde_json::{Map, Value};

use super::bundles::{Mode, Requested, ToolOffer, added_note};
use super::propose::Proposer;
use super::read::{self, ToolWorld};
use super::schemas::{ToolName, v4_names};
use super::{
    CallError, FAILED, LOOKUP_FAILED, ProposalCard, READ_ONLY_TURN, REFUSED, ToolContext,
    ToolOutcome, UNKNOWN, UNKNOWN_TOOL,
};
use crate::domain::drafts::ProposalStore;
use crate::domain::scheduler::{Clock, IdSource, ScheduleStore};
use crate::infrastructure::llm::identity::PassthroughSession;

/// One dispatched call.
#[derive(Clone, Debug, PartialEq)]
pub struct Dispatched {
    pub outcome: ToolOutcome,
    /// Raw tool message content for the transcript.
    pub model_content: String,
    /// A bundle `request_tools` added; the loop charges it one round.
    pub requested: Option<super::bundles::Bundle>,
}

/// v4 `_arguments`: an object, a JSON object string, or `{}`.
fn arguments(raw: &Value) -> Map<String, Value> {
    let text = match raw {
        Value::String(text) => text.clone(),
        Value::Object(_) => raw.to_string(),
        _ => return Map::new(),
    };
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

struct Call<'a> {
    name: &'a str,
    arguments: Map<String, Value>,
}

impl Call<'_> {
    fn done(
        self,
        output: String,
        error: Option<&'static str>,
        cards: Vec<ProposalCard>,
    ) -> ToolOutcome {
        ToolOutcome {
            name: self.name.to_owned(),
            output,
            arguments: self.arguments,
            ok: error.is_none(),
            error,
            created: cards.iter().map(|card| card.proposal_id.clone()).collect(),
            cards,
            detail: None,
        }
    }
}

/// Run one tool call and describe it; never fails.
pub async fn run<S, I, C>(
    ctx: &ToolContext,
    world: &ToolWorld<'_>,
    offer: &mut ToolOffer,
    proposer: &mut Proposer<'_, S, I, C>,
    session: &mut PassthroughSession,
    name: &str,
    raw_arguments: &Value,
) -> Dispatched
where
    S: ScheduleStore + ProposalStore + Sync,
    I: IdSource,
    C: Clock,
{
    let (outcome, requested) = call(
        ctx,
        world,
        offer,
        proposer,
        Call {
            name,
            arguments: arguments(raw_arguments),
        },
    )
    .await;
    let model_content = session.tool_result(&outcome.output);
    Dispatched {
        outcome,
        model_content,
        requested,
    }
}

async fn call<S, I, C>(
    ctx: &ToolContext,
    world: &ToolWorld<'_>,
    offer: &mut ToolOffer,
    proposer: &mut Proposer<'_, S, I, C>,
    call: Call<'_>,
) -> (ToolOutcome, Option<super::bundles::Bundle>)
where
    S: ScheduleStore + ProposalStore + Sync,
    I: IdSource,
    C: Clock,
{
    let tool = ToolName::parse(call.name)
        .filter(|tool| !(offer.mode() == Mode::FullSet && *tool == ToolName::RequestTools));
    // Structural, not advisory: the schemas were withheld too, but a model
    // naming a write from memory is refused here.
    if ctx.read_only && tool.is_some_and(ToolName::is_write) {
        return (
            call.done(READ_ONLY_TURN.to_owned(), Some(REFUSED), Vec::new()),
            None,
        );
    }
    let Some(tool) = tool else {
        let known = match offer.mode() {
            Mode::FullSet => v4_names(),
            Mode::Dynamic => offer.names(),
        };
        let note = UNKNOWN_TOOL
            .replace("{name}", call.name)
            .replace("{known}", &known.join(", "));
        return (call.done(note, Some(UNKNOWN), Vec::new()), None);
    };
    if !offer.offers(tool) {
        let note = offer.not_offered(tool);
        return (call.done(note, Some(REFUSED), Vec::new()), None);
    }
    let args = &call.arguments;
    let result = match tool {
        ToolName::GetSchedule => read::get_schedule(world, ctx, args).map_err(CallError::from),
        ToolName::GetRun => read::get_run(world, ctx, args).map_err(CallError::from),
        ToolName::ListBosses => Ok(read::list_bosses(world)),
        ToolName::GetBossStrategy => read::get_boss_strategy(world, args).map_err(CallError::from),
        ToolName::GetPending => Ok(read::get_pending(world)),
        ToolName::ListFixed => Ok(read::list_fixed(world)),
        ToolName::RequestTools => {
            let bundle = args.get("bundle").and_then(Value::as_str);
            return match offer.request(bundle) {
                Requested::Added(bundle) => (
                    call.done(added_note(bundle), None, Vec::new()),
                    Some(bundle),
                ),
                Requested::Refused(note) => (call.done(note, Some(REFUSED), Vec::new()), None),
            };
        }
        write => {
            let proposed = match write {
                ToolName::ProposeMove => proposer.propose_move(world, ctx, args).await,
                ToolName::ProposeAdd => proposer.propose_add(world, ctx, args).await,
                ToolName::ProposeCancel => proposer.propose_cancel(world, ctx, args).await,
                ToolName::ProposeRsvp => proposer.propose_rsvp(world, ctx, args).await,
                ToolName::ProposeRemoveFixed => {
                    proposer.propose_remove_fixed(world, ctx, args).await
                }
                ToolName::ProposeChangeFixed => {
                    proposer.propose_change_fixed(world, ctx, args).await
                }
                read => unreachable!("{read:?} is dispatched above"),
            };
            return match proposed {
                Ok(card) => {
                    let output = super::propose::card_ready(&card);
                    (call.done(output, None, vec![card]), None)
                }
                Err(error) => (failure(call, error), None),
            };
        }
    };
    match result {
        Ok(output) => (call.done(output, None, Vec::new()), None),
        Err(error) => (failure(call, error), None),
    }
}

fn failure(call: Call<'_>, error: CallError) -> ToolOutcome {
    match error {
        CallError::Refused(refusal) => call.done(refusal.0, Some(REFUSED), Vec::new()),
        // The cause is for the log; the model only learns it failed.
        CallError::Failed(cause) => ToolOutcome {
            detail: Some(cause),
            ..call.done(LOOKUP_FAILED.to_owned(), Some(FAILED), Vec::new())
        },
    }
}
