//! `sanitize.json`: note defusing, member-facing rewrites, false-claim
//! stripping, trusted defaults, tidy bounds and schedule regrounding.

use kanade::chat::sanitize::{
    defuse_notes, ground_schedule_reply, looks_like_clarification, member_facing, reply_parts,
    schedule_defaults, shape_reply, strip_false_card_claim, tidy,
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
