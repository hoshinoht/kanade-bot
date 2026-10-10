//! `/debug ping`'s options: a stored run or an in-memory sample run, the
//! channel, a one-off message style and a fresh header rewrite.

use chrono::TimeDelta;
use twilight_model::application::command::CommandOption;

use super::super::build::{choices, integer, picked, text, text_channel};
use super::super::context::{CommandContext, PingRequest, SampleRun, TestKind, TestSubject};
use super::super::dispatch::CommandError;
use super::super::invocation::Invocation;
use super::super::lookup::everything;
use super::super::options::Args;
use super::super::say::mentioned_users;
use crate::bot::ids::id_text;
use crate::domain::ids::resolve_id;
use crate::domain::schedule::RunStatus;
use crate::domain::settings::MessageStyle;

/// A sample run's default start, in minutes from now.
const SAMPLE_MINUTES: i64 = 120;
const SAMPLE_OPTIONS: [&str; 6] = ["bosses", "time", "party", "in", "out", "status"];
const STATUSES: [RunStatus; 5] = [
    RunStatus::Planned,
    RunStatus::Confirmed,
    RunStatus::AtRisk,
    RunStatus::Otot,
    RunStatus::Done,
];

fn user(message: impl Into<String>) -> CommandError {
    CommandError::User(message.into())
}

/// The registered options; required ones first, as Discord requires.
pub fn options(kinds: &[(&str, &str)]) -> Vec<CommandOption> {
    let styles: Vec<(&str, &str)> = [MessageStyle::Classic, MessageStyle::Redesigned]
        .iter()
        .map(|style| (style.as_str(), style.as_str()))
        .collect();
    let statuses: Vec<(&str, &str)> = STATUSES
        .iter()
        .map(|status| (status.as_str(), status.as_str()))
        .collect();
    vec![
        choices("kind", "Which message to post", true, kinds),
        picked(
            "run_id",
            "A run (dropdown or id like `a1b2c3d4`); omit for a sample run",
            false,
        ),
        text_channel(
            "channel",
            "Post here instead (elsewhere than home: display only)",
        ),
        choices("style", "Message style for this post only", false, &styles),
        choices(
            "header",
            "default: the seed; rewrite: a fresh persona rewrite (never stored)",
            false,
            &[("default", "default"), ("rewrite", "rewrite")],
        ),
        text(
            "bosses",
            "Sample run: bosses, e.g. `hard malefic star, hfa`",
            false,
        ),
        integer(
            "time",
            "Sample run: minutes from now (default 120)",
            -1440,
            10080,
        ),
        text(
            "party",
            "Sample run: members as mentions (default: you)",
            false,
        ),
        text("in", "Sample run: who answered ✅ (mentions)", false),
        text("out", "Sample run: who answered ❌ (mentions)", false),
        choices(
            "status",
            "Sample run: status (default planned)",
            false,
            &statuses,
        ),
    ]
}

/// The request the options describe.
pub async fn request(
    ctx: &CommandContext,
    invocation: &Invocation,
) -> Result<PingRequest, CommandError> {
    let args = Args(&invocation.options);
    let wanted = args.text("kind").unwrap_or_default();
    let kind = TestKind::parse(wanted)
        .ok_or_else(|| user(format!("Don't know how to render `{wanted}`.")))?;
    let sample_given = SAMPLE_OPTIONS
        .iter()
        .any(|name| invocation.options.iter().any(|option| option.name == *name));
    let subject = match (args.text("run_id"), args.text("bosses")) {
        (Some(_), _) if sample_given => {
            return Err(user(
                "Give either `run_id` or sample run options, not both.",
            ));
        }
        (Some(raw), _) => {
            let snapshot = everything(ctx).await?;
            // /debug reaches any run.
            let Ok(run_id) = resolve_id(raw, snapshot.runs.iter().map(|run| run.id.as_str()))
            else {
                return Err(user(format!("No run matches `{raw}`.")));
            };
            TestSubject::Run(run_id.to_owned())
        }
        (None, Some(bosses)) => TestSubject::Sample(sample(ctx, invocation, args, bosses)?),
        (None, None) if sample_given => return Err(user("A sample run needs `bosses`.")),
        (None, None) if kind == TestKind::Digest => TestSubject::Week,
        (None, None) => return Err(user("Give a `run_id`, or `bosses` for a sample run.")),
    };
    Ok(PingRequest {
        subject,
        kind,
        requested_by: id_text(invocation.invoker.user_id),
        channel: args.channel("channel"),
        invoked_in: invocation.channel_id.map(id_text),
        style: args.text("style").and_then(MessageStyle::parse),
        rewrite: args.text("header") == Some("rewrite"),
    })
}

fn members(args: Args<'_>, name: &str) -> Result<Vec<String>, CommandError> {
    match args.text(name) {
        None => Ok(Vec::new()),
        Some(text) => {
            let users = mentioned_users(text);
            if users.is_empty() {
                Err(user(format!("`{name}` needs member mentions like <@123>.")))
            } else {
                Ok(users)
            }
        }
    }
}

fn sample(
    ctx: &CommandContext,
    invocation: &Invocation,
    args: Args<'_>,
    bosses: &str,
) -> Result<SampleRun, CommandError> {
    let bosses = ctx
        .catalog
        .parse(bosses)
        .map_err(|error| user(error.to_string()))?;
    let minutes = args.integer("time").unwrap_or(SAMPLE_MINUTES);
    let mut party = members(args, "party")?;
    if party.is_empty() {
        party.push(id_text(invocation.invoker.user_id));
    }
    let status = args
        .text("status")
        .and_then(|text| RunStatus::parse(text).ok())
        .unwrap_or(RunStatus::Planned);
    Ok(SampleRun {
        bosses,
        at: ctx.now() + TimeDelta::minutes(minutes),
        party,
        yes: members(args, "in")?,
        no: members(args, "out")?,
        status,
    })
}
