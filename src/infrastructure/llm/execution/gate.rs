use tokio::time::Instant;

use super::super::CompletionResponse;
use super::super::governor::{Attempt, CallKind, Outcome, Permit, Random, Refused, SentRequest};

/// Admission for one runner call: every provider request passes the permit's
/// rate ceiling, breaker and (for retries) retry budget, and is capped by the
/// session's request count.
pub(in crate::infrastructure::llm) struct Gate<'a> {
    permit: Option<&'a Permit>,
    kind: CallKind,
    deadline: Option<Instant>,
    /// The session's own counter, bumped at admission so a cancelled call still counts.
    used: &'a mut u32,
    cap: u32,
    retry_next: bool,
    random: &'a dyn Random,
    attempt: Option<Attempt>,
    /// Session id; each request is tagged `{tag}-{n}` for gateway log correlation.
    tag: Option<&'a str>,
    /// The last admitted request as it went out.
    sent: Option<SentRequest>,
    /// Every correlation id handed to the provider, in order; the session's
    /// own list, so a cancelled call still keeps the id it sent.
    sent_ids: Option<&'a mut Vec<String>>,
    /// The last admitted request's token reservation.
    reservation: Option<u32>,
    /// A reply the runner refused: more tokens than its reservation, or cut off.
    refused: Option<CompletionResponse>,
    /// The call budget left when a reservation was refused before sending.
    budget: Option<u32>,
}

/// What [`Gate::take_measured`] hands the session.
pub(in crate::infrastructure::llm) struct Measured {
    pub(in crate::infrastructure::llm) reservation: Option<u32>,
    pub(in crate::infrastructure::llm) refused: Option<CompletionResponse>,
    pub(in crate::infrastructure::llm) budget: Option<u32>,
}

pub(in crate::infrastructure::llm) enum Denied {
    Governor(Refused),
    RequestLimit,
}

impl<'a> Gate<'a> {
    /// Admits while `*used < cap`; `retry_first` makes the first request spend
    /// retry budget (requeue, clean retry).
    pub(in crate::infrastructure::llm) fn governed(
        permit: &'a Permit,
        kind: CallKind,
        deadline: Instant,
        used: &'a mut u32,
        cap: u32,
        retry_first: bool,
        random: &'a dyn Random,
    ) -> Self {
        Self {
            permit: Some(permit),
            kind,
            deadline: Some(deadline),
            used,
            cap,
            retry_next: retry_first,
            random,
            attempt: None,
            tag: None,
            sent: None,
            sent_ids: None,
            reservation: None,
            refused: None,
            budget: None,
        }
    }

    /// Tags each request `{tag}-{n}` and appends every id sent to `sent_ids`.
    pub(in crate::infrastructure::llm) fn tagged(
        mut self,
        tag: &'a str,
        sent_ids: &'a mut Vec<String>,
    ) -> Self {
        self.tag = Some(tag);
        self.sent_ids = Some(sent_ids);
        self
    }

    pub(super) fn note_request_id(&mut self, id: &str) {
        if let Some(ids) = self.sent_ids.as_deref_mut() {
            ids.push(id.to_owned());
        }
    }

    pub(super) fn note_sent(&mut self, sent: SentRequest) {
        self.sent = Some(sent);
    }

    pub(super) fn note_reservation(&mut self, reservation: u32) {
        self.reservation = Some(reservation);
    }

    pub(super) fn note_refused(&mut self, response: CompletionResponse) {
        self.refused = Some(response);
    }

    /// A reservation larger than the call budget left: nothing is sent.
    pub(super) fn note_over_budget(&mut self, reservation: u32, budget: u32) {
        self.reservation = Some(reservation);
        self.budget = Some(budget);
    }

    /// The last request's reservation; if its reply was refused, that reply;
    /// if it was refused before sending, the budget it exceeded.
    pub(in crate::infrastructure::llm) fn take_measured(&mut self) -> Measured {
        Measured {
            reservation: self.reservation.take(),
            refused: self.refused.take(),
            budget: self.budget.take(),
        }
    }

    /// What the last admitted request sent (alias, reasoning effort, `max_tokens`).
    pub(in crate::infrastructure::llm) fn take_sent(&mut self) -> Option<SentRequest> {
        self.sent.take()
    }

    /// Id of the request admitted last (numbered from 1 within the session).
    pub(super) fn request_id(&self) -> Option<String> {
        self.tag.map(|tag| format!("{tag}-{}", *self.used))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn ungoverned(random: &'a dyn Random, used: &'a mut u32, kind: CallKind) -> Self {
        Self {
            permit: None,
            kind,
            deadline: None,
            used,
            cap: u32::MAX,
            retry_next: false,
            random,
            attempt: None,
            tag: None,
            sent: None,
            sent_ids: None,
            reservation: None,
            refused: None,
            budget: None,
        }
    }

    pub(super) fn kind(&self) -> CallKind {
        self.kind
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(super) fn random(&self) -> &dyn Random {
        self.random
    }

    pub(super) fn has_room(&self) -> bool {
        *self.used < self.cap
    }

    /// Asked before backing off so a denied retry costs no sleep.
    pub(super) fn check_retry(&self) -> Result<(), Denied> {
        if !self.has_room() {
            return Err(Denied::RequestLimit);
        }
        match self.permit {
            Some(permit) => permit.check_retry(false).map_err(Denied::Governor),
            None => Ok(()),
        }
    }

    pub(super) async fn admit(&mut self, retry: bool, deadline: Instant) -> Result<(), Denied> {
        if !self.has_room() {
            return Err(Denied::RequestLimit);
        }
        let retry = retry || std::mem::take(&mut self.retry_next);
        if let Some(permit) = self.permit {
            let wait = deadline.saturating_duration_since(Instant::now());
            // Try-only kinds (rewrite, pre-screen) never wait for a rate token
            // and never retry.
            let attempt = match (self.kind.may_wait(), retry) {
                (false, false) => permit.try_begin_request(),
                (false, true) => Err(Refused::MustNotWait),
                (true, true) => permit.begin_retry(wait).await,
                (true, false) => permit.begin_request(wait).await,
            }
            .map_err(Denied::Governor)?;
            self.attempt = Some(attempt);
        }
        *self.used += 1;
        Ok(())
    }

    /// `None` abandons the attempt (breaker-neutral), e.g. when our own deadline cut it off.
    pub(super) fn finish(&mut self, outcome: Option<Outcome>) {
        if let (Some(attempt), Some(outcome)) = (self.attempt.take(), outcome) {
            attempt.finish(outcome);
        }
    }
}
