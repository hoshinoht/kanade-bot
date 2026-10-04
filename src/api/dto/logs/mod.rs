//! Chat and extraction log rows (`chat.json`, `extractions.json`). A
//! withheld chat question is never shown: the admin sees the placeholder the
//! model sees, and the model output and tool arguments and results of that
//! turn are withheld with it, as they may quote the question.

mod chat;
mod extraction;

use std::collections::BTreeMap;

use serde::Serialize;

pub use chat::{
    Chat, ChatCard, ChatRoundFacts, ChatRow, ChatSummary, ChatToolCall, ChatTurn, MaskedRoundView,
    ModelView, RoundGuardrail, TokenName, asked, chat_row, chat_summary, chat_turn,
    created_proposals,
};
pub use extraction::{
    Amendment, CALL_FAILED, CallContext, Extraction, ExtractionBase, ExtractionRefusal,
    ExtractionRow, ExtractionSummary, Extractions, Proposed, ReadMessage, call_error, extraction,
    extraction_row, extraction_summary,
};

use super::Named;
use crate::{
    api::state::ChannelEntry,
    domain::{
        members::{Roster, member_name},
        model_log::LogFacets as Facets,
    },
};

pub struct Names<'a> {
    pub roster: &'a Roster,
    pub channels: &'a BTreeMap<String, ChannelEntry>,
}

impl Names<'_> {
    fn channel(&self, id: Option<&str>) -> Option<String> {
        id.and_then(|id| self.channels.get(id))
            .map(|channel| channel.name.clone())
    }

    fn channel_name(&self, id: &str) -> String {
        self.channels
            .get(id)
            .map_or_else(|| id.to_owned(), |channel| channel.name.clone())
    }

    fn member(&self, id: Option<&str>) -> Named {
        match id {
            Some(id) => Named {
                id: id.to_owned(),
                name: member_name(self.roster, id),
            },
            None => Named {
                id: String::new(),
                name: "unknown".to_owned(),
            },
        }
    }

    pub fn facets(&self, facets: &Facets) -> LogFacets {
        LogFacets {
            models: facets.models.clone(),
            tools: facets.tools.clone(),
            outcomes: facets.outcomes.clone(),
            channels: facets
                .channels
                .iter()
                .map(|id| Named {
                    id: id.clone(),
                    name: self.channel_name(id),
                })
                .collect(),
        }
    }
}

/// What the log filters can offer (all values seen, not only the filtered rows').
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct LogFacets {
    pub models: Vec<String>,
    pub tools: Vec<String>,
    pub outcomes: Vec<String>,
    pub channels: Vec<Named>,
}

/// Reported token usage over a set of logged requests: sums over those that
/// reported a pair (null when none did), how many did, and the median of
/// reported prompt tokens / local estimate (two decimals; null when none).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct UsageSummary {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub reported: usize,
    pub est_ratio: Option<f64>,
}

/// Accumulates a [`UsageSummary`]: sums over the requests that reported a
/// pair (`None` when none did, never 0), and the prompt/estimate ratios of
/// those that also have a non-zero estimate.
#[derive(Default)]
struct UsageTally {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    reported: usize,
    ratios: Vec<f64>,
}

impl UsageTally {
    fn add(&mut self, prompt: Option<u64>, completion: Option<u64>, estimate: Option<u64>) {
        let (Some(prompt), Some(completion)) = (prompt, completion) else {
            return;
        };
        let sum = |total: &mut Option<u64>, value: u64| {
            *total = Some(total.unwrap_or_default().saturating_add(value));
        };
        sum(&mut self.prompt_tokens, prompt);
        sum(&mut self.completion_tokens, completion);
        self.reported += 1;
        if let Some(estimate) = estimate.filter(|estimate| *estimate > 0) {
            self.ratios.push(prompt as f64 / estimate as f64);
        }
    }

    /// The median ratio, rounded to two decimals.
    fn est_ratio(&self) -> Option<f64> {
        let mut ratios = self.ratios.clone();
        ratios.sort_by(f64::total_cmp);
        let middle = ratios.len() / 2;
        let median = match ratios.len() {
            0 => return None,
            n if n % 2 == 1 => ratios[middle],
            _ => f64::midpoint(ratios[middle - 1], ratios[middle]),
        };
        Some((median * 100.0).round() / 100.0)
    }

    fn summary(&self) -> UsageSummary {
        UsageSummary {
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            reported: self.reported,
            est_ratio: self.est_ratio(),
        }
    }
}
