//! One Open API call under every rule: the process holds (key refused,
//! maintenance), the persisted 24-hour budget, the limiter's spacing and the
//! error-code backoffs.

use chrono::{DateTime, Utc};
use serde_json::json;

use super::{CharacterData, log};
use crate::nexon::{CallError, NexonClient};
use kanade_domain::chardata::{
    DAILY_CALLS, Failure, MAINTENANCE_AGAIN, MAINTENANCE_FIRST, RATE_LIMIT_TRIES, rate_limit_delay,
};

/// Why a call was not made or did not succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CallFailed {
    NoKey,
    KeyRefused,
    /// Every call waits for maintenance until then.
    Held(DateTime<Utc>),
    /// The 24-hour budget is used up.
    Budget,
    /// A second 429 in a batch: the batch stops for the day.
    BatchAborted,
    /// The character-data file could not be read or written.
    Store,
    Failed(CallError),
}

impl CallFailed {
    /// For refresh state and answers: an error code or a short reason.
    pub(super) fn label(&self) -> String {
        match self {
            Self::NoKey => "no_key".into(),
            Self::KeyRefused => "key_refused".into(),
            Self::Held(_) => "maintenance".into(),
            Self::Budget => "budget".into(),
            Self::BatchAborted => "OPENAPI00007".into(),
            Self::Store => "store".into(),
            Self::Failed(error) => error.label(),
        }
    }

    pub(super) fn failure(&self) -> Option<Failure> {
        match self {
            Self::Failed(error) => Some(error.failure()),
            _ => None,
        }
    }
}

/// 429s seen by one batch.
#[derive(Default)]
pub(super) struct Batch {
    pub(super) rate_limited: u32,
}

impl CharacterData {
    /// The hold every call checks first, at `now`.
    pub(super) fn held(&self, now: DateTime<Utc>) -> Option<CallFailed> {
        let holds = self.holds();
        if holds.key_refused {
            return Some(CallFailed::KeyRefused);
        }
        holds
            .maintenance_until
            .filter(|until| *until > now)
            .map(CallFailed::Held)
    }

    /// Run `op` against the client under the limiter, the budget and the
    /// backoffs; `endpoint` names it in logs.
    pub(super) async fn call<'c, T, F, Fut>(
        &'c self,
        endpoint: &'static str,
        mut batch: Option<&mut Batch>,
        op: F,
    ) -> Result<T, CallFailed>
    where
        F: Fn(&'c NexonClient) -> Fut,
        Fut: Future<Output = Result<T, CallError>>,
    {
        let Some(client) = &self.nexon else {
            return Err(CallFailed::NoKey);
        };
        let mut tries = 0;
        loop {
            tries += 1;
            let now = self.now();
            if let Some(held) = self.held(now) {
                return Err(held);
            }
            match self.store.reserve_call(now, DAILY_CALLS).await {
                Ok(true) => {}
                Ok(false) => {
                    log(
                        "WARN",
                        "nexon_budget_spent",
                        json!({ "endpoint": endpoint }),
                    );
                    return Err(CallFailed::Budget);
                }
                Err(_) => return Err(CallFailed::Store),
            }
            self.limiter.pace().await;
            let error = match op(client).await {
                Ok(value) => {
                    self.holds().maintenance_strikes = 0;
                    return Ok(value);
                }
                Err(error) => error,
            };
            log(
                "WARN",
                "nexon_call_failed",
                json!({ "endpoint": endpoint, "code": error.label(), "try": tries }),
            );
            match error.failure() {
                Failure::RateLimited => {
                    if let Some(batch) = batch.as_deref_mut() {
                        batch.rate_limited += 1;
                        if batch.rate_limited >= 2 {
                            return Err(CallFailed::BatchAborted);
                        }
                    }
                    if tries >= RATE_LIMIT_TRIES {
                        return Err(CallFailed::Failed(error));
                    }
                    let steps = u32::try_from(rate_limit_delay(tries).as_secs()).unwrap_or(60);
                    let unit = self.timing.rate_limit_unit;
                    tokio::time::sleep(unit * steps + jitter(unit / 2)).await;
                }
                Failure::Maintenance => {
                    let mut holds = self.holds();
                    let wait = if holds.maintenance_strikes == 0 {
                        MAINTENANCE_FIRST
                    } else {
                        MAINTENANCE_AGAIN
                    };
                    holds.maintenance_strikes += 1;
                    holds.maintenance_until = Some(self.now() + wait);
                    return Err(CallFailed::Failed(error));
                }
                Failure::KeyRefused => {
                    self.holds().key_refused = true;
                    log(
                        "ERROR",
                        "nexon_key_refused",
                        json!({ "code": error.label() }),
                    );
                    return Err(CallFailed::Failed(error));
                }
                Failure::Preparing | Failure::InvalidIdentifier | Failure::Bug => {
                    return Err(CallFailed::Failed(error));
                }
            }
        }
    }
}

/// A random wait up to `most`.
fn jitter(most: std::time::Duration) -> std::time::Duration {
    let mut bytes = [0u8; 4];
    if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes).is_err() {
        return std::time::Duration::ZERO;
    }
    most.mul_f64(f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX))
}
