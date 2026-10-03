//! `sanitize.json`: note defusing, member-facing rewrites, false-claim
//! stripping, trusted defaults, tidy bounds and schedule regrounding.

use std::sync::LazyLock;

use kanade::chat::sanitize::{
    self, defuse_notes, looks_like_clarification, member_facing, reply_parts, schedule_defaults,
    strip_false_card_claim, tidy,
};
use kanade::chat::tools::ToolOutcome;
use kanade::domain::catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec};
use serde_json::{Map, Value, json};

use crate::common::text;
use crate::support::{Named, check_family, dev, unknown_op, value};

/// The fixtures' boss catalog (aliases included); `will` is an ordinary word.
static CATALOG: LazyLock<BossTable> = LazyLock::new(|| {
    let difficulty = |prefix: &str, label: &str| DifficultySpec {
        prefix: prefix.to_owned(),
        label: label.to_owned(),
    };
    let boss = |short: &str, aliases: &[&str]| BossSpec {
        short: short.to_owned(),
        aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
        ..BossSpec::default()
    };
    BossTable::from_spec(&CatalogSpec {
        difficulties: vec![
            difficulty("e", "Easy"),
            difficulty("n", "Normal"),
            difficulty("h", "Hard"),
            difficulty("c", "Chaos"),
            difficulty("x", "Extreme"),
        ],
        bosses: vec![
            boss("Lotus", &["lotus", "lot"]),
            boss("Vellum", &["vellum"]),
            boss("Magnus", &["magnus"]),
            boss("Lucid", &["lucid"]),
            boss("Kalos", &["kalos", "gatekeeper"]),
            boss("Carling", &["carling", "karling", "kaling", "carl", "karl"]),
            boss("Will", &["will"]),
            BossSpec {
                full: Some("Radiant Malefic Star".to_owned()),
                ..boss("MaleficStar", &["star", "malefic"])
            },
            boss("FA", &["fa"]),
        ],
    })
    .expect("fixture catalog is valid")
});

fn ground_schedule_reply(reply: &str, outcomes: &[ToolOutcome]) -> String {
    sanitize::ground_schedule_reply(reply, outcomes, &CATALOG)
}

fn shape_reply(reply: &str, outcomes: &[ToolOutcome]) -> String {
    sanitize::shape_reply(reply, outcomes, &CATALOG)
}

/// The vectors' outcome descriptors as tool outcomes.
fn outcomes(raw: &Value) -> Vec<ToolOutcome> {
    raw.as_array()
        .expect("outcomes")
        .iter()
        .map(|item| {
            assert_eq!(item["posted"], json!([]), "sanitize outcomes post nothing");
            ToolOutcome {
                name: text(&item["name"]).to_owned(),
                output: text(&item["output"]).to_owned(),
                arguments: Map::new(),
                ok: item["ok"].as_bool().expect("ok"),
                error: match item["error"].as_str() {
                    None => None,
                    Some("refused") => Some(kanade::chat::tools::REFUSED),
                    Some(other) => panic!("unexpected outcome error {other}"),
                },
                created: Vec::new(),
                cards: Vec::new(),
                detail: None,
            }
        })
        .collect()
}

fn replay(case: &Value) -> Vec<Value> {
    case["input"]["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| {
            let input = || text(&step["text"]);
            value(match text(&step["op"]) {
                "defuse_notes" => json!(defuse_notes(input())),
                "member_facing" => json!(member_facing(input())),
                "strip_false_card_claim" => json!(strip_false_card_claim(input())),
                "looks_like_clarification" => json!(looks_like_clarification(input())),
                "schedule_defaults" => {
                    let found = schedule_defaults(
                        input(),
                        step["bot_user_id"].as_str(),
                        step["self_role_id"].as_str(),
                    );
                    json!({
                        "force_all_channels": found.force_all_channels,
                        "force_channel_scope": found.force_channel_scope,
                        "force_group_schedule": found.force_group_schedule,
                        "upcoming_only": found.upcoming_only,
                    })
                }
                "tidy" => json!(tidy(input(), step["protected"].as_str())),
                "ground_schedule_reply" => json!(ground_schedule_reply(
                    text(&step["reply"]),
                    &outcomes(&step["outcomes"])
                )),
                "shape_reply" => json!(shape_reply(
                    text(&step["reply"]),
                    &outcomes(&step["outcomes"])
                )),
                other => unknown_op("sanitize", other),
            })
        })
        .collect()
}

/// `D-GROUND-FILTERED`: a reply naming one of two listed runs gets that
/// run's record, not the whole listing.
fn named() -> Vec<Named> {
    vec![
        Named {
            name: "D-GROUND-FILTERED",
            entries: vec![dev(
                "schedule-grounding",
                "/steps/8/value",
                json!(
                    "**2 runs this week · All channels**\n\n`[9004eab0]` **Hard MaleficStar**\n*Tue 08 Sep · 00:00* · `planned` · `2/3 yes`\n\n`[9004eab1]` **Extreme Kalos**\n*Wed 09 Sep · 00:00* · `planned` · `2/3 yes`"
                ),
                json!(
                    "`[9004eab1]` **Extreme Kalos**\n*Wed 09 Sep · 00:00* · `planned` · `2/3 yes`"
                ),
            )],
        },
        voiced_card(),
    ]
}

/// `D-VOICED-CARD` (user decision 2026-10-03): a line citing a listed id and
/// stating no fact reads the id as the run's label, records placed under it
/// come without the listing heading, and one stating a fact (`later`) is
/// replaced by the cards.
fn voiced_card() -> Named {
    let malefic = "`[9004eab0]` **Hard MaleficStar**\n*Tue 08 Sep · 00:00* · `planned` · `2/3 yes`";
    let kalos = "`[9004eab1]` **Extreme Kalos**\n*Wed 09 Sep · 00:00* · `planned` · `2/3 yes`";
    let two = format!("**2 runs this week · All channels**\n\n{malefic}\n\n{kalos}");
    Named {
        name: "D-VOICED-CARD",
        entries: vec![
            dev(
                "schedule-grounding",
                "/steps/3/value",
                json!(format!(
                    "I saved 9004eab0 for later.\n\n**1 run this week · All channels**\n\n{malefic}\n\n9004eab0 is still the reference for the card."
                )),
                json!(format!(
                    "**1 run this week · All channels**\n\n{malefic}\n\n**Hard MaleficStar** is still the reference for the card."
                )),
            ),
            dev(
                "schedule-grounding",
                "/steps/4/value",
                json!(format!("{two}\n\nKeep 9004eab0 handy.")),
                json!(format!("{two}\n\nKeep **Hard MaleficStar** handy.")),
            ),
        ],
    }
}

#[tokio::test]
async fn the_sanitize_family_replays_exactly() {
    let counts = check_family("sanitize", &named(), |case| async move { replay(&case) }).await;
    assert_eq!(counts, (6, 64));
}

/// `D-CODE-FENCES`: fenced code survives shaping byte for byte.
#[test]
fn fenced_code_keeps_its_indentation_through_shaping() {
    let code = "```python\ndef two_sum(nums, target):\n    seen = {}\n\n\n    for i, n in enumerate(nums):\n        if target - n in seen:  # found\n            return [seen[target - n], i]\n```";
    let reply = format!("Here  you go:\n\n{code}\n\nNext run  is Tuesday .");
    let shaped = shape_reply(&reply, &[]);
    assert!(shaped.contains(code), "{shaped}");
    assert!(shaped.starts_with("Here you go:"), "{shaped}");
    assert!(shaped.ends_with("Next run is Tuesday."), "{shaped}");
}

/// `D-ELLIPSIS`: an ellipsis keeps every dot; stray dots are still tidied.
#[test]
fn ellipses_survive_member_facing() {
    assert_eq!(member_facing("Mou... fine."), "Mou... fine.");
    assert_eq!(member_facing("Well .... maybe"), "Well .... maybe");
    assert_eq!(member_facing("Done . ."), "Done.");
    assert_eq!(member_facing("Done.."), "Done.");
}

fn schedule_outcome(output: &str) -> ToolOutcome {
    ToolOutcome {
        name: "get_schedule".to_owned(),
        output: output.to_owned(),
        arguments: Map::new(),
        ok: true,
        error: None,
        created: Vec::new(),
        cards: Vec::new(),
        detail: None,
    }
}

const PAST: &str = " · *already happened*";

/// Six runs this week, the first four already over (the live 11fedae9 case).
fn six_runs() -> String {
    let runs = [
        ("1a2b3c4d", "Hard Lucid", "Mon 21 Sep · 21:00", PAST),
        ("2b3c4d5e", "Hard Will", "Tue 22 Sep · 21:00", PAST),
        ("3c4d5e6f", "Chaos Slime", "Wed 23 Sep · 22:00", PAST),
        ("4d5e6f70", "Hard Kaling", "Fri 25 Sep · 22:00", PAST),
        ("7c5f4680", "Hard Baldrix", "Sun 27 Sep · 22:00", ""),
        ("8d6e5791", "Extreme Kalos", "Sun 27 Sep · 23:30", ""),
    ];
    let mut parts = vec!["**Your 6 runs this week · All channels**".to_owned()];
    parts.extend(runs.iter().map(|(id, boss, when, past)| {
        format!("`[{id}]` **{boss}**\n*{when}* · `planned` · `2/6 yes` · <#9001>{past}")
    }));
    parts.join("\n\n")
}

const BALDRIX: &str =
    "`[7c5f4680]` **Hard Baldrix**\n*Sun 27 Sep · 22:00* · `planned` · `2/6 yes` · <#9001>";

fn rust_fence(lines: usize) -> String {
    let mut code = vec![
        "```rust".to_owned(),
        "fn longest_palindrome(s: &str) -> &str {".to_owned(),
        "    let bytes = s.as_bytes();".to_owned(),
        "    let (mut best_start, mut best_len) = (0, 0);".to_owned(),
    ];
    for step in 0..lines {
        code.push(format!(
            "    // expand around centre {step}: {step} + 1 keeps  the  spacing"
        ));
    }
    code.push("    &s[best_start..best_start + best_len]".to_owned());
    code.push("}".to_owned());
    code.push("```".to_owned());
    code.join("\n")
}

const CLOSING: &str = "Good luck tonight, you've got this!\n\nSee you at the run~";

/// `D-GROUND-FILTERED`: the live case keeps the code and the closing lines
/// and shows only the run the model named, canonically.
#[test]
fn grounding_keeps_the_answer_and_only_the_named_run() {
    let code = rust_fence(8);
    let reply = format!(
        "**Your next boss run**\n\n[7c5f4680] **Hard Baldrix** – *Sun 27 Sep · 22:00* – #hbaldguy\n\n{code}\n\n{CLOSING}"
    );
    let shaped = shape_reply(&reply, &[schedule_outcome(&six_runs())]);
    assert_eq!(
        shaped,
        format!("**Your next boss run**\n\n{BALDRIX}\n\n{code}\n\n{CLOSING}")
    );
}

/// A named run with a wrong time gets the tool's record.
#[test]
fn grounding_corrects_a_hallucinated_time() {
    let reply =
        "Next up:\n\n`[7c5f4680]` **Hard Baldrix**\n*Sun 27 Sep · 20:00* · `planned`\n\nBe there!";
    let shaped = shape_reply(reply, &[schedule_outcome(&six_runs())]);
    assert_eq!(shaped, format!("Next up:\n\n{BALDRIX}\n\nBe there!"));
}

/// An invented record next to a real one is dropped.
#[test]
fn grounding_drops_an_invented_run() {
    let reply = "Tonight:\n\n`[7c5f4680]` **Hard Baldrix** · 22:00\n`[deadbeef]` **Hard Seren** · 23:00\n\nBye!";
    let shaped = shape_reply(reply, &[schedule_outcome(&six_runs())]);
    assert_eq!(shaped, format!("Tonight:\n\n{BALDRIX}\n\nBye!"));
}

const KALOS: &str =
    "`[8d6e5791]` **Extreme Kalos**\n*Sun 27 Sep · 23:30* · `planned` · `2/6 yes` · <#9001>";

/// An invented line never takes a real run's line with it.
#[test]
fn an_invented_run_next_to_real_ones_keeps_them() {
    let seren = "`[deadbeef]` **Hard Seren** · 21:00";
    let baldrix = "`[7c5f4680]` **Hard Baldrix** · 22:00";
    let kalos = "`[8d6e5791]` **Extreme Kalos** · 23:30";
    for lines in [[seren, baldrix, kalos], [baldrix, seren, kalos]] {
        let reply = format!("Tonight:\n\n{}\n\nBye!", lines.join("\n"));
        let shaped = shape_reply(&reply, &[schedule_outcome(&six_runs())]);
        assert_eq!(shaped, format!("Tonight:\n\n{BALDRIX}\n\n{KALOS}\n\nBye!"));
    }
}

/// The listing goes where the first real run was, not an invented one.
#[test]
fn the_listing_replaces_the_first_real_run() {
    let reply = "Earlier:\n`[deadbeef]` **Hard Seren** · 21:00\n\nTonight:\n`[7c5f4680]` **Hard Baldrix** · 22:00\n\nBye!";
    let shaped = shape_reply(reply, &[schedule_outcome(&six_runs())]);
    assert_eq!(shaped, format!("Earlier:\n\nTonight:\n\n{BALDRIX}\n\nBye!"));
}

/// No listing, or code with no schedule text, leaves the answer whole.
#[test]
fn grounding_leaves_other_answers_alone() {
    let code = rust_fence(4);
    let reply = format!("Here it is:\n\n{code}\n\nEnjoy!");
    assert_eq!(shape_reply(&reply, &[]), reply);
    let with_code = format!("Here it is:\n\n{code}\n\n{CLOSING}");
    let shaped = shape_reply(&with_code, &[schedule_outcome(&six_runs())]);
    assert!(shaped.starts_with(&with_code), "{shaped}");
    assert!(shaped.ends_with(&six_runs()), "{shaped}");
    // A time inside code is not schedule text.
    let timed = "```text\nstarts 22:00\n```".to_owned();
    let shaped = shape_reply(
        &format!("Try:\n\n{timed}\n\nOk?"),
        &[schedule_outcome(&six_runs())],
    );
    assert!(shaped.contains(&timed), "{shaped}");
}

/// Over the bound, runs that already happened leave the listing; a reply
/// still over it is kept whole for follow-ups, the code untouched.
#[test]
fn an_over_long_reply_drops_past_runs_and_keeps_the_rest_whole() {
    let listed: String = six_runs()
        .split("\n\n")
        .skip(1)
        .collect::<Vec<_>>()
        .join("\n\n");
    let upcoming = six_runs()
        .split("\n\n")
        .skip(5)
        .collect::<Vec<_>>()
        .join("\n\n");
    for (lines, one_message) in [(6, true), (15, false)] {
        let code = rust_fence(lines);
        let reply = format!("Your week:\n\n{listed}\n\n{code}\n\n{CLOSING}");
        let shaped = shape_reply(&reply, &[schedule_outcome(&six_runs())]);
        assert_eq!(
            shaped,
            format!(
                "Your week:\n\n**Your 6 runs this week · All channels**\n\n{upcoming}\n\n*(and 4 more)*\n\n{code}\n\n{CLOSING}"
            )
        );
        assert_eq!(shaped.chars().count() <= 1200, one_message);
        let parts = reply_parts(&shaped);
        assert_eq!(parts.len() == 1, one_message);
        assert_eq!(parts.join("\n\n"), shaped);
        assert!(parts.iter().any(|part| part.contains(&code)));
    }
}

/// A long answer with no schedule is kept whole (v4 cut it at the bound).
#[test]
fn a_long_answer_is_not_cut() {
    let code = rust_fence(30);
    let reply = format!("Here:\n\n{code}\n\n{CLOSING}");
    assert_eq!(shape_reply(&reply, &[]), reply);
}

/// Three upcoming runs and more beyond (wholly invented fixture).
const UPCOMING: &str = "**Your 8 upcoming runs · All channels**\n\n`[c0ffee01]` **Hard Lotus**\n*Tue 06 Oct · 20:00* · `planned` · `0/4 yes` · <#4242>\n\n`[c0ffee02]` **Chaos Vellum**\n*Wed 07 Oct · 21:30* · `planned` · `2/4 yes` · <#4243>\n\n`[c0ffee03]` **Normal Magnus**\n*Thu 08 Oct · 19:00* · `planned` · `1/4 yes` · <#4244>\n\n*(and 5 more)*";
const LOTUS: &str =
    "`[c0ffee01]` **Hard Lotus**\n*Tue 06 Oct · 20:00* · `planned` · `0/4 yes` · <#4242>";
const VELLUM: &str =
    "`[c0ffee02]` **Chaos Vellum**\n*Wed 07 Oct · 21:30* · `planned` · `2/4 yes` · <#4243>";

/// `D-VOICED-CARD` (closed rule, user decision 2026-10-03): a line citing a
/// run that retells any fact (date, time, status, tally, channel, `first`)
/// is replaced by the card; a fact-free citing line in the same reply stays.
#[test]
fn a_citing_line_that_retells_facts_becomes_the_card() {
    let sentence = "Mama~ Your next run is **Hard Lotus** on *Tue 06 Oct at 20:00* — `planned`, `0/4 yes`, in <#4242>. `c0ffee01`—first on the board, so no hiding from the ready check!";
    let outcomes = [schedule_outcome(UPCOMING)];
    assert_eq!(ground_schedule_reply(sentence, &outcomes), LOTUS);
    let reply = format!("{sentence}\nBring potions.\n\nSee you at `c0ffee01`, ok?\n\nBye!");
    assert_eq!(
        ground_schedule_reply(&reply, &outcomes),
        format!("{LOTUS}\nBring potions.\n\nSee you at **Hard Lotus**, ok?\n\nBye!")
    );
    // A retold record and a retelling sentence both become cards, once each.
    let reply = "Your next run is Hard Lotus at 20:00 (`c0ffee01`)!\n\n`[c0ffee02]` **Chaos Vellum**\n*Wed 07 Oct · 22:00* · `planned`\n\nSee you!";
    assert_eq!(
        ground_schedule_reply(reply, &outcomes),
        format!("{LOTUS}\n\n{VELLUM}\n\nSee you!")
    );
}

/// Record-shaped and list-style retellings are still replaced whole.
#[test]
fn record_shaped_retellings_are_still_replaced() {
    let outcomes = [schedule_outcome(UPCOMING)];
    for retold in [
        "`[c0ffee02]` **Chaos Vellum** · 22:00",
        "Chaos Vellum - 22:00 - run ID 'c0ffee02' (2/4)",
    ] {
        let reply = format!("Tonight:\n\n{retold}\n\nBye!");
        assert_eq!(
            ground_schedule_reply(&reply, &outcomes),
            format!("Tonight:\n\n{VELLUM}\n\nBye!")
        );
    }
}

/// The live id-less shape, with an invented fixture.
const DATED_SENTENCE: &str = "Mama~ Your next run is **Hard Lotus** on *Tue 06 Oct at 20:00* · `planned` · `0/4 yes` · <#4242>. Be sure to check the card before queueing in.";

/// `D-GROUND-FILTERED` (user decision 2026-10-03, "fact-check prose in
/// place"): a sentence naming a run by its date and time, with every fact
/// it states matching, is kept and the record goes under it.
#[test]
fn a_sentence_naming_a_run_by_date_and_time_is_kept() {
    // `D-VOICED-CARD`: the sentence introduces the record; no heading.
    let next = format!("**Your next run · All channels**\n\n{LOTUS}");
    assert_eq!(
        ground_schedule_reply(DATED_SENTENCE, &[schedule_outcome(&next)]),
        format!("{DATED_SENTENCE}\n\n{LOTUS}")
    );
    assert_eq!(
        ground_schedule_reply(DATED_SENTENCE, &[schedule_outcome(UPCOMING)]),
        format!("{DATED_SENTENCE}\n\n{LOTUS}")
    );
    let shaped = shape_reply(DATED_SENTENCE, &[schedule_outcome(UPCOMING)]);
    assert!(shaped.starts_with("Mama~ Your next run is"), "{shaped}");
    assert!(shaped.ends_with(LOTUS), "{shaped}");
    // A unique time needs no date; a weekday alone is enough too.
    for sentence in [
        "Hard Lotus starts at 20:00, be on time!",
        "See you Tuesday at 20:00 for Lotus!",
    ] {
        assert_eq!(
            ground_schedule_reply(sentence, &[schedule_outcome(UPCOMING)]),
            format!("{sentence}\n\n{LOTUS}")
        );
    }
}

/// A time, date, tally or status that no listed run has falls back to the
/// listing, as does a list-style retelling that is not a sentence.
#[test]
fn an_unmatched_dated_sentence_falls_back_to_the_listing() {
    let outcomes = [schedule_outcome(UPCOMING)];
    for reply in [
        "Your next run is **Hard Lotus** on *Tue 06 Oct at 20:30*. Be ready!",
        "Your next run is **Hard Lotus** on *Wed 07 Oct at 20:00*. Be ready!",
        "Your next run is **Hard Lotus** at 20:00, `3/4 yes` so far. Be ready!",
        "Your next run is **Hard Lotus** at 20:00, already `confirmed`. Be ready!",
        "Your next run is **Hard Lotus** at 20:00 in <#4243>. Be ready!",
        "Hard Lotus - Tue 06 Oct - 20:00 - 0/4",
    ] {
        assert_eq!(ground_schedule_reply(reply, &outcomes), UPCOMING, "{reply}");
    }
    // One unchecked fact line keeps the whole reply on the fallback.
    let reply = format!("{DATED_SENTENCE}\n\nAlso Chaos Vellum at 23:00!");
    assert_eq!(ground_schedule_reply(&reply, &outcomes), UPCOMING);
}

/// Two runs at the same time on different days: the date picks one, and
/// without a date the sentence is ambiguous and falls back.
#[test]
fn the_date_resolves_runs_at_the_same_time() {
    let vellum =
        "`[c0ffee02]` **Chaos Vellum**\n*Thu 08 Oct · 20:00* · `planned` · `2/4 yes` · <#4243>";
    let listing =
        format!("**Your 2 upcoming runs this boss week · All channels**\n\n{LOTUS}\n\n{vellum}");
    let outcomes = [schedule_outcome(&listing)];
    let sentence = "Chaos Vellum is on Thu 08 Oct at 20:00 with `2/4 yes`.";
    assert_eq!(
        ground_schedule_reply(sentence, &outcomes),
        format!("{sentence}\n\n{vellum}")
    );
    assert_eq!(
        ground_schedule_reply("The run is at 20:00, see you!", &outcomes),
        listing
    );
}

/// Wrong facts never survive in prose: a boss, relative day, extra time or
/// status the picked run does not have falls back to the listing.
#[test]
fn a_dated_sentence_with_any_wrong_fact_falls_back() {
    let outcomes = [schedule_outcome(UPCOMING)];
    let mut kept = Vec::new();
    for reply in [
        // A boss the picked run is not (bold or plain).
        "Your next run is **Hard Lucid** on *Tue 06 Oct at 20:00*. Be ready!",
        "Your next run is Hard Lucid on Tue 06 Oct at 20:00. Be ready!",
        "Chaos Vellum is on Tue 06 Oct at 20:00. Be ready!",
        // A relative day grounding cannot check without a clock.
        "Hard Lotus is tonight at 20:00, don't be late!",
        "Hard Lotus is tomorrow at 20:00, don't be late!",
        "Hard Lotus is next Tuesday at 20:00, don't be late!",
        // A status the run does not have.
        "Your Lotus at 20:00 is done already.",
        "Your Lotus at 20:00 is at risk.",
        // A plain boss name the listing does not carry.
        "Your next run is Lucid on Tue 06 Oct at 20:00. Be ready!",
        // A tally in words the run does not have.
        "Hard Lotus is on Tue 06 Oct at 20:00, 3 of 4 have said yes.",
        // Fail-closed: a day number, date or capitalised alias left unread.
        "Hard Lotus is on the 7th at 20:00.",
        "Hard Lotus is on 2026-10-07 at 20:00.",
        "Karl is on Tue 06 Oct at 20:00.",
    ] {
        if ground_schedule_reply(reply, &outcomes) != UPCOMING {
            kept.push(reply);
        }
    }
    // More than one time cannot bind its facts to one run.
    let vellum =
        "`[c0ffee02]` **Chaos Vellum**\n*Thu 08 Oct · 21:00* · `planned` · `2/4 yes` · <#4243>";
    let listing = format!("**Your 2 upcoming runs · All channels**\n\n{LOTUS}\n\n{vellum}");
    let swapped =
        "Lotus is Thu 08 Oct at 20:00 and Vellum is Tue 06 Oct at 21:00, `2/4` and `0/4`.";
    if ground_schedule_reply(swapped, &[schedule_outcome(&listing)]) != listing {
        kept.push(swapped);
    }
    // A one-record "next run" listing: another catalog boss, in any case or
    // place, falls back.
    for other in [
        "Your next run is Kalos on Mon 05 Oct at 21:00.",
        "Kalos is on Mon 05 Oct at 21:00, 0 of 3 have said yes so far.",
        "Papa~ Kalos is on Mon 05 Oct at 21:00.",
        "Your next run: Kalos on Mon 05 Oct at 21:00.",
        "Heads up! Lucid starts Mon 05 Oct at 21:00.",
        "your next run is kalos on Mon 05 Oct at 21:00.",
        "The gatekeeper fight is on Mon 05 Oct at 21:00.",
        "Your next run is Hard Will on Mon 05 Oct at 21:00.",
        // Fail-closed: another difficulty, a count, a negation, an unread
        // status phrase or a channel name.
        "Your next run is Normal Carling on Mon 05 Oct at 21:00.",
        "Your next run is **Normal Carling** on Mon 05 Oct at 21:00.",
        "Your next run is ncarling on Mon 05 Oct at 21:00.",
        "Carling is on Mon 05 Oct at 21:00 and all 3 have said yes.",
        "Carling is on Mon 05 Oct at 21:00, on your own time.",
        "Carling is on Mon 05 Oct at 21:00 in #hard-runs.",
    ] {
        if ground_schedule_reply(other, &[schedule_outcome(&next_carling())]) != next_carling() {
            kept.push(other);
        }
    }
    let confirmed = next_carling().replace("`planned`", "`confirmed`");
    let negated = "Carling on Mon 05 Oct at 21:00 isn't confirmed yet.";
    if ground_schedule_reply(negated, &[schedule_outcome(&confirmed)]) != confirmed {
        kept.push(negated);
    }
    let morning = next_carling().replace("21:00", "09:00");
    let evening = "Carling is on Mon 05 Oct at 9:00 pm.";
    if ground_schedule_reply(evening, &[schedule_outcome(&morning)]) != morning {
        kept.push(evening);
    }
    assert!(kept.is_empty(), "kept wrong facts: {kept:#?}");
}

/// A one-record "next run" listing (invented fixture).
fn next_carling() -> String {
    format!("**Your next run · All channels**\n\n{CARLING}")
}

const CARLING: &str =
    "`[c0ffee05]` **Hard Carling**\n*Mon 05 Oct · 21:00* · `planned` · `0/3 yes` · <#4245>";

/// The run's own plain name, or a matching tally in words, is still kept.
#[test]
fn a_dated_sentence_with_the_right_plain_name_is_kept() {
    let outcomes = [schedule_outcome(&next_carling())];
    let mut lost = Vec::new();
    for sentence in [
        "Your next run is Carling on Mon 05 Oct at 21:00.",
        "Papa~ Your next run is **Hard Carling** on *Mon 05 Oct at 21:00* · `planned` · `0/3 yes` · <#4245>. Be sure to check the card before queueing in.",
        "Carling is on Mon 05 Oct at 21:00, 0 of 3 have said yes so far.",
        // Possessives, member/persona names, timezones and ordinary words.
        "Carling's party is on Mon 05 Oct at 21:00.",
        "Check the card, Papa. Carling is Mon 05 Oct at 21:00.",
        "Alvin will lead Carling on Mon 05 Oct at 21:00 MYT, it's a lot of fun!",
        // An alias of the run's boss.
        "Your next run is Karling on Mon 05 Oct at 21:00.",
        "Your next run is **Hard Kaling** on Mon 05 Oct at 21:00.",
        // The run's difficulty as a letter or `hm`-style shorthand.
        "Your next run is H Carling on Mon 05 Oct at 21:00.",
        "Your next run is HM Carling on Mon 05 Oct at 21:00.",
        // An ordinal that cannot be a date is flavour (live 2026-10-03 shape).
        "Papa~ Your next run is **Hard Carling** on *Mon 05 Oct · 21:00* — `planned`, `0/3 yes`, in <#4245>. The lobby’s waiting on its first ✅.",
        "Carling on Mon 05 Oct at 21:00 is your first run, Papa.",
    ] {
        if ground_schedule_reply(sentence, &outcomes) != format!("{sentence}\n\n{CARLING}") {
            lost.push(sentence);
        }
    }
    // A multi-word name and a `+`-joined label resolve through the catalog.
    let pair = "**Your next run · All channels**\n\n`[c0ffee07]` **Hard MaleficStar + Hard FA**\n*Wed 07 Oct · 22:00* · `planned` · `1/4 yes` · <#4246>";
    for sentence in [
        "Hard Radiant Malefic Star is on Wed 07 Oct at 22:00.",
        "Your Hard Star and FA run is on Wed 07 Oct at 22:00, 1 of 4 said yes.",
    ] {
        let record = pair.split_once("\n\n").expect("heading").1;
        if ground_schedule_reply(sentence, &[schedule_outcome(pair)])
            != format!("{sentence}\n\n{record}")
        {
            lost.push(sentence);
        }
    }
    // A hyphenated status that matches the run.
    for (status, sentence) in [
        ("at_risk", "Carling on Mon 05 Oct at 21:00 is at-risk."),
        (
            "otot",
            "Carling on Mon 05 Oct at 21:00 is on your own-time.",
        ),
    ] {
        let listing = next_carling().replace("`planned`", &format!("`{status}`"));
        let record = CARLING.replace("`planned`", &format!("`{status}`"));
        if ground_schedule_reply(sentence, &[schedule_outcome(&listing)])
            != format!("{sentence}\n\n{record}")
        {
            lost.push(sentence);
        }
    }
    assert!(lost.is_empty(), "lost right sentences: {lost:#?}");
}

/// Fail-closed: shorthand difficulties, tallies and statuses in words,
/// spelled-out dates and non-ASCII digits are facts too.
#[test]
fn a_dated_sentence_with_facts_in_words_falls_back() {
    let mut kept = Vec::new();
    let mut check = |listing: &str, sentence: &'static str| {
        if ground_schedule_reply(sentence, &[schedule_outcome(listing)]) != listing {
            kept.push(sentence);
        }
    };
    for sentence in [
        "Your next run is N Carling on Mon 05 Oct at 21:00.",
        "Your next run is N-Carling on Mon 05 Oct at 21:00.",
        "Your next run is NM Carling on Mon 05 Oct at 21:00.",
        "Carling is on Mon 05 Oct at 21:00 and everyone has said yes.",
        "Carling is on Mon 05 Oct at 21:00 and all three said yes.",
        "Carling is on Mon 05 Oct at 21:00 and nobody has answered.",
        "Carling is on Mon 05 Oct at 21:00 and no one has answered.",
        "Carling is on Mon 05 Oct at 21:00 and one of 3 said yes.",
        "Carling is on Mon 05 Oct at 21:00 and you said no.",
        "Carling is on Mon 05 Oct at 21:00, no answer from Alvin yet.",
        "Carling is on Mon 05 Oct at 21:00 and Alvin said yes.",
        "Carling on Mon 05 Oct at 21:00 was called off.",
        "Carling on Mon 05 Oct at 21:00 is finished.",
        "Carling on Mon 05 Oct at 21:00 was postponed.",
    ] {
        check(&next_carling(), sentence);
    }
    let confirmed = next_carling().replace("`planned`", "`confirmed`");
    check(
        &confirmed,
        "Carling on Mon 05 Oct at 21:00 is still unconfirmed.",
    );
    for sentence in [
        "Hard Lotus is on Tue 06 Oct at 20:00 and is at-risk.",
        "Hard Lotus is on October seventh at 20:00.",
        "Hard Lotus is on the seventh at 20:00.",
        "Hard Lotus is on the seventh of October at 20:00.",
        "Hard Lotus is on seventh Oct at 20:00.",
        "Hard Lotus on ０７ Oct at 20:00.",
    ] {
        check(UPCOMING, sentence);
    }
    assert!(kept.is_empty(), "kept wrong facts: {kept:#?}");
}

/// Known gap (documented in `D-GROUND-FILTERED`): a lowercase everyday-word
/// alias is not read as a boss, so this wrong boss is kept.
#[test]
fn a_lowercase_everyday_alias_is_a_documented_gap() {
    let sentence = "star is on tue 06 oct at 20:00.";
    assert_eq!(
        ground_schedule_reply(sentence, &[schedule_outcome(UPCOMING)]),
        format!("{sentence}\n\n{LOTUS}")
    );
}

/// A personal two-run listing with model-only context lines (invented).
const MINE_CARLING: &str =
    "`[b0a7c0de]` **Hard Carling**\n*Mon 05 Oct · 21:00* · `planned` · `0/3 yes` · <#4245>";
const MINE_VELLUM: &str =
    "`[f00dfeed]` **Chaos Vellum**\n*Tue 06 Oct · 22:00* · `planned` · `1/2 yes` · <#4246>";
const CARLING_CONTEXT: &str = "Context (hidden from members): in 2 days · you haven't answered · no answer yet from Rook and Wren";
const VELLUM_CONTEXT: &str = "Context (hidden from members): in 3 days · you said yes";

fn mine_next() -> String {
    format!("**Your next run · All channels**\n\n{MINE_CARLING}\n{CARLING_CONTEXT}")
}

fn mine_both() -> String {
    format!(
        "**Your 2 upcoming runs this boss week · All channels**\n\n{MINE_CARLING}\n{CARLING_CONTEXT}\n\n{MINE_VELLUM}\n{VELLUM_CONTEXT}"
    )
}

/// The member view of [`mine_both`]: no context lines.
fn mine_both_seen() -> String {
    format!(
        "**Your 2 upcoming runs this boss week · All channels**\n\n{MINE_CARLING}\n\n{MINE_VELLUM}"
    )
}

/// `D-VOICED-CARD` (closed rule, user decision 2026-10-03): a citing line
/// that states no fact beyond its ids and the cited run's exact context
/// phrases is kept, each id read as the run's label, with the card under
/// its paragraph and no listing heading. Every id form the model emits is
/// read.
#[test]
fn a_fact_free_citing_line_is_kept_with_the_card_below() {
    let outcomes = [schedule_outcome(&mine_next())];
    for id in [
        "[b0a7c0de]",
        "`b0a7c0de`",
        "b0a7c0de",
        "`[b0a7c0de]`",
        "[B0A7C0DE]",
    ] {
        let reply = format!(
            "Papa~ your next one is {id}, in 2 days and you haven't answered yet, don't leave them waiting!"
        );
        assert_eq!(
            shape_reply(&reply, &outcomes),
            format!(
                "Papa~ your next one is **Hard Carling**, in 2 days and you haven't answered yet, don't leave them waiting!\n\n{MINE_CARLING}"
            ),
            "{id}"
        );
    }
    for (reply, shown) in [
        (
            "[b0a7c0de] is waiting for you, Papa~",
            "**Hard Carling** is waiting for you, Papa~",
        ),
        (
            "Still no answer yet from Rook and Wren for [b0a7c0de] 🎉",
            "Still no answer yet from Rook and Wren for **Hard Carling** 🎉",
        ),
        // An id beside its label is dropped rather than doubled.
        (
            "Your next one is **Hard Carling** `[b0a7c0de]`, see you there!",
            "Your next one is **Hard Carling**, see you there!",
        ),
        (
            "`[b0a7c0de]` **Hard Carling** is calling, Papa!",
            "**Hard Carling** is calling, Papa!",
        ),
    ] {
        assert_eq!(
            shape_reply(reply, &outcomes),
            format!("{shown}\n\n{MINE_CARLING}"),
            "{reply}"
        );
    }
}

/// A citing line stating any fact is replaced by the cited run's card: a
/// retold date, time, status, tally or channel (even when correct), a
/// weekday, relative or comparison word, an answer word, a mention, or a
/// context phrase that is not verbatim or belongs to another run.
#[test]
fn a_citing_line_stating_any_fact_becomes_the_card() {
    let outcomes = [schedule_outcome(&mine_both())];
    let mut kept = Vec::new();
    for reply in [
        "Papa~ Your next run is **Hard Carling** on *Mon 05 Oct at 21:00* — `b0a7c0de`, `planned`, `0/3 yes`, in <#4245>.",
        "Papa~ [b0a7c0de] is at 21:00!",
        "Papa~ [b0a7c0de] is on Monday!",
        "Papa~ [b0a7c0de] is tomorrow!",
        "Papa~ [b0a7c0de] is tonight~",
        "Papa~ [b0a7c0de] starts in an hour!",
        "Papa~ [b0a7c0de] is in a few days!",
        "Papa~ [b0a7c0de] is `confirmed`!",
        "Papa~ [b0a7c0de] is done already!",
        "Papa~ [b0a7c0de] is in #hard-runs!",
        "Papa~ [b0a7c0de] is with @Rook!",
        "Papa~ you're a maybe for [b0a7c0de]!",
        "Papa~ you said yes to [b0a7c0de]!",
        // Another run's phrase, or a variant of the run's own.
        "Papa~ [b0a7c0de] is in 3 days!",
        "Papa~ you haven't RSVP'd to [b0a7c0de]!",
        "Papa~ nobody answered [b0a7c0de] yet!",
        // Review input: a record-head line has no exemption.
        "`[f00dfeed]` **Chaos Vellum** is on Sunday, you said no!",
        // Review input: a comparison between two runs.
        "[f00dfeed] is at 22:00, same time as [b0a7c0de]!",
    ] {
        let shaped = shape_reply(reply, &outcomes);
        let only_cards = shaped
            .split("\n\n")
            .all(|part| [MINE_CARLING, MINE_VELLUM].contains(&part) || part.starts_with("**Your"));
        if !only_cards {
            kept.push((reply, shaped));
        }
    }
    assert!(kept.is_empty(), "kept a fact: {kept:#?}");
}

/// Review input: an id-less fact line beside a kept citing line is dropped
/// in favour of the card already shown.
#[test]
fn an_id_less_fact_line_beside_a_kept_citing_line_is_dropped() {
    let outcomes = [schedule_outcome(&mine_both())];
    for stray in [
        "Chaos Vellum is tomorrow and you said no.",
        "It starts at 23:00 in <#4246>.",
    ] {
        let reply = format!("Papa~ your next one is [b0a7c0de]!\n{stray}\n\nSee you~");
        assert_eq!(
            shape_reply(&reply, &outcomes),
            format!("Papa~ your next one is **Hard Carling**!\n\n{MINE_CARLING}\n\nSee you~"),
            "{stray}"
        );
    }
}

/// Two citing lines in their own paragraphs: each card goes under its own.
#[test]
fn a_reply_citing_two_runs_places_both_records() {
    let outcomes = [schedule_outcome(&mine_both())];
    let reply =
        "Papa~ [b0a7c0de] is in 2 days!\n\nAnd [f00dfeed] is in 3 days, you said yes.\n\nSee you~";
    assert_eq!(
        shape_reply(reply, &outcomes),
        format!(
            "Papa~ **Hard Carling** is in 2 days!\n\n{MINE_CARLING}\n\nAnd **Chaos Vellum** is in 3 days, you said yes.\n\n{MINE_VELLUM}\n\nSee you~"
        )
    );
}

/// Known gap (documented in `D-VOICED-CARD`): a line citing several runs may
/// use any of their phrases, so a phrase can attach to the wrong one; a boss
/// name is not a fact under the closed rule.
#[test]
fn shared_phrases_and_boss_names_are_documented_gaps() {
    let outcomes = [schedule_outcome(&mine_both())];
    assert_eq!(
        shape_reply(
            "Papa~ [b0a7c0de] is in 3 days and [f00dfeed] in 2 days!",
            &outcomes
        ),
        format!(
            "Papa~ **Hard Carling** is in 3 days and **Chaos Vellum** in 2 days!\n\n{MINE_CARLING}\n\n{MINE_VELLUM}"
        )
    );
    assert_eq!(
        shape_reply("Papa~ [b0a7c0de] is a Kalos run!", &outcomes),
        format!("Papa~ **Hard Carling** is a Kalos run!\n\n{MINE_CARLING}")
    );
}

/// `D-PERSONAL-CONTEXT`: no copy of the context line reaches a member: the
/// label in any dress, its phrases as bullets after it (with or without a
/// blank line), under a plain `Context:` heading, inline with separators,
/// inside a code fence, or the whole tool output copied.
#[test]
fn the_context_line_never_reaches_the_reply() {
    let outcomes = [schedule_outcome(&mine_both())];
    let voiced = format!("Papa~ **Hard Carling** is in 2 days!\n\n{MINE_CARLING}");
    for reply in [
        format!("Papa~ [b0a7c0de] is in 2 days!\n{CARLING_CONTEXT}"),
        "Papa~ [b0a7c0de] is in 2 days!\n**Context (hidden from members)**: in 2 days · you haven't answered".to_owned(),
        "Papa~ [b0a7c0de] is in 2 days!\n[Context: HIDDEN FROM MEMBERS]".to_owned(),
        // Review input: bullets after the label and a blank line.
        "Papa~ [b0a7c0de] is in 2 days!\n\nContext (hidden from members):\n\n- in 2 days\n- you haven't answered\n- no answer yet from Rook and Wren".to_owned(),
        // Review input: bullets under a plain heading.
        "Papa~ [b0a7c0de] is in 2 days!\n\nContext:\n- in 3 days\n- you said yes".to_owned(),
        "Papa~ [b0a7c0de] is in 2 days!\nin 2 days · you haven't answered · no answer yet from Rook and Wren".to_owned(),
    ] {
        let shaped = shape_reply(&reply, &outcomes);
        assert_eq!(shaped, voiced, "{reply:?}");
    }
    // Inside a code fence the line goes too (the empty fence stays).
    let fenced = shape_reply(
        "Papa~ [b0a7c0de] is in 2 days!\n```\nContext (hidden from members): in 2 days\n```",
        &outcomes,
    );
    assert_eq!(fenced, format!("{voiced}\n\n```\n```"));
    // Inline with separators: the line goes, so the reply falls back.
    let inline =
        "Papa~ [b0a7c0de] is in 2 days · you haven't answered · no answer yet from Rook and Wren!";
    assert_eq!(shape_reply(inline, &outcomes), mine_both_seen());
    // Record-only replies are grounded as before, without context.
    assert_eq!(shape_reply(&mine_both(), &outcomes), mine_both_seen());
    let retold = format!("{MINE_CARLING}\n\n{MINE_VELLUM}");
    assert_eq!(shape_reply(&retold, &outcomes), mine_both_seen());
    // Without any outcome (a card-posting turn), the vocabulary alone strips.
    assert_eq!(
        sanitize::strip_context_copies(
            "The card is up!\nin 2 days · you haven't answered\n- tonight\nContext (hidden from members): in 2 days",
            &[]
        ),
        "The card is up!"
    );
}
