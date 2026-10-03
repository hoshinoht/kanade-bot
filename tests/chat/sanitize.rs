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
    vec![Named {
        name: "D-GROUND-FILTERED",
        entries: vec![dev(
            "schedule-grounding",
            "/steps/8/value",
            json!(
                "**2 runs this week · All channels**\n\n`[9004eab0]` **Hard MaleficStar**\n*Tue 08 Sep · 00:00* · `planned` · `2/3 yes`\n\n`[9004eab1]` **Extreme Kalos**\n*Wed 09 Sep · 00:00* · `planned` · `2/3 yes`"
            ),
            json!("`[9004eab1]` **Extreme Kalos**\n*Wed 09 Sep · 00:00* · `planned` · `2/3 yes`"),
        )],
    }]
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

/// `D-GROUND-FILTERED` (user decision 2026-10-03): a persona sentence that
/// names a run stays as written and the run's record goes under it (the
/// live case replaced the whole sentence with the record).
#[test]
fn a_sentence_naming_a_run_keeps_its_words_with_the_record_below() {
    let sentence = "Mama~ Your next run is **Hard Lotus** on *Tue 06 Oct at 20:00* — `planned`, `0/4 yes`, in <#4242>. `c0ffee01`—first on the board, so no hiding from the ready check!";
    let outcomes = [schedule_outcome(UPCOMING)];
    assert_eq!(
        ground_schedule_reply(sentence, &outcomes),
        format!("{sentence}\n\n{LOTUS}")
    );
    // The card goes after the sentence's paragraph, once, however many
    // sentences name the run.
    let reply = format!("{sentence}\nBring potions.\n\nAgain: `c0ffee01` at 20:00, ok?\n\nBye!");
    assert_eq!(
        ground_schedule_reply(&reply, &outcomes),
        format!("{sentence}\nBring potions.\n\n{LOTUS}\n\nAgain: `c0ffee01` at 20:00, ok?\n\nBye!")
    );
    let shaped = shape_reply(sentence, &outcomes);
    assert!(shaped.starts_with("Mama~ Your next run is"), "{shaped}");
    assert!(
        shaped.ends_with("*Tue 06 Oct · 20:00* · `planned` · `0/4 yes` · <#4242>"),
        "{shaped}"
    );
}

/// A sentence and a record-shaped retelling in one reply: the sentence
/// stays, the retold record is replaced, and each record shows once.
#[test]
fn a_sentence_and_a_retold_record_share_one_card() {
    let sentence = "Your next run is Hard Lotus at 20:00 (`c0ffee01`)!";
    let reply = format!(
        "{sentence}\n\n`[c0ffee02]` **Chaos Vellum**\n*Wed 07 Oct · 22:00* · `planned`\n\nSee you!"
    );
    assert_eq!(
        ground_schedule_reply(&reply, &[schedule_outcome(UPCOMING)]),
        format!("{sentence}\n\n{LOTUS}\n\n{VELLUM}\n\nSee you!")
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
    let next = format!("**Your next run · All channels**\n\n{LOTUS}");
    assert_eq!(
        ground_schedule_reply(DATED_SENTENCE, &[schedule_outcome(&next)]),
        format!("{DATED_SENTENCE}\n\n{next}")
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
    ] {
        if ground_schedule_reply(sentence, &outcomes) != format!("{sentence}\n\n{}", next_carling())
        {
            lost.push(sentence);
        }
    }
    // A multi-word name and a `+`-joined label resolve through the catalog.
    let pair = "**Your next run · All channels**\n\n`[c0ffee07]` **Hard MaleficStar + Hard FA**\n*Wed 07 Oct · 22:00* · `planned` · `1/4 yes` · <#4246>";
    for sentence in [
        "Hard Radiant Malefic Star is on Wed 07 Oct at 22:00.",
        "Your Hard Star and FA run is on Wed 07 Oct at 22:00, 1 of 4 said yes.",
    ] {
        if ground_schedule_reply(sentence, &[schedule_outcome(pair)])
            != format!("{sentence}\n\n{pair}")
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
        if ground_schedule_reply(sentence, &[schedule_outcome(&listing)])
            != format!("{sentence}\n\n{listing}")
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
