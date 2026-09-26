//! The extraction prompt and request (v4 `bot/extract/prompt.py` and the
//! request half of `llm.py`).
//!
//! The prompt carries the boss table, this channel's runs and timings, the
//! roster members the burst involves and the messages with `[msg_id]`
//! prefixes. Every identity-bearing fragment (author labels, mentions, names,
//! message and channel text) goes through the caller's `IdentitySession`, so
//! `Passthrough` renders v4's bytes and a pseudonymizing session hides them.

mod budget;
mod owned;
mod render;
mod system;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

pub use budget::{
    CHARS_PER_TOKEN, CONTEXT_RESERVE, TOKENS_PER_ID, estimate_messages, estimate_tokens,
    prompt_budget, prompt_text, schema_instruction_tokens,
};
pub use owned::{code_owned_schema, code_owned_texts};
pub use render::{member_name, named_bosses, relevant_roster};
pub use system::SYSTEM_PROMPT;

/// The RUNS heading's scope when the channel has runs of its own.
const CHANNEL_SCOPE: &str = "this channel";

use crate::domain::catalog::BossTable;
use crate::domain::schedule::{FixedRun, Run};
use crate::extract::schema::extraction_schema;
use crate::infrastructure::llm::identity::{IdentitySession, Member};
use crate::infrastructure::llm::{
    ChatRequest, Effort, Message, OutputSchema, OutputValidation, Sampling,
};

/// One chat message as the prompt shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptMessage {
    pub id: String,
    pub author_id: String,
    /// The author's Discord name; the session decides what the model sees.
    pub author_name: String,
    pub created_at: DateTime<Utc>,
    pub content: String,
}

/// Everything one prompt is built from; plain data.
#[derive(Clone, Copy, Debug)]
pub struct PromptContext<'a> {
    pub zone: Tz,
    pub table: &'a BossTable,
    /// The new messages, oldest first; the last one is "now".
    pub burst: &'a [PromptMessage],
    /// Earlier messages shown as background only.
    pub context: &'a [PromptMessage],
    /// This channel's runs.
    pub runs: &'a [&'a Run],
    pub fixed_runs: &'a [FixedRun],
    pub roster: &'a [Member],
    pub channel_name: &'a str,
    /// Runs elsewhere in the guild, shown only when the channel has none.
    pub guild_runs: &'a [&'a Run],
}

fn build_user_prompt(context: &PromptContext<'_>, session: &mut dyn IdentitySession) -> String {
    let zone = context.zone;
    // Every author's name is masked from the first line on, so a message
    // naming a later author (not on the roster) never goes out raw.
    for message in context.context.iter().chain(context.burst) {
        session.author_label(&message.author_id, &message.author_name);
    }
    let roster = relevant_roster(context);
    let names: HashMap<&str, &str> = roster
        .iter()
        .map(|member| (member.user_id.as_str(), member_name(member)))
        .collect();
    // A short table is easier for a small model; everything when the burst
    // named nothing recognisable.
    let table_text = render::render_bosses(context.table, &named_bosses(context));

    let (runs, scope) = if context.runs.is_empty() {
        (
            context.guild_runs,
            "the guild (this channel has no runs of its own)",
        )
    } else {
        (context.runs, CHANNEL_SCOPE)
    };

    let mut parts: Vec<String> = Vec::new();
    if !context.channel_name.is_empty() {
        parts.push(format!("CHANNEL: #{}", session.text(context.channel_name)));
    }
    if let Some(last) = context.burst.last() {
        parts.push(format!(
            "NOW: {} ({})",
            render::render_time(last.created_at, zone),
            zone.name()
        ));
    }
    parts.push(format!("BOSSES (use these exact names):\n{table_text}"));

    let roster_lines: Vec<String> = roster
        .iter()
        .map(|member| {
            let mention = session.mention(&member.user_id);
            let label = session.author_label(&member.user_id, member_name(member));
            format!("  {mention} = {label}")
        })
        .collect();
    parts.push(format!(
        "ROSTER:\n{}",
        or_placeholder(roster_lines, "  (nobody on the roster)")
    ));

    let run_lines: Vec<String> = runs
        .iter()
        .map(|run| render::render_run(run, zone, &names, session))
        .collect();
    parts.push(format!(
        "RUNS scheduled in {scope}:\n{}",
        or_placeholder(run_lines, "  (none)")
    ));

    let fixed_lines: Vec<String> = context
        .fixed_runs
        .iter()
        .map(|fixed| render::render_fixed(fixed, &names, session))
        .collect();
    parts.push(format!(
        "FIXED weekly timings for this channel:\n{}",
        or_placeholder(fixed_lines, "  (none)")
    ));

    if !context.context.is_empty() {
        let lines: Vec<String> = context
            .context
            .iter()
            .map(|message| render::render_message(message, zone, session))
            .collect();
        parts.push(format!(
            "EARLIER MESSAGES (background - a NEW message below may be answering one of these, \
             but do not extract an amendment whose only evidence is here):\n{}",
            lines.join("\n")
        ));
    }

    let lines: Vec<String> = context
        .burst
        .iter()
        .map(|message| render::render_message(message, zone, session))
        .collect();
    parts.push(format!(
        "NEW MESSAGES (extract from these):\n{}",
        lines.join("\n")
    ));
    parts.push(
        "Return the amendments these NEW messages support. If there are none, return \
         {\"amendments\": [], \"summary\": \"no schedule change\"}."
            .to_owned(),
    );
    parts.join("\n\n")
}

fn or_placeholder(lines: Vec<String>, placeholder: &str) -> String {
    if lines.is_empty() {
        placeholder.to_owned()
    } else {
        lines.join("\n")
    }
}

/// The system and user messages for one extraction.
pub fn build_messages(
    context: &PromptContext<'_>,
    session: &mut dyn IdentitySession,
) -> Vec<Message> {
    vec![
        Message::System {
            content: SYSTEM_PROMPT.to_owned(),
        },
        Message::User {
            content: build_user_prompt(context, session),
        },
    ]
}

/// The extraction request: the strict schema (closed over the session's issued
/// refs when it pseudonymizes), temperature 0 and seed 0, and room for the
/// answer. The runner shapes it to the model's capabilities, so call this with
/// the messages already encoded.
pub fn extraction_request(
    model: &str,
    messages: Vec<Message>,
    reasoning: Option<Effort>,
    session: &dyn IdentitySession,
) -> ChatRequest {
    let refs = session.participant_enum();
    ChatRequest {
        model: model.to_owned(),
        messages,
        tools: Vec::new(),
        output_schema: Some(OutputSchema {
            name: "extraction".to_owned(),
            schema: extraction_schema(refs.as_deref()),
            strict: true,
            // `parse_response` and the session decode are the validator: they
            // coerce v4's near-misses and answer a malformed reply with a retry.
            validation: OutputValidation::CallerValidates,
        }),
        max_output_tokens: u32::try_from(CONTEXT_RESERVE).expect("small constant"),
        reasoning,
        sampling: Some(Sampling {
            temperature: Some(0.0),
            seed: Some(0),
            top_p: None,
        }),
    }
}
