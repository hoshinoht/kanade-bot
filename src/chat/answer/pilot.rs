//! Opening a question: the identity session (guarded route) and the
//! governed question session, then the loop.

use super::{AnswerFailure, ChatPorts, Generation, GuildView, Question, run_question};
use crate::chat::tools::propose::Proposer;
use crate::domain::drafts::ProposalStore;
use crate::domain::scheduler::{Clock, IdSource, ScheduleStore};
use crate::infrastructure::llm::LlmProvider;
use crate::infrastructure::llm::governor::{
    Charge, ModelClient, QuestionLimits, Refused, Role, RoleRoute, SessionError, SessionFailure,
};
use crate::infrastructure::llm::identity::{IdentityCodec, Member, open_session, unmasked};

/// The model side of a question.
pub struct AnswerDeps<'a, P> {
    pub client: &'a ModelClient<P>,
    pub codec: &'a dyn IdentityCodec,
    /// Roster for the identity session.
    pub roster: &'a [Member],
    /// `(user id, name)` every name members were known by this process
    /// (renamed, removed aliases, left the roster): stored history and the
    /// bot's earlier replies may carry them, so each is masked and scanned.
    pub former: &'a [(String, String)],
    /// The chat route read when the question was prepared; `None` reads it
    /// now. Identity check, permit and requests all use this one route.
    pub route: Option<&'a RoleRoute>,
}

/// Answer one question: one identity session and one question session
/// (a permit across every round, `tool_rounds` + 1 requests, the timeout
/// bounding the whole question). Never fails.
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
    let mut identity = match open_session(deps.codec, &route, deps.roster) {
        Ok(identity) => identity,
        Err(refused) => return Generation::failed(AnswerFailure::Route(refused.to_string())),
    };
    for (user_id, name) in deps.former {
        identity.former_name(user_id, name);
    }
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
        Ok(session) => session.with_scanner(identity.scanner()),
        Err(error) => return Generation::failed(AnswerFailure::Session(error)),
    };
    let mut generation = run_question(
        question,
        &route.alias,
        &mut session,
        identity.as_mut(),
        guild,
        proposer,
        ports,
    )
    .await;
    generation.external_unmasked = unmasked(&route, deps.codec);
    generation.external = route.external;
    generation
}
