//! `D-MIXED-PEOPLE`: a question naming other roster members together with
//! the asker ("what do Bramble and I have on wed?",
//! [`ToolContext::schedule_people`]) is never answered from one person's
//! read. Before the reply is final, the asker and every named person must be
//! covered by a successful `get_schedule` read of the last read's period and
//! scope (their own, or one group read); code reads whoever the model left
//! out in that scope, and the loop lets the model answer again over them
//! when a round is left. The reply is then grounded against those
//! per-person listings, asker first, each person's runs under their own
//! heading, not only the latest read. Writes and questions the model
//! answered without a schedule read (a clarifying question, say) are left
//! alone.

use std::iter::once;

use serde_json::{Map, Value};

use super::RoundOutcome;
use crate::chat::tools::read::participants::py_text;
use crate::chat::tools::read::{ToolWorld, get_schedule, schedule_subject};
use crate::chat::tools::{REFUSED, ToolContext, ToolName, ToolOutcome};
use crate::domain::pytext::strip;

fn outcome(arguments: Map<String, Value>, result: Result<String, String>) -> ToolOutcome {
    let (output, error) = match result {
        Ok(output) => (output, None),
        Err(refusal) => (refusal, Some(REFUSED)),
    };
    ToolOutcome {
        name: ToolName::GetSchedule.as_str().to_owned(),
        output,
        arguments,
        ok: error.is_none(),
        error,
        created: Vec::new(),
        cards: Vec::new(),
        detail: None,
    }
}

/// The scope (arguments without a participant) of the model's last
/// successful schedule read, when the rule applies to this question.
fn last_scope(ctx: &ToolContext, outcomes: &[RoundOutcome]) -> Option<Map<String, Value>> {
    if ctx.schedule_people.is_empty() {
        return None;
    }
    let tool = |outcome: &ToolOutcome| ToolName::parse(&outcome.name);
    if outcomes
        .iter()
        .any(|o| tool(&o.outcome).is_some_and(ToolName::is_write))
    {
        return None;
    }
    let last = outcomes
        .iter()
        .rev()
        .map(|o| &o.outcome)
        .find(|outcome| outcome.ok && tool(outcome) == Some(ToolName::GetSchedule))?;
    let mut scope = last.arguments.clone();
    scope.remove("participant");
    Some(scope)
}

/// The asker, then everyone else the question names.
fn people(ctx: &ToolContext) -> impl Iterator<Item = &String> {
    once(&ctx.author_id).chain(&ctx.schedule_people)
}

/// A read's period and scope as `get_schedule` reads them (defaults filled,
/// case and spacing ignored), so a read covers a person only for the same
/// week, day and channel scope.
fn period(arguments: &Map<String, Value>) -> [String; 4] {
    let field = |name: &str, default: &str| {
        let value = strip(&py_text(arguments.get(name))).to_lowercase();
        if value.is_empty() {
            default.to_owned()
        } else {
            value
        }
    };
    [
        field("week", "this"),
        field("week_basis", "calendar"),
        field("day", ""),
        field("scope", "all"),
    ]
}

/// The code reads a mixed question still needs, as `(call id, outcome)`
/// pairs for `round`; empty when every person is covered.
pub(super) fn missing_reads(
    ctx: &ToolContext,
    world: &ToolWorld<'_>,
    outcomes: &[RoundOutcome],
    round: u32,
) -> Vec<(String, ToolOutcome)> {
    let Some(scope) = last_scope(ctx, outcomes) else {
        return Vec::new();
    };
    let wanted = period(&scope);
    let mut covered = Vec::new();
    // Only reads of the last read's period and scope count: an earlier read
    // of another day or week says nothing about this one.
    let reads = outcomes.iter().map(|o| &o.outcome).filter(|outcome| {
        outcome.ok
            && ToolName::parse(&outcome.name) == Some(ToolName::GetSchedule)
            && period(&outcome.arguments) == wanted
    });
    for read in reads {
        match schedule_subject(world, ctx, &read.arguments) {
            // A group read lists everyone's runs.
            Ok(None) => return Vec::new(),
            Ok(Some(id)) => covered.push(id),
            Err(_) => {}
        }
    }
    people(ctx)
        .filter(|id| !covered.contains(id))
        .enumerate()
        .map(|(index, id)| {
            let (arguments, result) = read_of(ctx, world, &scope, id);
            (format!("cover-{round}-{index}"), outcome(arguments, result))
        })
        .collect()
}

/// `id`'s own `get_schedule` in `scope`.
fn read_of(
    ctx: &ToolContext,
    world: &ToolWorld<'_>,
    scope: &Map<String, Value>,
    id: &str,
) -> (Map<String, Value>, Result<String, String>) {
    let mut arguments = scope.clone();
    let who = if id == ctx.author_id {
        "me".to_owned()
    } else {
        format!("<@{id}>")
    };
    arguments.insert("participant".into(), Value::String(who));
    let result = get_schedule(world, ctx, &arguments).map_err(|refusal| refusal.0);
    (arguments, result)
}

/// What a mixed question's reply is grounded against, as the latest
/// listing: each person's own `get_schedule` listing for the last read's
/// period and scope (what their read returned, or would have, after a group
/// read), the asker first and then the named people in question order,
/// each under its own heading; a run two of them share is under each.
/// `None` when the rule does not apply.
pub(super) fn grounding_listing(
    ctx: &ToolContext,
    world: &ToolWorld<'_>,
    outcomes: &[RoundOutcome],
) -> Option<ToolOutcome> {
    let scope = last_scope(ctx, outcomes)?;
    let listings: Vec<String> = people(ctx)
        .filter_map(|id| read_of(ctx, world, &scope, id).1.ok())
        .collect();
    Some(outcome(scope, Ok(listings.join("\n\n"))))
}
