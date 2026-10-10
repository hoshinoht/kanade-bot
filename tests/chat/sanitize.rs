//! `sanitize.json`: note defusing, member-facing rewrites, false-claim
//! stripping, trusted defaults, tidy bounds and schedule regrounding.

use kanade::chat::sanitize::{
    self, defuse_notes, ground_schedule_reply, looks_like_clarification, member_facing,
    reply_parts, schedule_defaults, shape_reply, strip_false_card_claim, tidy,
};
use kanade::chat::tools::ToolOutcome;
use serde_json::{Map, Value, json};

use crate::common::text;
use crate::support::{Named, check_family, dev, unknown_op, value};

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

const MALEFIC: &str =
    "`[9004eab0]` **Hard MaleficStar**\n*Tue 08 Sep · 00:00* · `planned` · `2/3 yes`";
const KALOS_09: &str =
    "`[9004eab1]` **Extreme Kalos**\n*Wed 09 Sep · 00:00* · `planned` · `2/3 yes`";

fn two_runs() -> String {
    format!("**2 runs this week · All channels**\n\n{MALEFIC}\n\n{KALOS_09}")
}

/// `D-GROUND-FILTERED`: a reply naming one of two listed runs gets that
/// run's record, not the whole listing; a reply naming none keeps its
/// wording with the listing after it (user decision 2026-10-03).
fn named() -> Vec<Named> {
    vec![
        Named {
            name: "D-GROUND-FILTERED",
            entries: vec![
                dev(
                    "schedule-grounding",
                    "/steps/6/value",
                    json!(two_runs()),
                    json!(format!("Nothing about ids here.\n\n{}", two_runs())),
                ),
                dev(
                    "schedule-grounding",
                    "/steps/8/value",
                    json!(two_runs()),
                    json!(format!(
                        "Latest listing wins: **Extreme Kalos** at 00:00\n\n{KALOS_09}"
                    )),
                ),
            ],
        },
        voiced_card(),
    ]
}

/// `D-VOICED-CARD` (user decision 2026-10-03): a line citing a listed id is
/// kept with the id read as the run's label, and records placed under it
/// come without the listing heading; a retold record of the same run is
/// not shown twice.
fn voiced_card() -> Named {
    Named {
        name: "D-VOICED-CARD",
        entries: vec![
            dev(
                "schedule-grounding",
                "/steps/3/value",
                json!(format!(
                    "I saved 9004eab0 for later.\n\n**1 run this week · All channels**\n\n{MALEFIC}\n\n9004eab0 is still the reference for the card."
                )),
                json!(format!(
                    "I saved **Hard MaleficStar** for later.\n\n{MALEFIC}\n\n**Hard MaleficStar** is still the reference for the card."
                )),
            ),
            dev(
                "schedule-grounding",
                "/steps/4/value",
                json!(format!("{}\n\nKeep 9004eab0 handy.", two_runs())),
                json!(format!(
                    "{}\n\nKeep **Hard MaleficStar** handy.",
                    two_runs()
                )),
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

/// `D-GROUND-FILTERED` (user decision 2026-10-03, "keep wording, card
/// under"): the live shape, with invented ids, is kept word for word, a
/// curly apostrophe and the trailing channel mention included, with the
/// run's card under it and no listing heading.
#[test]
fn the_live_shape_is_kept_word_for_word_with_the_card_under() {
    let outcomes = [schedule_outcome(&mine_next())];
    let reply = "Papa~ Your next run is **Hard Carling** on *Mon 05 Oct · 21:00* — `planned`, `0/3 yes`. You haven’t answered yet; go claim your spot with ✅. <#4245>";
    assert_eq!(
        shape_reply(reply, &outcomes),
        format!("{reply}\n\n{MINE_CARLING}")
    );
    // The mention on its own line stays in the paragraph above the card.
    let split = reply.replace(" <#4245>", "\n<#4245>");
    assert_eq!(
        shape_reply(&split, &outcomes),
        format!("{split}\n\n{MINE_CARLING}")
    );
}

/// Accepted gap (documented in `D-GROUND-FILTERED`): only times, dates and
/// ids are checked, so a kept line naming the run by its time may carry a
/// wrong count, status, boss or member name above the correct card.
#[test]
fn a_wrong_count_status_or_boss_beside_the_right_time_is_an_accepted_gap() {
    let outcomes = [schedule_outcome(&mine_next())];
    for reply in [
        "Your next run is **Hard Carling** on Mon 05 Oct at 21:00, `3/3 yes` already!",
        "Your next run is **Hard Carling** at 21:00 and it's `confirmed`.",
        "Your next run is **Hard Lucid** on Monday at 21:00.",
        "Carling at 21:00 Monday, Alvin said yes!",
        "Papa~ [b0a7c0de] is a Kalos run!",
    ] {
        let shown = reply.replace("[b0a7c0de]", "**Hard Carling**");
        assert_eq!(
            shape_reply(reply, &outcomes),
            format!("{shown}\n\n{MINE_CARLING}"),
            "{reply}"
        );
    }
}

/// A time no listed run has replaces the line: by the cards of the runs it
/// names, or by the full listing when it names none.
#[test]
fn a_line_with_a_time_no_listed_run_has_is_replaced() {
    let outcomes = [schedule_outcome(UPCOMING)];
    let wrong = "Your next run is **Hard Lotus** on *Tue 06 Oct at 20:30*. Be ready!";
    assert_eq!(ground_schedule_reply(wrong, &outcomes), UPCOMING);
    assert_eq!(
        ground_schedule_reply(&format!("Hi Papa!\n\n{wrong}\n\nSee you~"), &outcomes),
        format!("Hi Papa!\n\n{UPCOMING}\n\nSee you~")
    );
    // A cited run with a wrong time: its card instead of the line.
    assert_eq!(
        ground_schedule_reply("Hi!\n\n[c0ffee01] is at 20:30!\n\nBye!", &outcomes),
        format!("Hi!\n\n{LOTUS}\n\nBye!")
    );
}

/// A date no listed run has replaces the line too.
#[test]
fn a_line_with_a_date_no_listed_run_has_is_replaced() {
    let outcomes = [schedule_outcome(UPCOMING)];
    for reply in [
        "Hard Lotus is on 09 Oct at 20:00, be ready!",
        "Hard Lotus is on Oct 9th, be ready!",
    ] {
        assert_eq!(ground_schedule_reply(reply, &outcomes), UPCOMING, "{reply}");
    }
}

/// An id no tool returned replaces the line, whether record-shaped or
/// cited in a sentence.
#[test]
fn an_unlisted_id_is_replaced() {
    let outcomes = [schedule_outcome(UPCOMING)];
    for reply in [
        "Your next run is `[deadbeef]`, see you!",
        "Your next run is `deadbeef`!",
    ] {
        assert_eq!(ground_schedule_reply(reply, &outcomes), UPCOMING, "{reply}");
    }
    let reply = "Tonight:\n\n`[deadbeef]` **Hard Seren**\n*Tue 06 Oct · 20:00*\n\nBye!";
    assert_eq!(
        ground_schedule_reply(reply, &outcomes),
        format!("Tonight:\n\n{UPCOMING}\n\nBye!")
    );
}

/// `D-VOICED-CARD`: a line citing a listed run in any id form is kept with
/// the id read as the run's label (dropped beside the label), its card
/// under its paragraph and no listing heading.
#[test]
fn a_citing_line_reads_the_id_as_the_label_with_the_card_below() {
    let outcomes = [schedule_outcome(&mine_next())];
    for id in [
        "[b0a7c0de]",
        "`b0a7c0de`",
        "b0a7c0de",
        "`[b0a7c0de]`",
        "[B0A7C0DE]",
    ] {
        let reply = format!(
            "Papa~ your next one is {id}, in 2 days and you haven’t answered yet, don't leave them waiting!"
        );
        assert_eq!(
            shape_reply(&reply, &outcomes),
            format!(
                "Papa~ your next one is **Hard Carling**, in 2 days and you haven’t answered yet, don't leave them waiting!\n\n{MINE_CARLING}"
            ),
            "{id}"
        );
    }
    for (reply, shown) in [
        (
            "[b0a7c0de] is at 21:00 on Monday, Papa~",
            "**Hard Carling** is at 21:00 on Monday, Papa~",
        ),
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

/// Two runs named in their own paragraphs, by id or by time: each card
/// goes under its own paragraph.
#[test]
fn a_two_run_reply_places_each_card_after_its_own_paragraph() {
    let outcomes = [schedule_outcome(&mine_both())];
    let reply =
        "Papa~ [b0a7c0de] is in 2 days!\n\nAnd [f00dfeed] is in 3 days, you said yes.\n\nSee you~";
    assert_eq!(
        shape_reply(reply, &outcomes),
        format!(
            "Papa~ **Hard Carling** is in 2 days!\n\n{MINE_CARLING}\n\nAnd **Chaos Vellum** is in 3 days, you said yes.\n\n{MINE_VELLUM}\n\nSee you~"
        )
    );
    let reply =
        "Carling is Monday at 21:00!\nBring potions.\n\nVellum is Tuesday at 22:00.\n\nSee you~";
    assert_eq!(
        shape_reply(reply, &outcomes),
        format!(
            "Carling is Monday at 21:00!\nBring potions.\n\n{MINE_CARLING}\n\nVellum is Tuesday at 22:00.\n\n{MINE_VELLUM}\n\nSee you~"
        )
    );
}

/// Two runs at the same time on different days: the date picks one, and
/// without one the line names neither, so the listing goes after it.
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
    let vague = "The run is at 20:00, see you!";
    assert_eq!(
        ground_schedule_reply(vague, &outcomes),
        format!("{vague}\n\n{listing}")
    );
}

/// Record-shaped retellings are replaced by the canonical record, and a
/// kept line naming the same run never shows its card twice.
#[test]
fn record_shaped_retellings_are_still_replaced() {
    let outcomes = [schedule_outcome(UPCOMING)];
    for retold in [
        "`[c0ffee02]` **Chaos Vellum** · 22:00",
        "`[c0ffee02]` **Chaos Vellum** · 21:30 · `2/4 yes`",
        "`[c0ffee02]` **Chaos Vellum**\n*Wed 07 Oct · 21:30* · `planned`",
        "Chaos Vellum - 22:00 - run ID 'c0ffee02' (2/4)",
        "Chaos Vellum - 21:30 - run ID 'c0ffee02' (2/4)",
    ] {
        let reply = format!("Tonight:\n\n{retold}\n\nBye!");
        assert_eq!(
            ground_schedule_reply(&reply, &outcomes),
            format!("Tonight:\n\n{VELLUM}\n\nBye!"),
            "{retold}"
        );
    }
    let outcomes = [schedule_outcome(&mine_next())];
    let reply = format!("Papa~ your next one is [b0a7c0de]!\n\n{MINE_CARLING}\n\nSee you~");
    assert_eq!(
        shape_reply(&reply, &outcomes),
        format!("Papa~ your next one is **Hard Carling**!\n\n{MINE_CARLING}\n\nSee you~")
    );
}

/// A reply naming no run and stating no hard fact keeps its wording, with
/// the full listing after it (v4 replaced the prose between hint lines).
#[test]
fn a_reply_naming_no_run_gets_the_listing_after_it() {
    let outcomes = [schedule_outcome(UPCOMING)];
    for reply in [
        "Here's your week, Papa~ Don't skip anything!",
        "Ara~ busy week!\n\nYou said yes to most of them.\n\nSee you there~",
    ] {
        assert_eq!(
            ground_schedule_reply(reply, &outcomes),
            format!("{reply}\n\n{UPCOMING}"),
            "{reply}"
        );
    }
}

/// `D-PERSONAL-CONTEXT`: no copy of the context line reaches a member: the
/// label in any dress, its phrases as bullets after it (with or without a
/// blank line), under a plain `Context:` heading, inline with separators
/// (straight or curly apostrophes), inside a code fence, or the whole tool
/// output copied.
#[test]
fn the_context_line_never_reaches_the_reply() {
    let outcomes = [schedule_outcome(&mine_both())];
    let voiced = format!("Papa~ **Hard Carling** is in 2 days!\n\n{MINE_CARLING}");
    for reply in [
        format!("Papa~ [b0a7c0de] is in 2 days!\n{CARLING_CONTEXT}"),
        "Papa~ [b0a7c0de] is in 2 days!\n**Context (hidden from members)**: in 2 days · you haven't answered".to_owned(),
        "Papa~ [b0a7c0de] is in 2 days!\n[Context: HIDDEN FROM MEMBERS]".to_owned(),
        "Papa~ [b0a7c0de] is in 2 days!\n\nContext (hidden from members):\n\n- in 2 days\n- you haven't answered\n- no answer yet from Rook and Wren".to_owned(),
        "Papa~ [b0a7c0de] is in 2 days!\n\nContext:\n- in 3 days\n- you said yes".to_owned(),
        "Papa~ [b0a7c0de] is in 2 days!\nin 2 days · you haven't answered · no answer yet from Rook and Wren".to_owned(),
        "Papa~ [b0a7c0de] is in 2 days!\nin 2 days · you haven’t answered · no answer yet from Rook and Wren".to_owned(),
        "Papa~ [b0a7c0de] is in 2 days!\nYou haven’t answered.".to_owned(),
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
    // Inline with separators: the line goes, leaving only the listing.
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
            "The card is up!\nin 2 days · you haven’t answered\n- tonight\nContext (hidden from members): in 2 days",
            &[]
        ),
        "The card is up!"
    );
}
