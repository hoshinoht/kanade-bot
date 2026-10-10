//! Run completion (user decisions 2026-10-09/10): half an hour after a run
//! ends Kanade asks its channel whether it happened (Done / Didn't happen /
//! Not yet); an unanswered run is marked done at its cutoff. `plan.rs` holds
//! the timing rules, `ends.rs` the one "is it over?" rule (a live run past
//! its end is frozen), `prompt.rs` the stored asks and their port. Posting
//! and pressing live in `bot::delivery::run_prompts` and `bot::commands::runs`.

mod ends;
mod plan;
mod prompt;

pub use ends::{RunEnds, RunEndsSource, RunLengthsNow};
pub use plan::{ASK_AGAIN_AFTER, CompletionPlan, PROMPT_DELAY, plan, run_minutes};
pub use prompt::{PromptClose, PromptFuture, PromptOutcome, RunPrompt, RunPromptStore};

/// The request id that marks a press's status change in History:
/// `run-outcome:<run>:<ask>:done|didnt-happen`. A redelivered press replays.
pub fn pressed_request_id(run_id: &str, ask: u32, outcome: PromptOutcome) -> String {
    let word = match outcome {
        PromptOutcome::DidntHappen => "didnt-happen",
        _ => "done",
    };
    format!("run-outcome:{run_id}:{ask}:{word}")
}

/// The request id that marks an automatic done in History:
/// `run-outcome:<run>:auto-done:<unix seconds>`. The instant keeps a run
/// reopened by hand and cut off again from replaying the first record.
pub fn auto_request_id(run_id: &str, at: chrono::DateTime<chrono::Utc>) -> String {
    format!("run-outcome:{run_id}:auto-done:{}", at.timestamp())
}
