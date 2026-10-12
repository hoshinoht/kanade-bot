//! The parts of MapleScouter's `GET /v1/bosses` answers kanade reads;
//! every other field is ignored. Boss keys are the sidecar's
//! `<difficulty>_<boss>` card keys (`hard_limbo`), mapped to catalog tokens
//! by [`super::keys`] on the way out.

use std::collections::BTreeMap;

use serde::Deserialize;

/// One character: the `ign=` answer, or one of a batch's `members`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Member {
    pub ign: String,
    /// The preset the result was read with (`00010` unless MapleScouter
    /// fell back to another for critical rate).
    pub preset: String,
    pub bosses: BTreeMap<String, Card>,
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

/// The `igns=` answer.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Batch {
    pub members: Vec<Member>,
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
