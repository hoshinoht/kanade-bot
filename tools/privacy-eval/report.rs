use std::collections::BTreeMap;

use serde::Serialize;

use crate::probe::CaseObservation;

#[derive(Serialize)]
pub struct AggregateReport {
    cases: usize,
    completed: usize,
    failures: usize,
    candidate_identity_strings_scored: usize,
    exact_recoveries_per_case: usize,
    exact_recovery_target: usize,
    false_guesses_per_case: usize,
    ambiguous_terms_per_case: usize,
    abstentions: usize,
    prompt_tokens: u64,
    completion_tokens: u64,
    usage_responses: usize,
    latency_ms_total: u64,
    latency_ms_max: u64,
    residual_quasi_identifier_categories: BTreeMap<&'static str, usize>,
}

impl AggregateReport {
    pub fn new(cases: usize, candidate_identity_strings_scored: usize) -> Self {
        Self {
            cases,
            completed: 0,
            failures: 0,
            candidate_identity_strings_scored,
            exact_recoveries_per_case: 0,
            exact_recovery_target: 0,
            false_guesses_per_case: 0,
            ambiguous_terms_per_case: 0,
            abstentions: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            usage_responses: 0,
            latency_ms_total: 0,
            latency_ms_max: 0,
            residual_quasi_identifier_categories: BTreeMap::new(),
        }
    }

    pub fn record_failure(&mut self) {
        self.failures += 1;
    }

    pub fn record_success(&mut self, observation: CaseObservation) {
        self.completed += 1;
        self.exact_recoveries_per_case += observation.exact_recoveries;
        self.false_guesses_per_case += observation.false_guesses;
        self.ambiguous_terms_per_case += observation.ambiguous_terms;
        self.abstentions += usize::from(observation.abstained);
        self.latency_ms_total = self.latency_ms_total.saturating_add(observation.latency_ms);
        self.latency_ms_max = self.latency_ms_max.max(observation.latency_ms);
        if let Some((prompt, completion)) = observation.usage {
            self.usage_responses += 1;
            self.prompt_tokens += u64::from(prompt);
            self.completion_tokens += u64::from(completion);
        }
        for category in observation.categories {
            *self
                .residual_quasi_identifier_categories
                .entry(category)
                .or_default() += 1;
        }
    }

    pub fn failed_acceptance(&self) -> bool {
        self.failures > 0 || self.completed != self.cases || self.exact_recoveries_per_case != 0
    }
}
