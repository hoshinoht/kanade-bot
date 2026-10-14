//! Scheduler internals out of a member-facing reply (v4 `_member_facing`
//! and `_strip_tool_directives`); Discord channel links survive.

use std::sync::LazyLock;

use regex::{Captures, Regex, RegexBuilder};

use super::fence::map_prose;
use super::{is_word, pattern, pattern_i};

static EMPTY_PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"`?<\s*none\s*>`?"));
static SCHEDULE_CALL: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new(r"`?\bget_schedule\s*\([^)]*\)`?")
        .case_insensitive(true)
        .dot_matches_new_line(true)
        .build()
        .expect("pattern")
});
/// v4's `(?P<quote>['"])(?P<quoted>[^'"]+)(?P=quote)` without a backreference.
static SCHEDULE_ARGUMENT: LazyLock<Regex> = LazyLock::new(|| {
    pattern_i(
        r#"`?(?:\b(?P<assigned>participant|scope|week|week_basis|day)\s*=|['"](?P<json>participant|scope|week|week_basis|day)['"]\s*:)\s*(?:'(?P<q1>[^'"]+)'|"(?P<q2>[^'"]+)"|(?P<bare><@(?:[!&])?\d+>|[\w-]+))`?"#,
    )
});
static WEEK_MODE: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"this_boss|next_boss|auto"));
static CALL_SCHEDULE: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\b(?:call|use)\s+get_schedule\b"));
static GET_SCHEDULE: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"\bget_schedule\b"));

static SHORT_FORM: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\s+and pass the short form\b[^.?!]*"));
static SHORT_FORMS_TOOL_ONLY: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"[^.?!]*\bshort forms? are for the tool only\b[^.?!]*[.?!]?"));
static TOOL_ONLY: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"[^.?!]*\bfor the tool only\b[^.?!]*[.?!]?"));
static NEVER_SHOW: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"[^.?!]*\bnever show them to a member\b[^.?!]*[.?!]?"));
static BACK_TO_TOOL: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\s*\bback to the tool\b[^.?!]*[.?!]?"));
static GUARD: LazyLock<Regex> =
    LazyLock::new(|| pattern_i(r"\s*(?:--|—|–)\s*do not (?:choose|pick|guess|offer)[^.?!]*\."));
static PROPOSE_TOOL: LazyLock<Regex> = LazyLock::new(|| pattern(r"\bpropose_\w+\b"));
static WEEKLY_TRUE: LazyLock<Regex> = LazyLock::new(|| pattern_i(r"\bweekly\s*=\s*true\b"));
static RUNS_OF_SPACE: LazyLock<Regex> = LazyLock::new(|| pattern(r"[ \t]{2,}"));
static SPACE_DOT: LazyLock<Regex> = LazyLock::new(|| pattern(r"\s+\."));
static DOUBLE_DOT: LazyLock<Regex> = LazyLock::new(|| pattern(r"\.\s*\."));
static ELLIPSIS: LazyLock<Regex> = LazyLock::new(|| pattern(r"\.{3,}"));

/// Model-only tool instructions that must never reach a member.
fn strip_tool_directives(text: &str) -> String {
    let mut cleaned = SHORT_FORM.replace_all(text, "").into_owned();
    for strip in [
        &SHORT_FORMS_TOOL_ONLY,
        &TOOL_ONLY,
        &NEVER_SHOW,
        &BACK_TO_TOOL,
    ] {
        cleaned = strip.replace_all(&cleaned, "").into_owned();
    }
    cleaned = GUARD.replace_all(&cleaned, ".").into_owned();
    cleaned = PROPOSE_TOOL
        .replace_all(&cleaned, "the schedule change")
        .into_owned();
    cleaned = WEEKLY_TRUE.replace_all(&cleaned, "weekly").into_owned();
    cleaned = RUNS_OF_SPACE.replace_all(&cleaned, " ").into_owned();
    tidy_dots(&cleaned)
}

/// v4's stray-dot cleanup around, never inside, an ellipsis (named v5
/// difference `D-ELLIPSIS`: v4 turned `Mou...` into `Mou..`).
fn tidy_dots(text: &str) -> String {
    let dots = |gap: &str| {
        let gap = SPACE_DOT.replace_all(gap, ".");
        DOUBLE_DOT.replace_all(&gap, ".").into_owned()
    };
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for ellipsis in ELLIPSIS.find_iter(text) {
        out.push_str(&dots(&text[last..ellipsis.start()]));
        out.push_str(ellipsis.as_str());
        last = ellipsis.end();
    }
    out.push_str(&dots(&text[last..]));
    out
}

fn natural_argument(found: &Captures<'_>) -> String {
    let name = found
        .name("assigned")
        .or_else(|| found.name("json"))
        .map_or(String::new(), |m| m.as_str().to_lowercase());
    let value = found
        .name("q1")
        .or_else(|| found.name("q2"))
        .or_else(|| found.name("bare"))
        .map_or("", |m| m.as_str());
    if value.starts_with("<@") {
        return "the named person".to_owned();
    }
    let natural = match (name.as_str(), value.to_lowercase().as_str()) {
        ("participant", "me") => "your own runs",
        ("scope", "channel") => "this channel",
        ("scope", "all") => "all channels",
        ("week", "this") => "this week",
        ("week", "next") => "next week",
        ("week", "this_boss") => "this boss week",
        ("week", "next_boss") => "next boss week",
        ("week", "auto") => "the relevant week",
        ("week_basis", "calendar") => "calendar week",
        ("week_basis", "boss") => "boss week",
        _ => return value.to_owned(),
    };
    natural.to_owned()
}

/// v4 `(?<![\w<#@&])(this_boss|next_boss|auto)(?!\w)` → words.
fn internal_week_modes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for found in WEEK_MODE.find_iter(text) {
        let before = text[..found.start()].chars().next_back();
        let after = text[found.end()..].chars().next();
        let blocked =
            before.is_some_and(|c| is_word(c) || "<#@&".contains(c)) || after.is_some_and(is_word);
        if blocked {
            continue;
        }
        out.push_str(&text[last..found.start()]);
        out.push_str(match found.as_str().to_lowercase().as_str() {
            "this_boss" => "this boss week",
            "next_boss" => "next boss week",
            _ => "the relevant week",
        });
        last = found.end();
    }
    out.push_str(&text[last..]);
    out
}

/// Remove scheduler internals while keeping Discord channel links; fenced
/// code is left untouched.
pub fn member_facing(text: &str) -> String {
    map_prose(text, member_facing_prose)
}

fn member_facing_prose(text: &str) -> String {
    let cleaned = EMPTY_PLACEHOLDER.replace_all(text, "");
    let cleaned = strip_tool_directives(&cleaned);
    let cleaned = SCHEDULE_CALL.replace_all(&cleaned, "the schedule");
    let cleaned = SCHEDULE_ARGUMENT.replace_all(&cleaned, natural_argument);
    let cleaned = internal_week_modes(&cleaned);
    let cleaned = CALL_SCHEDULE.replace_all(&cleaned, "check the schedule");
    GET_SCHEDULE
        .replace_all(&cleaned, "the schedule lookup")
        .into_owned()
}
