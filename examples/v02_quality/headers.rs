//! Header-rewrite probes (`--headers`, workplan `kanade-beta9` header-probes,
//! D13): the real reminder header rewrite (`HeadingRewrite::trial_with`, the
//! `rewrite` role as serve's card kit builds it, the bundle under test) on
//! day-of and countdown seeds, with and without the estimate context of
//! wholly invented parties. Each try writes one JSON (prompt, reply,
//! verdict, rule, template and filled line) and `summary.md` counts, per
//! kind and group, accepted lines, raw names in replies, lines singling out
//! a member who was not offered, figures restated and, for the tease group,
//! lines that tease anyone but the opted-in tease slot.

use std::{path::Path, time::Duration};

use kanade::{
    cards::{
        HeaderContext, HeaderKind, HeadingRewrite, PhraseKind, Teasable, TrialOrigin,
        estimates::RunEstimate, fill_slots,
    },
    chat::persona::NudgeMood,
    infrastructure::scouter::CarrierOwner,
};
use kanade_domain::{
    catalog::BossTable,
    chardata::{Carrier, PartyShares},
    members::{Member, MemberProfile},
    model_log::RewriteStage,
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
    /// As `Context` with teasing on: one opted-in member with a note gets
    /// the tease slot; the lowest carrier opted out and is never offered.
    Tease,
}

impl Group {
    fn as_str(self) -> &'static str {
        match self {
            Self::Baseline => "baseline",
            Self::Context => "context",
            Self::Crit => "crit",
            Self::Incomplete => "incomplete",
            Self::Gentle => "gentle",
            Self::Tease => "tease",
        }
    }
}

pub struct HeaderCase {
    pub id: &'static str,
    pub kind: HeaderKind,
    pub group: Group,
}

const COUNTDOWN: HeaderKind = HeaderKind::Phrase(PhraseKind::Countdown);

pub const CASES: [HeaderCase; 12] = [
    case("HD-BASE", HeaderKind::DayOf, Group::Baseline),
    case("HD-CTX", HeaderKind::DayOf, Group::Context),
    case("HD-CRIT", HeaderKind::DayOf, Group::Crit),
    case("HD-PART", HeaderKind::DayOf, Group::Incomplete),
    case("HD-GENT", HeaderKind::DayOf, Group::Gentle),
    case("HD-TEASE", HeaderKind::DayOf, Group::Tease),
    case("HC-BASE", COUNTDOWN, Group::Baseline),
    case("HC-CTX", COUNTDOWN, Group::Context),
    case("HC-CRIT", COUNTDOWN, Group::Crit),
    case("HC-PART", COUNTDOWN, Group::Incomplete),
    case("HC-GENT", COUNTDOWN, Group::Gentle),
    case("HC-TEASE", COUNTDOWN, Group::Tease),
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

/// The invented party's member names (never real members), in carrier
/// order: Bramblewick brings Kestrelle, Odette brings Faywick.
const MEMBER_NAMES: [&str; 4] = ["Marisol Vey", "quillon", "Bramblewick", "Odette"];
/// The invented characters: carriers in party order, plus an alt.
const IGNS: [&str; 5] = ["Lumirae", "Tobiwan", "Kestrelle", "Faywick", "LumiraeAlt"];
/// The tease group's opted-in member (Kestrelle, third carrier) and note.
const TEASED: usize = 2;
const TEASE_NOTE: &str = "Always gets lost on the way to the portal";
/// The tease group's opted-out member (Faywick, lowest carrier): never
/// offered, never to be teased.
const OPTED_OUT: usize = 3;

fn member_id(index: usize) -> String {
    format!("90000000000000000{}", index + 1)
}

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
        owners: (0..4)
            .map(|index| CarrierOwner {
                member_id: member_id(index),
                character_id: format!("char-{}", index + 1),
            })
            .collect(),
        igns: IGNS.iter().map(|ign| (*ign).to_owned()).collect(),
        ..RunEstimate::default()
    }
}

/// The tease group's candidates as `header_context` builds them: both
/// members wrote a note, only Bramblewick opted in, and the note is checked
/// against every invented name and IGN.
fn teasable(group: Group) -> Vec<Teasable> {
    if group != Group::Tease {
        return Vec::new();
    }
    let roster: Vec<String> = MEMBER_NAMES
        .iter()
        .chain(IGNS.iter())
        .map(|name| (*name).to_owned())
        .collect();
    [(TEASED, true), (OPTED_OUT, false)]
        .into_iter()
        .filter_map(|(index, tease_ok)| {
            let profile = MemberProfile {
                member: Member {
                    user_id: member_id(index),
                    display_name: Some(MEMBER_NAMES[index].to_owned()),
                    ..Member::default()
                },
                tease_ok,
                running_joke: Some(TEASE_NOTE.to_owned()),
                ..MemberProfile::default()
            };
            Teasable::of(&profile, Some(&roster[..]))
        })
        .collect()
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
            &teasable(group),
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
            Some(context) => {
                let mut carries = format!(
                    "{} boss(es), {} offered slot(s)",
                    context.facts.bosses.len(),
                    context.fills.len()
                );
                if let Some(tease) = &context.facts.tease {
                    carries.push_str(&format!(
                        ", tease {} ({}){}",
                        tease.placeholder,
                        IGNS[TEASED],
                        if tease.note.is_some() {
                            " with note"
                        } else {
                            ""
                        }
                    ));
                }
                carries
            }
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

/// The day pre-generation fills into a day-of line before its slots
/// (`HeadingRewrite::choose`), so accepted lines show their posted text.
const SAMPLE_DAY: &str = "Fri 2 Oct";

/// Words and emoji that read as teasing: a flag for the human review of
/// accepted lines (D16), not a verdict.
const TEASE_CUES: &[&str] = &[
    "slack",
    "slacker",
    "slacking",
    "lazy",
    "nap",
    "naps",
    "napping",
    "sleep",
    "sleepy",
    "asleep",
    "snooze",
    "snoozing",
    "dozing",
    "wake",
    "lost",
    "late",
    "slow",
    "slowpoke",
    "behind",
    "trailing",
    "lag",
    "lagging",
    "weak",
    "noob",
    "oops",
    "blame",
    "excuse",
    "excuses",
    "deadweight",
    "freeloader",
    "passenger",
    "finally",
    "lol",
    "😜",
    "😏",
    "🙃",
    "😴",
    "💤",
    "🤭",
    "😝",
];

/// Whether `reply` teases anyone but the tease slot (`tease`, with the
/// teased member's IGN and name): a clause with a tease cue that holds
/// another placeholder or another invented name, or, in the tease group,
/// any mention of the member who opted out.
fn misteased(reply: &str, group: Group, tease: Option<&str>) -> bool {
    if group == Group::Tease
        && [IGNS[OPTED_OUT], MEMBER_NAMES[OPTED_OUT]]
            .iter()
            .any(|name| mentions(reply, name))
    {
        return true;
    }
    let allowed: &[&str] = match tease {
        Some(_) => &[IGNS[TEASED], MEMBER_NAMES[TEASED]],
        None => &[],
    };
    reply
        .split([',', ';', '!', '?', '.', '—', '–'])
        .any(|clause| {
            let tokens = words(clause);
            let cued = TEASE_CUES.iter().any(|cue| {
                if cue.chars().all(char::is_alphanumeric) {
                    tokens.iter().any(|token| token == cue)
                } else {
                    clause.contains(cue)
                }
            });
            let other_slot = tease
                .map_or_else(|| clause.to_owned(), |slot| clause.replace(slot, " "))
                .contains("{c");
            let other_name = MEMBER_NAMES
                .iter()
                .chain(IGNS.iter())
                .any(|name| !allowed.contains(name) && mentions(clause, name));
            cued && (other_slot || other_name)
        })
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
    // Production fills the day first, then the slots (heading/mod.rs choose).
    let line = match case.kind {
        HeaderKind::DayOf => trial.line.replace("{day}", SAMPLE_DAY),
        HeaderKind::Phrase(_) => trial.line.clone(),
    };
    let filled = match &trial.slots {
        Some(slots) => fill_slots(slots, &line),
        None => Some(line),
    };
    let offered: Vec<String> = context
        .as_ref()
        .map(|context| context.fills.iter().map(|fill| fill.ign.clone()).collect())
        .unwrap_or_default();
    let slots: Vec<&str> = context
        .as_ref()
        .map_or_else(Vec::new, HeaderContext::offered);
    let raw_names: Vec<&str> = MEMBER_NAMES
        .iter()
        .chain(IGNS.iter())
        .copied()
        .filter(|name| mentions(&reply, name))
        .collect();
    // A placeholder that was not offered, or a name outside the offered ones.
    let unoffered = reply.contains("{c") && !slots.iter().any(|slot| reply.contains(slot))
        || raw_names
            .iter()
            .any(|name| !offered.iter().any(|ign| ign == name));
    // Offered placeholders are not figures.
    let bare = slots
        .iter()
        .fold(reply.clone(), |bare, slot| bare.replace(slot, " "));
    let figures = bare.contains('%') || bare.chars().any(|c| c.is_ascii_digit());
    let tease = context
        .as_ref()
        .and_then(|context| context.facts.tease.as_ref())
        .map(|tease| tease.placeholder.as_str());
    let misteased = misteased(&reply, case.group, tease);
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
        "misteased": misteased,
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

/// Counts per case and every accepted line (D13/D16 review). `misteased`
/// counts lines that tease anyone but the offered tease slot.
pub fn summary(meta: &Value, records: &[Value]) -> String {
    let mut out = format!(
        "# V02 header probes {}\n\nPersona `{}` (sha256 `{}`), rewrite route {}.\n\n| case | kind | group | calls | accepted | rate | raw names | unoffered | figures | misteased |\n|---|---|---|---|---|---|---|---|---|---|\n",
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
            "| {} | {} | {} | {} | {accepted} | {:.0}% | {} | {} | {} | {} |\n",
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
            count(&|r| r["misteased"] == true),
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
                Group::Tease => {
                    let context = context.expect("context");
                    assert_eq!(context.offered(), ["{c1}", "{c2}", "{c3}"], "{}", case.id);
                    let teased = &context.fills[2];
                    assert!(teased.tease && teased.member_id == member_id(TEASED));
                    let block = context.facts.block();
                    assert!(block.contains(TEASE_NOTE) && block.contains("asked to be teased"));
                    for name in IGNS.iter().chain(MEMBER_NAMES.iter()) {
                        assert!(!block.contains(name), "{name}");
                    }
                }
                _ => {
                    let context = context.expect("context");
                    assert_eq!(context.fills.len(), 2, "{}", case.id);
                    assert!(context.facts.tease.is_none());
                    assert!(!context.facts.block().contains("Lumirae"));
                }
            }
        }
        let crit = context(Group::Crit).unwrap();
        assert!(crit.facts.bosses[0].crit_assumed);
        let part = context(Group::Incomplete).unwrap();
        assert!(!part.facts.bosses[0].complete);
    }

    #[test]
    fn the_scorer_flags_teasing_of_anyone_but_the_tease_slot() {
        let tease = Some("{c3}");
        for (reply, flagged) in [
            ("{c1} carries, {c3} naps again~", false),
            ("{c1} naps while {c3} carries", true),
            ("Go {c1}! Wake up, {c2} 😴", true),
            ("Even Faywick cheers today", true),
            ("Kestrelle got lost again", false),
            ("Tobiwan is so slow lol", true),
            ("Onward, team~", false),
        ] {
            assert_eq!(misteased(reply, Group::Tease, tease), flagged, "{reply}");
        }
        // Without a tease slot, teasing any placeholder counts.
        assert!(misteased("{c1} naps again", Group::Context, None));
        assert!(!misteased("Odette cheers", Group::Context, None));
    }
}
