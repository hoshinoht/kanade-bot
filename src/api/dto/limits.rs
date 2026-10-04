//! `limits.json`: model backends behind the Kanata gateway, admission and
//! the chat allowances.

use serde::Serialize;
use serde_json::Value;

use super::{Named, iso_instant};
use crate::infrastructure::llm::governor::GroupSnapshot;

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Permits {
    pub in_use: u32,
    pub total: u32,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct QueuedCall {
    pub position: u32,
    pub kind: &'static str,
    pub who: String,
    pub waiting_s: u64,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RateLevel {
    pub available: u32,
    pub capacity: u32,
    pub refill_per_min: u32,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RetryLevel {
    pub remaining: u32,
    pub capacity: u32,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Breaker {
    #[cfg_attr(test, ts(type = "'closed' | 'half_open' | 'open'"))]
    pub state: &'static str,
    pub failures: u32,
    pub since: String,
    /// Set only while a probe is scheduled (open).
    #[cfg_attr(test, ts(optional = nullable))]
    pub retry_at: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct BackendGroup {
    pub name: String,
    pub backend: String,
    pub models: Vec<String>,
    pub permits: Permits,
    pub queue: Vec<QueuedCall>,
    pub rate: RateLevel,
    pub retry: RetryLevel,
    pub breaker: Breaker,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct AdmissionWindow {
    pub window: &'static str,
    /// No refusal is recorded yet, so the list is always empty.
    #[cfg_attr(test, ts(type = "Refusal[]"))]
    pub refusals: Vec<Value>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Quota {
    pub count: usize,
    pub per_s: f64,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Allowance {
    pub member: Named,
    pub staff: bool,
    /// Null for staff (no limit).
    pub allowance: Option<Quota>,
    pub used: usize,
    #[serde(rename = "override")]
    pub overridden: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Limits {
    pub groups: Vec<BackendGroup>,
    pub admission: AdmissionWindow,
    pub allowances: Vec<Allowance>,
}

impl AdmissionWindow {
    /// The admission window the page reports on.
    pub fn last_hour() -> Self {
        Self {
            window: "last hour",
            refusals: Vec::new(),
        }
    }
}

pub fn group(group: GroupSnapshot) -> BackendGroup {
    BackendGroup {
        name: group.name,
        backend: group.backend,
        models: group.models,
        permits: Permits {
            in_use: group.permits.in_use,
            total: group.permits.total,
        },
        queue: group
            .queue
            .into_iter()
            .map(|entry| QueuedCall {
                position: entry.position,
                kind: entry.kind.as_str(),
                who: entry.who,
                waiting_s: entry.waiting_s,
            })
            .collect(),
        rate: RateLevel {
            available: group.rate.available,
            capacity: group.rate.capacity,
            refill_per_min: group.rate.refill_per_min,
        },
        retry: RetryLevel {
            remaining: group.retry.remaining,
            capacity: group.retry.capacity,
        },
        breaker: Breaker {
            state: group.breaker.state.as_str(),
            failures: group.breaker.failures,
            since: iso_instant(group.breaker.since),
            retry_at: group.breaker.retry_at.map(iso_instant),
        },
    }
}
