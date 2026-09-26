//! The code-owned words an extraction request carries unmasked, for the
//! boundary scanner's exemptions: the system prompt, the user prompt's
//! headings and placeholders, run and timing lines, weekday names, the
//! retry instruction and the schema. The boss table and zone name come from
//! the caller (loaded at runtime).

use chrono::DateTime;
use chrono_tz::Tz;
use serde_json::Value;

use super::{PromptContext, PromptMessage, SYSTEM_PROMPT, build_user_prompt};
use crate::domain::catalog::BossTable;
use crate::domain::schedule::RunStatus;
use crate::extract::schema::{extraction_schema, retry_instruction};
use crate::infrastructure::llm::identity::PassthroughSession;

/// Code-owned texts (`zone` as the prompt names it).
pub fn code_owned_texts(zone: Tz, table: &BossTable) -> Vec<String> {
    let mut texts = vec![SYSTEM_PROMPT.to_owned(), retry_instruction("")];
    texts.push(skeleton(zone, table));
    texts.push(super::render::LINE_WORDS.join(" "));
    texts.extend(super::render::WEEKDAY_NAMES.map(str::to_owned));
    texts.extend(
        RunStatus::ALL
            .iter()
            .map(|status| status.as_str().to_owned()),
    );
    texts.push(zone.name().to_owned());
    texts
}

/// The extraction schema (keys, descriptions and enums).
pub fn code_owned_schema() -> Value {
    extraction_schema(None)
}

/// The user prompt around one empty message, so every heading and
/// placeholder renders, plus the per-channel scope wording.
fn skeleton(zone: Tz, table: &BossTable) -> String {
    let message = PromptMessage {
        id: String::new(),
        author_id: String::new(),
        author_name: String::new(),
        created_at: DateTime::from_timestamp(0, 0).expect("epoch"),
        content: String::new(),
    };
    let burst = [message];
    let context = PromptContext {
        zone,
        table,
        burst: &burst,
        context: &burst,
        runs: &[],
        fixed_runs: &[],
        roster: &[],
        channel_name: "",
        guild_runs: &[],
    };
    let guild = build_user_prompt(&context, &mut PassthroughSession);
    format!("{guild}\n{}", super::CHANNEL_SCOPE)
}
