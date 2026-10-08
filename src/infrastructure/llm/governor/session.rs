//! Governed model sessions: the only way callers obtain completions. A session
//! holds one permit for its whole interaction and caps every provider request
//! it sends, retries and requeues included.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tokio::time::Instant;

use super::super::{
    ChatRequest, CompletionResponse, Effort, ErrorCode, LlmError, LlmProvider,
    execution::{Cause, CompletionRunner, Denied, ExecutionLimits, Gate, RetryPolicy, RunError},
};
use super::{CallKind, Governor, Permit, Priority, Refused, Role, RoleRoute, Ticket, full_jitter};

/// User decision: default tool-round cap, admin-adjustable 1..=12.
pub const DEFAULT_TOOL_ROUNDS: u8 = 8;
pub const MAX_TOOL_ROUNDS: u8 = 12;
const MAX_TIMEOUT: Duration = Duration::from_secs(300);
/// Kanata's admission `queue_ms`; assumed when a refusal names no `Retry-After`.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(1);
/// Floor for `Retry-After: 0` so a requeue never resends at once.
const MIN_RETRY_AFTER: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuestionLimits {
    /// Model rounds a question may use; one more request is reserved for the clean retry.
    pub tool_rounds: u8,
    /// Wall-clock bound for the whole question, queueing included.
    pub timeout: Duration,
}

impl QuestionLimits {
    pub fn new(timeout: Duration) -> Self {
        Self {
            tool_rounds: DEFAULT_TOOL_ROUNDS,
            timeout,
        }
    }

    fn validate(&self) -> Result<(), SessionError> {
        if (1..=MAX_TOOL_ROUNDS).contains(&self.tool_rounds) && valid_timeout(self.timeout) {
            Ok(())
        } else {
            Err(invalid("invalid-question-limits"))
        }
    }
}

/// What one provider request actually sent, after shaping: the source of
/// truth for logs even when aliases or efforts change between questions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentRequest {
    pub alias: String,
    /// `None`: no `reasoning_effort` went out.
    pub effort: Option<Effort>,
}

/// Whether a failure counts against the member's chat allowance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charge {
    /// The backend produced a reply or may still be working (timeouts).
    Charged,
    /// Turned away, shed or failed before any model work.
    Refunded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionFailure {
    /// The governor turned the call away; nothing was sent.
    Refused(Refused),
    /// The session's request cap is spent (a question keeps one for its clean retry).
    RequestsExhausted,
    /// Ended by a chat timeout, a lost permit, or an extraction session's single call.
    Ended,
    /// Not a question session, or its clean retry is already used.
    CleanRetryUnavailable,
    /// Not an extraction session, no answer to retry yet, or the retry is used.
    AnswerRetryUnavailable,
    Model(LlmError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionError {
    pub failure: SessionFailure,
    pub charge: Charge,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.failure {
            SessionFailure::Refused(refused) => refused.fmt(f),
            SessionFailure::RequestsExhausted => f.write_str("model request cap reached"),
            SessionFailure::Ended => f.write_str("model session has ended"),
            SessionFailure::CleanRetryUnavailable => f.write_str("clean retry unavailable"),
            SessionFailure::AnswerRetryUnavailable => f.write_str("answer retry unavailable"),
            SessionFailure::Model(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for SessionError {}

impl SessionError {
    /// Fixing it needs an operator or a code change (role unset or ungrouped,
    /// key or capability mismatch, invalid request); anything else
    /// is "unavailable now" and may succeed later.
    pub fn is_misconfiguration(&self) -> bool {
        match &self.failure {
            SessionFailure::Refused(refused) => refused.is_misconfiguration(),
            SessionFailure::Model(error) => matches!(
                error.code,
                ErrorCode::RequestInvalid
                    | ErrorCode::UnsupportedCapability
                    | ErrorCode::ProviderAuthentication
                    | ErrorCode::KeyExpired
                    | ErrorCode::ModelMismatch
            ),
            _ => false,
        }
    }

    /// A stable snake-case code for logs and transcripts.
    pub fn code(&self) -> &'static str {
        match &self.failure {
            SessionFailure::Refused(refused) => match refused {
                Refused::UnknownRole => "unknown_role",
                Refused::Ungrouped => "ungrouped",
                Refused::ExternalForbidden => "external_forbidden",
                Refused::MustNotWait => "must_not_wait",
                Refused::Busy => "busy",
                Refused::Timeout => "queue_timeout",
                Refused::Unavailable { .. } => "backend_unavailable",
                Refused::RateLimited { .. } => "rate_ceiling",
                Refused::RetryBudgetExhausted => "retry_budget_exhausted",
            },
            SessionFailure::RequestsExhausted => "requests_exhausted",
            SessionFailure::Ended => "session_ended",
            SessionFailure::CleanRetryUnavailable => "clean_retry_unavailable",
            SessionFailure::AnswerRetryUnavailable => "answer_retry_unavailable",
            SessionFailure::Model(model) => model.code.as_str(),
        }
    }
}

fn refunded(failure: SessionFailure) -> SessionError {
    SessionError {
        failure,
        charge: Charge::Refunded,
    }
}

fn invalid(reason: &str) -> SessionError {
    refunded(SessionFailure::Model(LlmError::new(
        ErrorCode::RequestInvalid,
        reason,
    )))
}

fn valid_timeout(timeout: Duration) -> bool {
    !timeout.is_zero() && timeout <= MAX_TIMEOUT
}

/// Opens governed sessions over one provider.
pub struct ModelClient<P> {
    governor: Arc<Governor>,
    runner: CompletionRunner<P>,
    max_attempts: u8,
    /// Random per client so ids from different processes rarely collide in gateway logs.
    instance: u32,
    sessions: AtomicU64,
}

impl<P> fmt::Debug for ModelClient<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelClient")
            .field("governor", &self.governor)
            .finish_non_exhaustive()
    }
}

impl<P: LlmProvider> ModelClient<P> {
    /// Backoff jitter uses the governor's `Random`.
    pub fn new(
        governor: Arc<Governor>,
        provider: Arc<P>,
        limits: ExecutionLimits,
        retry: RetryPolicy,
    ) -> Result<Self, LlmError> {
        let max_attempts = retry.max_attempts;
        let runner = CompletionRunner::new(provider, limits, retry, governor.random.clone())?;
        let instance = (governor.random.next_u64() >> 32) as u32;
        Ok(Self {
            governor,
            runner,
            max_attempts,
            instance,
            sessions: AtomicU64::new(0),
        })
    }

    /// `kanade-{kind}-{instance}-{sequence}`: always a valid `x-request-id` stem.
    fn next_id(&self, kind: CallKind) -> String {
        let sequence = self.sessions.fetch_add(1, Ordering::Relaxed) + 1;
        format!(
            "kanade-{}-{:08x}-{sequence:x}",
            kind.as_str().replace('_', "-"),
            self.instance
        )
    }

    pub fn governor(&self) -> &Arc<Governor> {
        &self.governor
    }

    /// Queues for a chat permit (admin or new-question class) within the
    /// question's timeout; the permit is held for every round.
    pub async fn open_question(
        &self,
        who: impl Into<String>,
        admin: bool,
        limits: QuestionLimits,
    ) -> Result<Session<'_, P>, SessionError> {
        self.open_question_with(None, who, admin, limits).await
    }

    /// As [`Self::open_question`] on a route read earlier, so a live switch
    /// in between cannot move the question to another model.
    pub async fn open_question_on(
        &self,
        route: &RoleRoute,
        who: impl Into<String>,
        admin: bool,
        limits: QuestionLimits,
    ) -> Result<Session<'_, P>, SessionError> {
        if route.role != Role::Chat {
            return Err(invalid("session-role"));
        }
        self.open_question_with(Some(route), who, admin, limits)
            .await
    }

    async fn open_question_with(
        &self,
        route: Option<&RoleRoute>,
        who: impl Into<String>,
        admin: bool,
        limits: QuestionLimits,
    ) -> Result<Session<'_, P>, SessionError> {
        limits.validate()?;
        let deadline = Instant::now() + limits.timeout;
        let (first, requeue) = if admin {
            (Priority::Admin, Priority::Admin)
        } else {
            (Priority::ChatNew, Priority::ChatRound)
        };
        let ticket = Ticket {
            priority: first,
            kind: CallKind::Chat,
            who: who.into(),
        };
        let permit = match route {
            Some(route) => self
                .governor
                .acquire_route(route, ticket.clone(), limits.timeout)
                .await
                .map_err(|refused| refunded(SessionFailure::Refused(refused)))?,
            None => {
                self.acquire(Role::Chat, ticket.clone(), limits.timeout)
                    .await?
            }
        };
        Ok(Session {
            client: self,
            permit: Some(permit),
            role: Role::Chat,
            ticket: Ticket {
                priority: requeue,
                ..ticket
            },
            deadline,
            max_requests: u32::from(limits.tool_rounds) + 1,
            used: 0,
            requeues_left: 1,
            question: true,
            clean_used: false,
            completed: false,
            answered: false,
            answer_retry_used: false,
            ended: false,
            last_sent: None,
            last_reservation: None,
            refused: None,
            over_budget: None,
            request_ids: Vec::new(),
            id: self.next_id(CallKind::Chat),
        })
    }

    /// One extraction completion at extraction priority, plus at most one
    /// [`Session::answer_retry`] after a reply the caller rejects: queues up to
    /// `wait`, then `timeout` bounds the session. Requests are capped at the
    /// runner's attempts plus one reshape, plus one reserved for the answer retry.
    pub async fn open_extraction(
        &self,
        who: impl Into<String>,
        wait: Duration,
        timeout: Duration,
    ) -> Result<Session<'_, P>, SessionError> {
        self.open_extraction_with(None, who, wait, timeout).await
    }

    /// Opens extraction on the route snapshot read by the caller. A role
    /// switch before admission cannot redirect this session to another alias.
    pub async fn open_extraction_on(
        &self,
        route: &RoleRoute,
        who: impl Into<String>,
        wait: Duration,
        timeout: Duration,
    ) -> Result<Session<'_, P>, SessionError> {
        if route.role != Role::Extraction {
            return Err(invalid("session-role"));
        }
        self.open_extraction_with(Some(route), who, wait, timeout)
            .await
    }

    async fn open_extraction_with(
        &self,
        route: Option<&RoleRoute>,
        who: impl Into<String>,
        wait: Duration,
        timeout: Duration,
    ) -> Result<Session<'_, P>, SessionError> {
        if !valid_timeout(timeout) {
            return Err(invalid("invalid-extraction-timeout"));
        }
        let ticket = Ticket {
            priority: Priority::Extraction,
            kind: CallKind::Extraction,
            who: who.into(),
        };
        let permit = match route {
            Some(route) => self
                .governor
                .acquire_route(route, ticket.clone(), wait)
                .await
                .map_err(|refused| refunded(SessionFailure::Refused(refused)))?,
            None => self.acquire(Role::Extraction, ticket.clone(), wait).await?,
        };
        Ok(Session {
            client: self,
            permit: Some(permit),
            role: Role::Extraction,
            ticket,
            deadline: Instant::now() + timeout,
            max_requests: u32::from(self.max_attempts) + 2,
            used: 0,
            requeues_left: 1,
            question: false,
            clean_used: false,
            completed: false,
            answered: false,
            answer_retry_used: false,
            ended: false,
            last_sent: None,
            last_reservation: None,
            refused: None,
            over_budget: None,
            request_ids: Vec::new(),
            id: self.next_id(CallKind::Extraction),
        })
    }

    /// A one-line persona rewrite (nudges): never waits. The `rewrite` role's
    /// permit comes from `try_acquire` and the rate token from
    /// `try_begin_request`, so a busy group, open breaker or empty bucket is
    /// refused at once with nothing sent. Exactly one `complete` sending exactly
    /// one request: no transport retry, reshape, requeue or answer retry.
    /// `timeout` bounds that request.
    pub fn open_rewrite(
        &self,
        who: impl Into<String>,
        timeout: Duration,
    ) -> Result<Session<'_, P>, SessionError> {
        self.open_rewrite_with(None, who, timeout)
    }

    /// Try-only rewrite on the route snapshot read by the caller.
    pub fn open_rewrite_on(
        &self,
        route: &RoleRoute,
        who: impl Into<String>,
        timeout: Duration,
    ) -> Result<Session<'_, P>, SessionError> {
        if route.role != Role::Rewrite {
            return Err(invalid("session-role"));
        }
        self.open_rewrite_with(Some(route), who, timeout)
    }

    fn open_rewrite_with(
        &self,
        route: Option<&RoleRoute>,
        who: impl Into<String>,
        timeout: Duration,
    ) -> Result<Session<'_, P>, SessionError> {
        if !valid_timeout(timeout) {
            return Err(invalid("invalid-rewrite-timeout"));
        }
        let who = who.into();
        let permit = match route {
            Some(route) => self
                .governor
                .try_acquire_route(route, CallKind::Rewrite, who.clone()),
            None => self
                .governor
                .try_acquire(Role::Rewrite, CallKind::Rewrite, who.clone()),
        }
        .map_err(|refused| refunded(SessionFailure::Refused(refused)))?;
        Ok(Session {
            client: self,
            permit: Some(permit),
            role: Role::Rewrite,
            ticket: Ticket {
                // Never queued: requeues are disabled below.
                priority: Priority::FollowUp,
                kind: CallKind::Rewrite,
                who,
            },
            deadline: Instant::now() + timeout,
            max_requests: 1,
            used: 0,
            requeues_left: 0,
            question: false,
            clean_used: false,
            completed: false,
            answered: false,
            // No answer retry, so no request is held in reserve for one.
            answer_retry_used: true,
            ended: false,
            last_sent: None,
            last_reservation: None,
            refused: None,
            over_budget: None,
            request_ids: Vec::new(),
            id: self.next_id(CallKind::Rewrite),
        })
    }

    async fn acquire(
        &self,
        role: Role,
        ticket: Ticket,
        wait: Duration,
    ) -> Result<Permit, SessionError> {
        self.governor
            .acquire(role, ticket, wait)
            .await
            .map_err(|refused| refunded(SessionFailure::Refused(refused)))
    }
}

/// One governed interaction. Dropping it releases the permit.
pub struct Session<'c, P> {
    client: &'c ModelClient<P>,
    permit: Option<Permit>,
    role: Role,
    /// Ticket used if the session must requeue after losing its gateway slot.
    ticket: Ticket,
    deadline: Instant,
    max_requests: u32,
    used: u32,
    requeues_left: u8,
    question: bool,
    clean_used: bool,
    /// Extraction: the one `complete` has started (set before awaiting, so a
    /// cancelled call cannot be repeated).
    completed: bool,
    /// Extraction: that call returned a reply, so an answer retry may follow.
    answered: bool,
    answer_retry_used: bool,
    ended: bool,
    last_sent: Option<SentRequest>,
    /// The token reservation of the last request admitted.
    last_reservation: Option<u32>,
    /// The last reply the runner refused (diagnostics only).
    refused: Option<CompletionResponse>,
    /// The call budget the last reservation exceeded (nothing was sent).
    over_budget: Option<u32>,
    /// Every `x-request-id` sent, in order (retries and requeues included).
    request_ids: Vec<String>,
    id: String,
}

impl<P> fmt::Debug for Session<'_, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("role", &self.role)
            .field("used", &self.used)
            .field("max_requests", &self.max_requests)
            .field("ended", &self.ended)
            .finish_non_exhaustive()
    }
}

impl<P: LlmProvider> Session<'_, P> {
    /// Provider requests sent so far (retries, reshapes and requeues included).
    pub fn requests_used(&self) -> u32 {
        self.used
    }

    /// What the last request this session sent carried (alias, effort).
    pub fn last_sent(&self) -> Option<&SentRequest> {
        self.last_sent.as_ref()
    }

    /// The token reservation (prompt estimate + `max_tokens`) the last
    /// admitted request was checked against.
    pub fn last_reservation(&self) -> Option<u32> {
        self.last_reservation
    }

    /// The last reply the runner refused, for logs, never for answers: its
    /// reported usage exceeded the reservation (`budget_exceeded`), or it
    /// was cut off (`incomplete`: `finish=length` or an unknown reason).
    pub fn refused_reply(&self) -> Option<&CompletionResponse> {
        self.refused.as_ref()
    }

    /// The call token budget the last reservation exceeded, when the runner
    /// refused it before sending (`budget_exceeded`, `attempt-reservation`).
    pub fn over_budget(&self) -> Option<u32> {
        self.over_budget
    }

    pub fn max_requests(&self) -> u32 {
        self.max_requests
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn requeued(&self) -> bool {
        // Rewrite sessions start with no requeue to spend.
        self.requeues_left == 0 && self.role != Role::Rewrite
    }

    /// No further request may be sent.
    pub fn is_ended(&self) -> bool {
        self.ended
            || (!self.question && self.completed && (!self.answered || self.answer_retry_used))
    }

    /// Correlation id; request `n` of this session is sent as `x-request-id: {id}-{n}`.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The `x-request-id`s actually sent so far, in order. An admitted
    /// request that never reached the provider has none, and a request sent
    /// without known capabilities carries no header.
    pub fn request_ids(&self) -> &[String] {
        &self.request_ids
    }

    /// The alias every request of this session must name.
    pub fn alias(&self) -> Option<&str> {
        self.permit.as_ref().map(Permit::alias)
    }

    /// One model round. A question keeps one request in reserve for
    /// `clean_retry`, an extraction for `answer_retry`; an extraction has one call.
    pub async fn complete(
        &mut self,
        request: &ChatRequest,
    ) -> Result<CompletionResponse, SessionError> {
        if self.question {
            return self.send(request, false).await;
        }
        if self.completed {
            return Err(refunded(SessionFailure::Ended));
        }
        self.completed = true;
        let result = self.send(request, false).await;
        self.answered = result.is_ok();
        result
    }

    /// Once per extraction, after `complete` returned a reply the caller could
    /// not accept: resend (v4's corrected-answer request) with the reserved
    /// request. A content retry, not a failure retry: the group's retry budget
    /// is not spent.
    pub async fn answer_retry(
        &mut self,
        request: &ChatRequest,
    ) -> Result<CompletionResponse, SessionError> {
        if self.question || !self.answered || self.answer_retry_used {
            return Err(refunded(SessionFailure::AnswerRetryUnavailable));
        }
        self.answer_retry_used = true;
        self.send(request, false).await
    }

    /// Once per question: resend with a clean context, using the reserved request
    /// and the group's retry budget. Refused unless the breaker is closed and
    /// budget remains.
    pub async fn clean_retry(
        &mut self,
        request: &ChatRequest,
    ) -> Result<CompletionResponse, SessionError> {
        if !self.question || self.clean_used {
            return Err(refunded(SessionFailure::CleanRetryUnavailable));
        }
        let permit = match (&self.permit, self.ended) {
            (Some(permit), false) => permit,
            _ => return Err(refunded(SessionFailure::Ended)),
        };
        permit
            .check_retry(true)
            .map_err(|refused| refunded(SessionFailure::Refused(refused)))?;
        self.clean_used = true;
        self.send(request, true).await
    }

    async fn send(
        &mut self,
        request: &ChatRequest,
        clean: bool,
    ) -> Result<CompletionResponse, SessionError> {
        let mut retry_first = clean;
        loop {
            let permit = match (&self.permit, self.ended) {
                (Some(permit), false) => permit,
                _ => return Err(refunded(SessionFailure::Ended)),
            };
            if request.model != permit.alias() {
                return Err(invalid("session-alias"));
            }
            let reserve_held = if self.question {
                !self.clean_used
            } else {
                !self.answer_retry_used
            };
            let cap = self.max_requests.saturating_sub(u32::from(reserve_held));
            if self.used >= cap {
                return Err(refunded(SessionFailure::RequestsExhausted));
            }
            let random = self.client.runner.random().clone();
            let mut gate = Gate::governed(
                permit,
                self.ticket.kind,
                self.deadline,
                &mut self.used,
                cap,
                retry_first,
                random.as_ref(),
            )
            .tagged(&self.id, &mut self.request_ids);
            let result = self.client.runner.run(request, &mut gate).await;
            if let Some(sent) = gate.take_sent() {
                self.last_sent = Some(sent);
            }
            let measured = gate.take_measured();
            if measured.reservation.is_some() {
                self.last_reservation = measured.reservation;
                self.refused = measured.refused;
                self.over_budget = measured.budget;
            }
            drop(gate);
            let failure = match result {
                Ok(response) => return Ok(response),
                Err(RunError::Denied(Denied::Governor(refused))) => {
                    return Err(refunded(SessionFailure::Refused(refused)));
                }
                Err(RunError::Denied(Denied::RequestLimit)) => {
                    return Err(refunded(SessionFailure::RequestsExhausted));
                }
                Err(RunError::Failed(failure)) => failure,
            };
            if let Cause::Admission { retry_after } = failure.cause
                && self.requeues_left > 0
                && self.used < cap
                && self.requeue(retry_after).await?
            {
                retry_first = true;
                continue;
            }
            // A timed-out chat request may still run upstream: charge it and stop.
            if failure.cause == Cause::Timeout && self.ticket.kind == CallKind::Chat {
                self.ended = true;
            }
            return Err(SessionError {
                failure: SessionFailure::Model(failure.error),
                charge: if failure.charged {
                    Charge::Charged
                } else {
                    Charge::Refunded
                },
            });
        }
    }

    /// Gives the permit back, waits `Retry-After` plus full jitter, and queues
    /// again (in-flight class). `false` when the deadline leaves no room or the
    /// resend could not be admitted as a retry anyway.
    async fn requeue(&mut self, retry_after: Option<Duration>) -> Result<bool, SessionError> {
        let base = retry_after
            .unwrap_or(DEFAULT_RETRY_AFTER)
            .max(MIN_RETRY_AFTER);
        let pause = base + full_jitter(self.client.governor.random.as_ref(), base);
        let now = Instant::now();
        if now + pause >= self.deadline {
            return Ok(false);
        }
        // Checked while still holding the slot: no pointless wait and requeue.
        let (group, alias) = match &self.permit {
            Some(permit) if permit.check_retry(false).is_ok() => permit.pinned(),
            _ => return Ok(false),
        };
        self.requeues_left -= 1;
        self.permit = None;
        self.ended = true;
        tokio::time::sleep(pause).await;
        let wait = self.deadline.saturating_duration_since(Instant::now());
        // Back to the same model and group even if the role was rerouted.
        let permit = super::permit::acquire(group, alias, self.ticket.clone(), wait)
            .await
            .map_err(|refused| refunded(SessionFailure::Refused(refused)))?;
        self.permit = Some(permit);
        self.ended = false;
        Ok(true)
    }
}
