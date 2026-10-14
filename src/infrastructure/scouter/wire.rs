//! The parts of the scouter's `GET /v1/bosses` answers kanade reads;
//! every other field is ignored. Boss keys are the sidecar's
//! `<difficulty>_<boss>` card keys (`hard_limbo`), mapped to catalog tokens
//! by [`super::keys`] on the way out.

use std::collections::BTreeMap;

use serde::Deserialize;

/// One character: the `ign=` answer, or one of a batch's `members`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Member {
    pub ign: String,
    /// The preset the result was read with (`00010`).
    pub preset: String,
    /// The character's critical rate is under 100% and the scouter's
    /// manual Input flow raised it to exactly 100 for this result: a "what
    /// if crit were 100%" estimate, labelled as such wherever it shows.
    #[serde(default)]
    pub critical_rate_assumed: bool,
    /// The character's own critical rate, with an assumed result.
    #[serde(default)]
    pub critical_rate_actual: Option<f64>,
    /// `manual_input` with an assumed result; absent for the normal page.
    #[serde(default)]
    pub source: Option<String>,
    pub bosses: BTreeMap<String, Card>,
}

impl Member {
    /// A result from the manual Input flow: either marker labels it, so a
    /// "what if" result never shows as the normal benchmark.
    pub fn crit_assumed(&self) -> bool {
        self.critical_rate_assumed || self.source.as_deref() == Some("manual_input")
    }
}

/// One boss card. Unknown values stay `null`; a missing card is never 0%.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Card {
    pub percentage: f64,
    pub rating: String,
    pub party: bool,
    pub effective_percentage: Option<f64>,
    pub estimated_minutes: Option<f64>,
}

/// The `igns=` answer (its `members` repeat the `ign=` answers kanade
/// already holds).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Batch {
    pub combined: BTreeMap<String, Combined>,
}

/// The party's additive estimate for one boss key.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Combined {
    pub combined_percentage: Option<f64>,
    pub estimated_minutes: Option<f64>,
    pub technically_possible: Option<bool>,
    pub complete: bool,
    /// IGN → `missing_boss` / `unknown_benchmark_players`.
    #[serde(default)]
    pub issues: BTreeMap<String, String>,
}

/// `{"error": {"code", "message", "ign"?}}`; the message is never kept.
#[derive(Deserialize)]
pub(super) struct ErrorBody {
    pub error: ErrorField,
}

#[derive(Deserialize)]
pub(super) struct ErrorField {
    pub code: String,
    #[serde(default)]
    pub ign: Option<String>,
}
