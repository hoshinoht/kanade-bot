//! Dynamic tool loading (v5; user decision 2026-09-25). v4 offered all
//! twelve schemas every round (~2.2k of a 2.5k-token conversation budget);
//! v5 offers fixed, stably ordered bundles chosen by code with no extra
//! model call, plus a tiny `request_tools` escape hatch. The full-set mode
//! keeps v4's schema shape and key order, with named schema differences.
//!
//! Order is fixed for prefix caching: READ, `request_tools`, then STRATEGY,
//! RUN_WRITES and FIXED_WRITES as offered. What is offered never grants
//! authority: the dispatcher refuses unoffered tools and still checks
//! authority on every write.

mod select;

use std::collections::BTreeSet;

pub use select::{CardContext, Intent, Signals, select};

use super::READ_ONLY_TURN;
use super::schemas::{ToolName, ToolName as Tool, surface_text, v4_surface};

/// v4's tool-round cap.
pub const V4_MAX_TOOL_ROUNDS: u32 = 4;
/// v5's default tool rounds per question (admin-adjustable).
pub const DEFAULT_TOOL_ROUNDS: u32 = 8;
/// The configurable range of tool rounds.
pub const TOOL_ROUNDS: std::ops::RangeInclusive<u32> = 1..=12;

/// A fixed group of tools offered together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Bundle {
    /// Always offered.
    Read,
    Strategy,
    RunWrites,
    FixedWrites,
}

impl Bundle {
    pub const OPTIONAL: [Self; 3] = [Self::Strategy, Self::RunWrites, Self::FixedWrites];

    pub fn tools(self) -> &'static [ToolName] {
        match self {
            Self::Read => &[
                Tool::GetSchedule,
                Tool::GetRun,
                Tool::ListBosses,
                Tool::GetPending,
                Tool::ListFixed,
            ],
            Self::Strategy => &[Tool::GetBossStrategy],
            Self::RunWrites => &[
                Tool::ProposeMove,
                Tool::ProposeAdd,
                Tool::ProposeCancel,
                Tool::ProposeRsvp,
            ],
            // `propose_add` doubles as "make a run weekly".
            Self::FixedWrites => &[
                Tool::ProposeChangeFixed,
                Tool::ProposeRemoveFixed,
                Tool::ProposeAdd,
            ],
        }
    }

    /// The `request_tools` argument naming this bundle.
    pub fn request_name(self) -> Option<&'static str> {
        match self {
            Self::Read => None,
            Self::Strategy => Some("strategy"),
            Self::RunWrites => Some("run_changes"),
            Self::FixedWrites => Some("weekly_changes"),
        }
    }

    /// This bundle's sentence in the `request_tools` description.
    fn request_description(self) -> Option<&'static str> {
        match self {
            Self::Read => None,
            Self::Strategy => Some("strategy: boss mechanics."),
            Self::RunWrites => Some("run_changes: move, add, cancel or RSVP one run."),
            Self::FixedWrites => {
                Some("weekly_changes: change or remove a weekly, or make a run weekly.")
            }
        }
    }

    pub fn from_request(name: &str) -> Option<Self> {
        Self::OPTIONAL
            .into_iter()
            .find(|bundle| bundle.request_name() == Some(name))
    }

    fn writes(self) -> bool {
        matches!(self, Self::RunWrites | Self::FixedWrites)
    }
}

/// Which surface a question sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// v4's surface: every tool every round, no `request_tools`.
    FullSet,
    Dynamic,
}

/// What `request_tools` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Requested {
    /// The bundle is offered from the next round; the call spends one round
    /// of the question's request cap (the loop charges it).
    Added(Bundle),
    /// Nothing changed; the model reads the note.
    Refused(String),
}

/// `request_tools` when no tool-offering round is left after this one.
pub const NO_ROUND_LEFT: &str = "There is no step left to use more tools for this message. Answer with the tools you have, or ask them in words.";

/// The `request_tools` enum JSON and description for `bundles`, in order.
fn request_fragments(bundles: impl IntoIterator<Item = Bundle>) -> (String, String) {
    let (names, descriptions): (Vec<_>, Vec<_>) = bundles
        .into_iter()
        .filter_map(|bundle| Some((bundle.request_name()?, bundle.request_description()?)))
        .map(|(name, description)| (format!(r#""{name}""#), description))
        .unzip();
    (format!("[{}]", names.join(",")), descriptions.join(" "))
}

/// Replace the one occurrence of `from`. A canonical schema that no longer
/// contains it exactly once would advertise bundles `request` refuses, so it
/// fails loudly (every narrowed surface is covered by tests).
fn replace_once(text: &str, from: &str, to: &str) -> String {
    assert_eq!(
        text.matches(from).count(),
        1,
        "request_tools schema fragment drifted: {from}"
    );
    text.replacen(from, to, 1)
}

/// The tools offered to one question, across its rounds. A requested bundle
/// is held until [`ToolOffer::begin_round`], so a call in the same reply as
/// `request_tools` is judged by what the model was actually sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolOffer {
    mode: Mode,
    read_only: bool,
    bundles: BTreeSet<Bundle>,
    available: BTreeSet<Bundle>,
    pending: Option<Bundle>,
    requested: bool,
    closed: bool,
}

impl ToolOffer {
    /// v4 parity: all twelve tools, or the six read tools on a read-only turn.
    pub fn full_set(read_only: bool) -> Self {
        Self {
            mode: Mode::FullSet,
            read_only,
            bundles: BTreeSet::new(),
            available: Bundle::OPTIONAL.into_iter().collect(),
            pending: None,
            requested: false,
            closed: false,
        }
    }

    /// Start a model round: a bundle requested last round is offered from now.
    pub fn begin_round(&mut self) {
        if let Some(bundle) = self.pending.take() {
            self.bundles.insert(bundle);
        }
    }

    /// Refuse further `request_tools` (no tool-offering round left).
    pub fn close_requests(&mut self) {
        self.closed = true;
    }

    /// READ plus the chosen bundles; write bundles never on a read-only turn.
    pub fn dynamic(chosen: impl IntoIterator<Item = Bundle>, read_only: bool) -> Self {
        let mut bundles: BTreeSet<Bundle> = chosen
            .into_iter()
            .filter(|bundle| !(read_only && bundle.writes()))
            .collect();
        bundles.insert(Bundle::Read);
        Self {
            mode: Mode::Dynamic,
            read_only,
            bundles,
            available: Bundle::OPTIONAL.into_iter().collect(),
            pending: None,
            requested: false,
            closed: false,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn bundles(&self) -> Vec<Bundle> {
        self.bundles.iter().copied().collect()
    }

    /// Remove a runtime-unavailable optional bundle before the first round.
    /// It also disappears from `request_tools`, so the model cannot spend a
    /// round asking for a capability the live server does not have.
    pub fn disallow(&mut self, bundle: Bundle) {
        if self.mode == Mode::Dynamic {
            self.available.remove(&bundle);
            self.bundles.remove(&bundle);
            if self.pending == Some(bundle) {
                self.pending = None;
            }
        }
    }

    /// Whether `request_tools` was used this question.
    pub fn requested(&self) -> bool {
        self.requested
    }

    /// The offered tools in their fixed order, without duplicates.
    pub fn tools(&self) -> Vec<ToolName> {
        if self.mode == Mode::FullSet {
            return v4_surface(self.read_only);
        }
        let mut out = Bundle::Read.tools().to_vec();
        out.push(Tool::RequestTools);
        for bundle in Bundle::OPTIONAL {
            if self.bundles.contains(&bundle) {
                for tool in bundle.tools() {
                    if !out.contains(tool) {
                        out.push(*tool);
                    }
                }
            }
        }
        out
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.tools().into_iter().map(ToolName::as_str).collect()
    }

    pub fn offers(&self, tool: ToolName) -> bool {
        self.tools().contains(&tool)
    }

    /// The `request_tools` schema text narrowed to the available bundles.
    /// Edited as text, not re-serialised, so the canonical bytes (and key
    /// order) survive; with every bundle available it is the canonical text.
    fn request_schema_text(&self) -> String {
        let canonical = Tool::RequestTools.schema_text();
        if self.available.len() == Bundle::OPTIONAL.len() {
            return canonical.to_owned();
        }
        let (all_enum, all_description) = request_fragments(Bundle::OPTIONAL);
        let (names, description) = request_fragments(self.available.iter().copied());
        let text = replace_once(canonical, &all_enum, &names);
        replace_once(&text, &all_description, &description)
    }

    /// The request schema, with runtime-unavailable bundles removed from the
    /// `request_tools` surface.
    pub fn schema(&self, tool: ToolName) -> serde_json::Value {
        if self.mode == Mode::Dynamic && tool == Tool::RequestTools {
            serde_json::from_str(&self.request_schema_text()).expect("request-tools schema")
        } else {
            tool.schema()
        }
    }

    /// The compact `tools` JSON for the request, in v4 key order.
    pub fn surface_text(&self) -> String {
        if self.mode == Mode::FullSet {
            return surface_text(&self.tools());
        }
        let parts: Vec<String> = self
            .tools()
            .into_iter()
            .map(|tool| {
                if tool == Tool::RequestTools {
                    self.request_schema_text()
                } else {
                    tool.schema_text().to_owned()
                }
            })
            .collect();
        format!("[{}]", parts.join(","))
    }

    /// What the surface costs the prompt (v4 `estimate_tokens`).
    pub fn estimated_tokens(&self) -> usize {
        crate::extract::prompt::estimate_tokens(&self.surface_text())
    }

    /// The first bundle that would offer `tool`, for the steering note.
    fn bundle_for(&self, tool: ToolName) -> Option<Bundle> {
        Bundle::OPTIONAL
            .into_iter()
            .find(|bundle| self.available.contains(bundle) && bundle.tools().contains(&tool))
    }

    /// Handle one `request_tools(bundle)` call: at most once per question.
    pub fn request(&mut self, bundle: Option<&str>) -> Requested {
        if self.mode == Mode::FullSet {
            return Requested::Refused("Every tool is already available.".to_owned());
        }
        if self.requested {
            return Requested::Refused(
                "You already asked for more tools for this message. Answer with the tools you have, or ask them in words."
                    .to_owned(),
            );
        }
        let names: Vec<&str> = self
            .available
            .iter()
            .filter_map(|bundle| bundle.request_name())
            .collect();
        let Some(bundle) = bundle
            .and_then(Bundle::from_request)
            .filter(|bundle| self.available.contains(bundle))
        else {
            return Requested::Refused(format!("bundle must be one of: {}.", names.join(", ")));
        };
        if self.read_only && bundle.writes() {
            return Requested::Refused(READ_ONLY_TURN.to_owned());
        }
        if self.bundles.contains(&bundle) {
            return Requested::Refused(format!(
                "The {} tools are already available; use them.",
                bundle.request_name().unwrap_or_default()
            ));
        }
        if self.closed {
            return Requested::Refused(NO_ROUND_LEFT.to_owned());
        }
        self.pending = Some(bundle);
        self.requested = true;
        Requested::Added(bundle)
    }

    /// The steering note for a call to a tool this question was not offered.
    pub fn not_offered(&self, tool: ToolName) -> String {
        let arriving = self
            .pending
            .is_some_and(|pending| pending.tools().contains(&tool));
        let hint = match self.bundle_for(tool).and_then(Bundle::request_name) {
            _ if arriving => {
                " It becomes available from your next step; call it again then.".to_owned()
            }
            Some(name) if !self.requested && !self.closed => {
                format!(" If they asked for that, call request_tools with bundle '{name}' first.")
            }
            _ => String::new(),
        };
        format!(
            "{} is not available for this message.{hint} The tools you have are: {}.",
            tool.as_str(),
            self.names().join(", ")
        )
    }
}

/// The note `request_tools` returns when it adds a bundle.
pub fn added_note(bundle: Bundle) -> String {
    let names: Vec<&str> = bundle.tools().iter().map(|tool| tool.as_str()).collect();
    format!(
        "Added {}. They are available from your next step.",
        names.join(", ")
    )
}
