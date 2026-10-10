//! `rewrites.json`: the Rewrites log list, its per-model summary and one
//! attempt. Rows carry the persona seed and the model's own text only.

use std::collections::BTreeSet;

use serde::Serialize;

use super::{UsageSummary, UsageTally};
use crate::{
    api::dto::iso_instant,
    domain::{
        ids::short_id,
        model_log::{RewriteFacets as Facets, RewriteLog},
    },
};

/// Query params of `GET /api/admin/rewrites`, all optional and combinable:
/// `model`, `from`/`to` (guild-local YYYY-MM-DD), `kind`, `stage`,
/// `verdict` (comma-separated, any of), `q`. Unknown values or malformed
/// dates: 422 invalid_filter.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Rewrites {
    /// Per model over the filtered attempts that reached one.
    pub summary: Vec<RewriteSummary>,
    pub rows: Vec<RewriteRow>,
    /// Rows before filtering.
    pub total: u64,
    pub facets: RewriteFacets,
}

/// Every value the filters can offer (all rows, not only the filtered ones).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RewriteFacets {
    pub models: Vec<String>,
    pub kinds: Vec<String>,
    pub stages: Vec<String>,
    pub verdicts: Vec<String>,
}

impl From<&Facets> for RewriteFacets {
    fn from(facets: &Facets) -> Self {
        Self {
            models: facets.models.clone(),
            kinds: facets.kinds.clone(),
            stages: facets.stages.clone(),
            verdicts: facets.verdicts.clone(),
        }
    }
}

/// Per model over the listed attempts: how many, how many were accepted,
/// and their reported usage.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RewriteSummary {
    pub model: String,
    pub count: usize,
    pub accepted: usize,
    #[serde(flatten)]
    pub usage: UsageSummary,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RewriteRow {
    pub id: String,
    pub short_id: String,
    pub at: String,
    #[cfg_attr(test, ts(type = "RewriteKind"))]
    pub kind: &'static str,
    #[cfg_attr(test, ts(type = "RewriteStage"))]
    pub stage: &'static str,
    /// A card key, digest week, `/debug` command or nudge purpose.
    pub context: Option<String>,
    #[cfg_attr(test, ts(type = "RewriteVerdict"))]
    pub verdict: String,
    /// The gate rule that refused the line (`rejected` only).
    pub rule: Option<String>,
    /// The specific failure: `budget_exceeded`, `busy`, `shutdown`, …
    pub code: Option<String>,
    pub latency_ms: Option<u64>,
    /// The alias the request named; null when nothing was sent.
    pub model: Option<String>,
    /// Reasoning effort as sent.
    pub reasoning: Option<String>,
    /// Provider-reported tokens; null = not reported (never 0).
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    /// The runner's token reservation the reported usage was checked against.
    pub reservation: Option<u64>,
    /// The call token budget the reservation exceeded when the runner refused
    /// it before sending; null otherwise.
    pub budget: Option<u64>,
    /// The seed line the model was asked to rewrite.
    pub seed: String,
    /// The line used (accepted rewrite or seed), placeholders unfilled.
    pub line: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Rewrite {
    #[serde(flatten)]
    pub row: RewriteRow,
    /// The model's raw reply, capped at 8 KiB with a visible marker.
    pub reply: Option<String>,
    /// Response-only reasoning text, capped at 64 KiB.
    pub reasoning_content: Option<String>,
    /// `max_tokens` as sent; null when nothing was sent or the route has no
    /// sampling controls (the body omits it). Older rows hold the reserve.
    pub max_output_tokens: Option<u64>,
    /// The local prompt estimate: the reservation less the `max_tokens` sent;
    /// null when no `max_tokens` went out.
    pub prompt_estimate: Option<u64>,
    pub request_id: Option<String>,
    /// The messages the call was given under `[system]`/`[user]` labels,
    /// capped at 16 KiB; null when no call was attempted or on older rows.
    pub prompt: Option<String>,
}

fn prompt_estimate(log: &RewriteLog) -> Option<u64> {
    log.reservation?.checked_sub(log.max_output_tokens?)
}

pub fn rewrite_row(log: &RewriteLog) -> RewriteRow {
    RewriteRow {
        id: log.id.clone(),
        short_id: short_id(&log.id),
        at: iso_instant(log.at),
        kind: log.kind.as_str(),
        stage: log.stage.as_str(),
        context: log.context.clone(),
        verdict: log.verdict.clone(),
        rule: log.rule.clone(),
        code: log.code.clone(),
        latency_ms: log.latency_ms,
        model: log.model.clone(),
        reasoning: log.reasoning.clone(),
        prompt_tokens: log.prompt_tokens,
        completion_tokens: log.completion_tokens,
        reasoning_tokens: log.reasoning_tokens,
        reservation: log.reservation,
        budget: log.budget,
        seed: log.seed.clone(),
        line: log.line.clone(),
    }
}

pub fn rewrite(log: &RewriteLog) -> Rewrite {
    Rewrite {
        row: rewrite_row(log),
        reply: log.reply.clone(),
        reasoning_content: log.reasoning_content.clone(),
        max_output_tokens: log.max_output_tokens,
        prompt_estimate: prompt_estimate(log),
        request_id: log.request_id.clone(),
        prompt: log.prompt.clone(),
    }
}

/// Per model over the listed attempts that named one.
pub fn rewrite_summary(rows: &[RewriteLog]) -> Vec<RewriteSummary> {
    let models: BTreeSet<&str> = rows.iter().filter_map(|log| log.model.as_deref()).collect();
    models
        .into_iter()
        .map(|model| {
            let mine: Vec<&RewriteLog> = rows
                .iter()
                .filter(|log| log.model.as_deref() == Some(model))
                .collect();
            let mut usage = UsageTally::default();
            for log in &mine {
                usage.add(
                    log.prompt_tokens,
                    log.completion_tokens,
                    prompt_estimate(log),
                );
            }
            RewriteSummary {
                model: model.to_owned(),
                count: mine.len(),
                accepted: mine.iter().filter(|log| log.verdict == "accepted").count(),
                usage: usage.summary(),
            }
        })
        .collect()
}
