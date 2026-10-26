use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use regex::Regex;

use super::{PromptContext, PromptMessage};
use crate::extract::gate::{BossLexicon, find_bosses};
use crate::extract::text::pattern;
use kanade_domain::catalog::BossTable;
use kanade_domain::ids::short_id;
use kanade_domain::pytext::strip;
use kanade_domain::schedule::{FixedRun, Run, RunStatus};
use kanade_llm::identity::{Member, PassthroughSession};
use kanade_voice::persona::split_lines;

static MENTION: LazyLock<Regex> = LazyLock::new(|| pattern(r"<@!?(\d+)>"));

pub(super) const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn weekday_name(weekday: Weekday) -> &'static str {
    WEEKDAY_NAMES[weekday.num_days_from_monday() as usize]
}

/// v4 `member_name`: nickname, else display name, else the id.
pub fn member_name(member: &Member) -> &str {
    member
        .nickname
        .as_deref()
        .filter(|name| !name.is_empty())
        .or(Some(member.display_name.as_str()).filter(|name| !name.is_empty()))
        .unwrap_or(&member.user_id)
}

/// `2026-08-30 13:07 Sun` in the guild zone.
pub(super) fn render_time(when: DateTime<Utc>, zone: Tz) -> String {
    let local = when.with_timezone(&zone);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02} {}",
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        weekday_name(local.weekday())
    )
}

/// `[123] [2026-08-30 13:07 Sun] [kanon <@114...>] then weds lah`.
pub(super) fn render_message(
    message: &PromptMessage,
    zone: Tz,
    session: &mut PassthroughSession,
) -> String {
    let lines: Vec<&str> = split_lines(&message.content)
        .into_iter()
        .map(strip)
        .filter(|line| !line.is_empty())
        .collect();
    let label = session.author_label(&message.author_id, &message.author_name);
    let mention = session.mention(&message.author_id);
    let content = session.text(&lines.join(" / "));
    let id = session.message_ref(&message.id);
    format!(
        "[{id}] [{}] [{label} {mention}] {content}",
        render_time(message.created_at, zone)
    )
}

/// One line per boss: canonical forms, full name, and up to six chat aliases.
pub(super) fn render_bosses(table: &BossTable, only: &BTreeSet<String>) -> String {
    let lines: Vec<String> = table
        .bosses()
        .iter()
        .filter(|boss| only.is_empty() || only.contains(boss.short()))
        .map(|boss| {
            let forms: Vec<String> = boss
                .difficulties()
                .iter()
                .map(|letter| boss.canonical(letter))
                .collect();
            let aliases: Vec<&str> = boss.aliases().iter().take(6).map(String::as_str).collect();
            let mut line = format!("  {}  = {}", forms.join(", "), boss.full());
            if !aliases.is_empty() {
                line.push_str(&format!("  (chat: {})", aliases.join(", ")));
            }
            line
        })
        .collect();
    lines.join("\n")
}

fn who(
    participants: &[String],
    names: &HashMap<&str, &str>,
    session: &mut PassthroughSession,
) -> String {
    let parts: Vec<String> = participants
        .iter()
        .map(|uid| {
            let name = names.get(uid.as_str()).copied().unwrap_or("?");
            format!(
                "{}({})",
                session.mention(uid),
                session.author_label(uid, name)
            )
        })
        .collect();
    parts.join(" ")
}

fn boss_list(bosses: &[String]) -> String {
    if bosses.is_empty() {
        "(none)".to_owned()
    } else {
        bosses.join(" + ")
    }
}

pub(super) fn render_run(
    run: &Run,
    zone: Tz,
    names: &HashMap<&str, &str>,
    session: &mut PassthroughSession,
) -> String {
    let who = who(&run.participants, names, session);
    let when = if run.status == RunStatus::Otot {
        "own time".to_owned()
    } else {
        render_time(run.datetime, zone)
    };
    format!(
        "  #{}  {}  {when}  [{}]  {who}",
        short_id(&run.id),
        boss_list(&run.bosses),
        run.status.as_str()
    )
}

pub(super) fn render_fixed(
    fixed: &FixedRun,
    names: &HashMap<&str, &str>,
    session: &mut PassthroughSession,
) -> String {
    let who = who(&fixed.participants, names, session);
    format!(
        "  #{}  {}  every {} {:02}:{:02}  {who}",
        short_id(&fixed.id),
        boss_list(&fixed.bosses),
        weekday_name(fixed.weekday),
        fixed.time.hour(),
        fixed.time.minute()
    )
}

fn messages<'a>(context: &'a PromptContext<'_>) -> impl Iterator<Item = &'a PromptMessage> {
    context.burst.iter().chain(context.context)
}

/// The members worth naming: authors, people mentioned, and participants of
/// this channel's runs and timings; the whole roster when none of them is on it.
pub fn relevant_roster<'a>(context: &PromptContext<'a>) -> Vec<&'a Member> {
    let mut wanted: HashSet<&str> = HashSet::new();
    for message in messages(context) {
        wanted.insert(&message.author_id);
        wanted.extend(
            MENTION
                .captures_iter(&message.content)
                .map(|found| found.get(1).expect("group").as_str()),
        );
    }
    let participants = context.runs.iter().flat_map(|run| &run.participants).chain(
        context
            .fixed_runs
            .iter()
            .flat_map(|fixed| &fixed.participants),
    );
    wanted.extend(participants.map(String::as_str));
    let ordered: Vec<&Member> = context
        .roster
        .iter()
        .filter(|member| wanted.contains(member.user_id.as_str()))
        .collect();
    if ordered.is_empty() {
        context.roster.iter().collect()
    } else {
        ordered
    }
}

/// Short names of every boss mentioned in the messages or already in a run here.
pub fn named_bosses(context: &PromptContext<'_>) -> BTreeSet<String> {
    let lexicon = BossLexicon::new(context.table);
    let mut found: BTreeSet<String> = messages(context)
        .flat_map(|message| find_bosses(&message.content, &lexicon))
        .map(|hit| hit.short)
        .collect();
    let canonicals = context
        .runs
        .iter()
        .flat_map(|run| &run.bosses)
        .chain(context.fixed_runs.iter().flat_map(|fixed| &fixed.bosses));
    for canonical in canonicals {
        if let Some((_, boss)) = context.table.split(canonical) {
            found.insert(boss.short().to_owned());
        }
    }
    found
}
