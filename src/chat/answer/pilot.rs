//! Open a governed question session on the prepared route, then run the loop.

use super::{AnswerFailure, ChatPorts, Generation, GuildView, Question, run_question};
use crate::chat::tools::propose::Proposer;
use crate::domain::drafts::ProposalStore;
use crate::domain::scheduler::{Clock, IdSource, ScheduleStore};
use crate::infrastructure::llm::LlmProvider;
use crate::infrastructure::llm::governor::{
    Charge, ModelClient, QuestionLimits, Refused, Role, RoleRoute, SessionError, SessionFailure,
};
use crate::infrastructure::llm::identity::PassthroughSession;

/// The model side of a question.
pub struct AnswerDeps<'a, P> {
    pub client: &'a ModelClient<P>,
    /// The chat route read when the question was prepared; `None` reads it now.
    pub route: Option<&'a RoleRoute>,
}

/// Answer one question over a governed session (a permit across every round,
/// `tool_rounds` + 1 requests, the timeout bounding the whole question).
pub async fn answer<P, S, I, C, X>(
    deps: &AnswerDeps<'_, P>,
    question: Question<'_>,
    guild: &GuildView<'_>,
    proposer: &mut Proposer<'_, S, I, C>,
    ports: &X,
) -> Generation
where
    P: LlmProvider,
    S: ScheduleStore + ProposalStore + Sync,
    I: IdSource,
    C: Clock,
    X: ChatPorts,
{
    let route = deps
        .route
        .cloned()
        .or_else(|| deps.client.governor().route(Role::Chat));
    let Some(route) = route else {
        return Generation::failed(AnswerFailure::Session(SessionError {
            failure: SessionFailure::Refused(Refused::UnknownRole),
            charge: Charge::Refunded,
        }));
    };
    // The route's live level, read with its alias (callers' own otherwise).
    let mut question = question;
    question.settings.reasoning = route.effort.or(question.settings.reasoning);
    let limits = QuestionLimits {
        tool_rounds: question.settings.tool_rounds,
        timeout: question.settings.timeout,
    };
    let ctx = question.ctx;
    let mut session = match deps
        .client
        .open_question_on(&route, ctx.author_id.clone(), ctx.is_admin, limits)
        .await
    {
        Ok(session) => session,
        Err(error) => {
            let mut generation = Generation::failed(AnswerFailure::Session(error));
            generation.external_unmasked = route.external && generation.requests > 0;
            generation.external = route.external;
            return generation;
        }
    };
    let mut identity = PassthroughSession;
    let mut generation = run_question(
        question,
        &route.alias,
        &mut session,
        &mut identity,
        guild,
        proposer,
        ports,
    )
    .await;
    generation.external_unmasked = route.external && generation.requests > 0;
    generation.external = route.external;
    generation
}
