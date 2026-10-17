//! Header-rewrite probes (`--headers`, workplan `kanade-beta9` header-probes,
//! D13): the real reminder header rewrite (`HeadingRewrite::trial_with`, the
//! `rewrite` role as serve's card kit builds it, the bundle under test) on
//! day-of and countdown seeds, with and without the estimate context of
//! wholly invented parties. Each try writes one JSON (prompt, reply,
//! verdict, rule, template and filled line) and `summary.md` counts, per
//! kind and group, accepted lines, raw names in replies, lines singling out
//! a member who was not offered, and figures restated.

use std::{path::Path, time::Duration};

use kanade::{
    bot::delivery::{
        RunEstimate,
        cards::{HeaderContext, HeaderKind, HeadingRewrite, PhraseKind, TrialOrigin},
    },
    chat::persona::NudgeMood,
    domain::{
        catalog::BossTable,
        chardata::{Carrier, PartyShares},
        model_log::RewriteStage,
    },
    infrastructure::scouter::CarrierOwner,
};
use serde_json::{Value, json};

/// One rewrite's budget, as pre-generation gives it.
const DEADLINE: Duration = Duration::from_secs(30);
/// Tries per case (D13: at least 10 per group per kind).
pub const DEFAULT_TRIES: u32 = 10;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// No context: today's prompt.
    Baseline,
    /// Fresh, complete estimates with two offered carriers.
    Context,
    /// As `Context`, with an assumed critical rate.
    Crit,
    /// As `Context`, with a party member without an estimate.
    Incomplete,
    /// Context in the gentle mood: totals, no call-outs.
    Gentle,
}

impl Group {
    fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Context => "context",
            Self::Crit => "crit",
            Self::Incomplete => "incomplete",
            Self::Gentle => "gentle",
        }
    }
}

pub struct HeaderCase {
    pub id: &'static str,
    pub kind: HeaderKind,
    pub group: Group,
}

const COUNTDOWN: HeaderKind = HeaderKind::Phrase(PhraseKind::Countdown);

pub const CASES: [HeaderCase; 10] = [
    case("HD-BASE", HeaderKind::DayOf, Group::Baseline),
    case("HD-CTX", HeaderKind::DayOf, Group::Context),
    case("HD-CRIT", HeaderKind::DayOf, Group::Crit),
    case("HD-PART", HeaderKind::DayOf, Group::Incomplete),
    case("HD-GENT", HeaderKind::DayOf, Group::Gentle),
    case("HC-BASE", COUNTDOWN, Group::Baseline),
    case("HC-CTX", COUNTDOWN, Group::Context),
    case("HC-CRIT", COUNTDOWN, Group::Crit),
    case("HC-PART", COUNTDOWN, Group::Incomplete),
    case("HC-GENT", COUNTDOWN, Group::Gentle),
];

const fn case(id: &'static str, kind: HeaderKind, group: Group) -> HeaderCase {
    HeaderCase { id, kind, group }
}

fn kind_name(kind: HeaderKind) -> &'static str {
    match kind {
        HeaderKind::DayOf => "day_of",
        HeaderKind::Phrase(_) => "countdown",
    }
}

/// The invented party's member names (never real members).
const MEMBER_NAMES: [&str; 4] = ["Marisol Vey", "quillon", "Bramblewick", "Odette"];
/// The invented characters: carriers in party order, plus an alt.
const IGNS: [&str; 5] = ["Lumirae", "Tobiwan", "Kestrelle", "Faywick", "LumiraeAlt"];

fn carrier(ign: &str, crit: bool, effective: &[Option<f64>]) -> Carrier {
    Carrier {
        ign: ign.into(),
        crit_assumed: crit,
        effective: effective.to_vec(),
        per_boss: Vec::new(),
    }
}

/// The group's invented estimate: two bosses, four carriers.
fn estimate(group: Group) -> RunEstimate {
    let crit = group == Group::Crit;
    let missing = if group == Group::Incomplete {
        None
    } else {
        Some(4.0)
    };
    RunEstimate {
        party: PartyShares {
            bosses: vec!["HKalos".into(), "HLimbo".into()],
            carriers: vec![
                carrier(IGNS[0], crit, &[Some(52.4), Some(47.1)]),
                carrier(IGNS[1], false, &[Some(31.0), Some(29.5)]),
                carrier(IGNS[2], false, &[Some(18.2), Some(16.0)]),
                carrier(IGNS[3], false, &[missing, Some(5.5)]),
            ],
            missing: 0,
        },
        minutes: vec![Some(23.4), None],
        possible: vec![Some(true), None],
        owners: (1..=4)
            .map(|n| CarrierOwner {
                member_id: format!("90000000000000000{n}"),
                character_id: format!("char-{n}"),
            })
            .collect(),
        igns: IGNS.iter().map(|ign| (*ign).to_owned()).collect(),
        ..RunEstimate::default()
    }
}

fn context(group: Group) -> Option<HeaderContext> {
    let mood = if group == Group::Gentle {
        NudgeMood::Gentle
    } else {
        NudgeMood::Playful
    };
    match group {
        Group::Baseline => None,
        _ => HeaderContext::build(
            &[estimate(group)],
            mood,
            MEMBER_NAMES.iter().map(|name| (*name).to_owned()),
        ),
    }
}

/// The selected cases, as listed by `--dry-run`.
pub fn selected(ids: Option<&[String]>) -> Vec<&'static HeaderCase> {
    CASES
        .iter()
        .filter(|case| ids.is_none_or(|ids| ids.iter().any(|id| id == case.id)))
        .collect()
}

pub fn known(id: &str) -> bool {
    CASES.iter().any(|case| case.id == id)
}

/// One line per case: id, kind, group, tries and what its prompt carries.
pub fn listing(cases: &[&HeaderCase], tries: u32) -> String {
    let mut out = String::new();
    for case in cases {
        let carries = match context(case.group) {
            None => "no context".to_owned(),
            Some(context) => format!(
                "{} boss(es), {} offered slot(s)",
                context.facts.bosses.len(),
                context.fills.len()
            ),
        };
        out.push_str(&format!(
            "{}\t{}\t{}\t{tries} tries\t{carries}\n",
            case.id,
            kind_name(case.kind),
            case.group.as_str()
        ));
    }
    out
}

/// Every word-ish token of `text`, lowercased.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn mentions(text: &str, name: &str) -> bool {
    let tokens = words(text);
    let name = words(name);
    tokens
        .windows(name.len())
        .any(|window| window == name.as_slice())
}

/// One try: the trial with what the scorer needs.
async fn attempt(
    heading: &HeadingRewrite,
    catalog: &BossTable,
    case: &HeaderCase,
    n: u32,
) -> Value {
    let context = context(case.group);
    let origin = TrialOrigin::new(RewriteStage::Debug, format!("v02 {}-{n}", case.id));
    let prompt = heading.prompt_with(case.kind, context.as_ref());
    let trial = heading
        .trial_with(
            case.kind,
            Some(catalog),
            DEADLINE,
            &origin,
            context.as_ref(),
        )
        .await;
    let reply = trial.output.clone().unwrap_or_default();
    let filled = match &trial.slots {
        Some(slots) => slots.fill(&trial.line),
        None => Some(trial.line.clone()),
    };
    let offered: Vec<String> = context
        .as_ref()
        .map(|context| context.fills.iter().map(|fill| fill.ign.clone()).collect())
        .unwrap_or_default();
    let raw_names: Vec<&str> = MEMBER_NAMES
        .iter()
        .chain(IGNS.iter())
        .copied()
        .filter(|name| mentions(&reply, name))
        .collect();
    // A placeholder that was not offered, or a name outside the offered ones.
    let unoffered = reply.contains("{c")
        && !context
            .as_ref()
            .is_some_and(|context| context.offered().iter().any(|slot| reply.contains(slot)))
        || raw_names
            .iter()
            .any(|name| !offered.iter().any(|ign| ign == name));
    let figures = reply.contains('%') || reply.chars().any(|c| c.is_ascii_digit());
    json!({
        "case": case.id,
        "attempt": n,
        "kind": kind_name(case.kind),
        "group": case.group.as_str(),
        "context": context.is_some(),
        "prompt": prompt.map(|prompt| prompt.transcript()),
        "reply": trial.output,
        "verdict": trial.verdict.reason(),
        "rule": trial.verdict.rule(),
        "code": trial.code(),
        "template": trial.line,
        "line": filled,
        "slots": trial.slots.map(|slots| slots.slots.len()).unwrap_or(0),
        "raw_names": raw_names,
        "unoffered": unoffered,
        "figures": figures,
    })
}

/// Run every selected case `tries` times; one JSON per try in `out`.
pub async fn run(
    heading: &HeadingRewrite,
    catalog: &BossTable,
    cases: &[&HeaderCase],
    tries: u32,
    out: &Path,
    write: impl Fn(&Path, &Value) -> Result<(), String>,
) -> Result<Vec<Value>, String> {
    let mut records = Vec::new();
    for case in cases {
        for n in 1..=tries {
            let record = attempt(heading, catalog, case, n).await;
            write(&out.join(format!("{}-{n}.json", case.id)), &record)?;
            eprintln!(
                "v02 headers: {}-{n} {}{}",
                case.id,
                record["verdict"].as_str().unwrap_or_default(),
                record["rule"]
                    .as_str()
                    .map(|rule| format!(" ({rule})"))
                    .unwrap_or_default()
            );
            records.push(record);
        }
    }
    Ok(records)
}

/// Counts per case and every accepted line (D13/D16 review).
pub fn summary(meta: &Value, records: &[Value]) -> String {
    let mut out = format!(
        "# V02 header probes {}\n\nPersona `{}` (sha256 `{}`), rewrite route {}.\n\n| case | kind | group | calls | accepted | rate | raw names | unoffered | figures |\n|---|---|---|---|---|---|---|---|---|\n",
        meta["run"].as_str().unwrap_or_default(),
        meta["persona"]["id"].as_str().unwrap_or_default(),
        meta["persona"]["sha256"].as_str().unwrap_or_default(),
        meta["route"],
    );
    for case in &CASES {
        let rows: Vec<&Value> = records.iter().filter(|r| r["case"] == case.id).collect();
        if rows.is_empty() {
            continue;
        }
        let count = |test: &dyn Fn(&Value) -> bool| rows.iter().filter(|r| test(r)).count();
        let accepted = count(&|r| r["verdict"] == "accepted");
        out.push_str(&format!(
            "| {} | {} | {} | {} | {accepted} | {:.0}% | {} | {} | {} |\n",
            case.id,
            kind_name(case.kind),
            case.group.as_str(),
            rows.len(),
            100.0 * accepted as f64 / rows.len() as f64,
            count(&|r| r["raw_names"]
                .as_array()
                .is_some_and(|names| !names.is_empty())),
            count(&|r| r["unoffered"] == true),
            count(&|r| r["figures"] == true),
        ));
    }
    out.push_str("\n## Rejections\n\n");
    for record in records.iter().filter(|r| r["verdict"] != "accepted") {
        out.push_str(&format!(
            "- {}-{}: {} {} — `{}`\n",
            record["case"].as_str().unwrap_or_default(),
            record["attempt"],
            record["verdict"].as_str().unwrap_or_default(),
            record["rule"].as_str().unwrap_or_default(),
            record["reply"]
                .as_str()
                .unwrap_or_default()
                .replace('`', "ˋ"),
        ));
    }
    out.push_str("\n## Accepted lines\n\n");
    for record in records.iter().filter(|r| r["verdict"] == "accepted") {
        out.push_str(&format!(
            "- {}-{}: {} → {}\n",
            record["case"].as_str().unwrap_or_default(),
            record["attempt"],
            record["template"].as_str().unwrap_or_default(),
            record["line"].as_str().unwrap_or("(no fill)"),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_groups_offer_slots_and_gentle_none() {
        for case in &CASES {
            let context = context(case.group);
            match case.group {
                Group::Baseline => assert!(context.is_none()),
                Group::Gentle => assert!(context.unwrap().fills.is_empty()),
                _ => {
                    let context = context.expect("context");
                    assert_eq!(context.fills.len(), 2, "{}", case.id);
                    assert!(!context.facts.block().contains("Lumirae"));
                }
            }
        }
        let crit = context(Group::Crit).unwrap();
        assert!(crit.facts.bosses[0].crit_assumed);
        let part = context(Group::Incomplete).unwrap();
        assert!(!part.facts.bosses[0].complete);
    }
}
