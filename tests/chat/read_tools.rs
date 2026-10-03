//! `read_tools.json`: every read tool and the dispatcher boundary, through
//! `dispatch::run` in the full-set (v4) mode with a passthrough identity
//! session.

use kanade::chat::sanitize::schedule_defaults;
use kanade::chat::tools::ToolOutcome;
use kanade::infrastructure::llm::identity::PassthroughSession;
use serde_json::{Value, json};

use crate::common::{instant, text};
use crate::support::{check_family, load, unknown_op, value};
use crate::world::World;

/// v4 `host.outcome`; `posted` is the cards the call handed over for posting.
pub fn outcome_json(outcome: &ToolOutcome) -> Value {
    json!({
        "name": outcome.name,
        "output": outcome.output,
        "arguments": outcome.arguments,
        "ok": outcome.ok,
        "error": outcome.error,
        "created": outcome.created,
        "posted": outcome.cards.iter().map(|card| card.proposal_id.as_str()).collect::<Vec<_>>(),
    })
}

async fn replay(case: Value) -> Vec<Value> {
    let input = &case["input"];
    let mut world = World::new(input).await;
    let mut session = PassthroughSession;
    let mut out = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        out.push(match text(&step["op"]) {
            "run" => value(outcome_json(&world.run_tool(step, &mut session).await)),
            "set_clock" => {
                world.clock.set(instant(&step["clock"]));
                value(step["clock"].clone())
            }
            other => unknown_op("read_tools", other),
        });
    }
    out
}

#[tokio::test]
async fn the_read_tools_family_replays_exactly() {
    assert_eq!(check_family("read_tools", &[], replay).await, (7, 74));
}

#[tokio::test]
async fn an_explicit_self_schedule_recovers_only_unrecognized_model_mentions() {
    let vectors = load("read_tools.json");
    let fixture = vectors["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["case_id"] == "schedule-scope-and-people")
        .unwrap();
    let mut world = World::new(&fixture["input"]).await;
    let mut session = PassthroughSession;
    let step = |participant: Option<&str>, source: &str, group: bool| {
        let mut arguments = json!({"week": "this"});
        if let Some(participant) = participant {
            arguments["participant"] = json!(participant);
        }
        json!({
            "op": "run", "tool": "get_schedule", "author_id": "22", "channel_id": "700",
            "arguments": arguments,
            "self_schedule_requested": schedule_defaults(source, Some("999"), None).self_schedule_requested,
            "force_group_schedule": group,
        })
    };
    let me = world
        .run_tool(
            &step(Some("me"), "what's for me today?", false),
            &mut session,
        )
        .await;
    assert!(me.ok);
    let invented = world
        .run_tool(
            &step(Some("<@123>"), "<@999> what's for me today?", false),
            &mut session,
        )
        .await;
    assert!(invented.ok);
    assert_eq!(invented.output, me.output);
    for bot_name in ["@Kanade", "kanade", "@kAnAdE"] {
        let copied_bot = world
            .run_tool(
                &step(Some(bot_name), "<@999> whats for me today?", false),
                &mut session,
            )
            .await;
        assert!(copied_bot.ok, "{bot_name}");
        assert_eq!(copied_bot.output, me.output, "{bot_name}");
    }
    for self_only in [
        "my schedule this week",
        "what's my schedule today?",
        "what is my schedule?",
        "what's on my schedule today?",
        "show me my schedule tomorrow",
    ] {
        let omitted = world
            .run_tool(&step(None, self_only, false), &mut session)
            .await;
        assert_eq!(omitted.output, me.output, "{self_only}");
        let unrecognized = world
            .run_tool(&step(Some("<@123>"), self_only, false), &mut session)
            .await;
        assert_eq!(unrecognized.output, me.output, "{self_only}");
    }

    let unknown = world
        .run_tool(
            &step(Some("<@123>"), "what's for Kanon today?", false),
            &mut session,
        )
        .await;
    assert!(!unknown.ok);
    assert_eq!(
        unknown.output,
        "That does not identify one person on the roster. Ask who they mean."
    );
    for unsafe_name in ["@Kanade", "@Kanade and Kanon", "@NotTheBot"] {
        let not_self = world
            .run_tool(
                &step(Some(unsafe_name), "what's for Kanon today?", false),
                &mut session,
            )
            .await;
        assert!(!not_self.ok, "{unsafe_name}");
    }
    let mixed_participant = world
        .run_tool(
            &step(Some("@Kanade and Kanon"), "what's for me today?", false),
            &mut session,
        )
        .await;
    assert!(!mixed_participant.ok);
    let other = world
        .run_tool(
            &step(Some("kanon"), "what's for me today?", false),
            &mut session,
        )
        .await;
    let other_without_self = world
        .run_tool(
            &step(Some("kanon"), "what's for Kanon today?", false),
            &mut session,
        )
        .await;
    assert!(other.ok);
    assert_eq!(other.output, other_without_self.output);
    let group = world
        .run_tool(&step(None, "what's for me today?", true), &mut session)
        .await;
    let all = world
        .run_tool(&step(None, "what's on today?", false), &mut session)
        .await;
    assert_eq!(group.output, all.output);
    for mixed in [
        "what's for me and Kanon today?",
        "my runs with Kanon today?",
        "what's for me, Kanon, today?",
        "what's my schedule and Kanon today?",
        "my schedule for Kanon today?",
        "my schedule <@5000> today?",
        "my schedule everyone today?",
        "what's for me + Kanon today?",
    ] {
        let missing = world
            .run_tool(&step(None, mixed, false), &mut session)
            .await;
        assert_eq!(missing.output, all.output, "{mixed}");
        let unrecognized = world
            .run_tool(&step(Some("<@123>"), mixed, false), &mut session)
            .await;
        assert!(!unrecognized.ok, "{mixed}");
        assert_eq!(unrecognized.output, unknown.output, "{mixed}");
    }
    let mut collision_input = fixture["input"].clone();
    collision_input["bot_user"]["name"] = json!("Kanon");
    let mut collision_world = World::new(&collision_input).await;
    let recognized_member = collision_world
        .run_tool(
            &step(Some("@Kanon"), "what's for me today?", false),
            &mut session,
        )
        .await;
    assert!(recognized_member.ok);
    assert_eq!(recognized_member.output, other.output);
}

/// The live guide over tracked schema v2 knowledge: v4's section shape,
/// letter-keyed `difficulty_notes` under their difficulty, never sources.
#[test]
fn strategy_guides_render_tracked_knowledge_in_the_v4_shape() {
    use std::path::Path;

    use kanade::chat::tools::read::render_guide;
    use kanade::domain::catalog::BossReference;
    use kanade::infrastructure::files::{load_catalog, load_knowledge_dir};

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog = load_catalog(&root.join("boss/bosses.yaml")).expect("shipped catalog");
    let knowledge = load_knowledge_dir(&root.join("boss/knowledge")).expect("knowledge");
    let guide = |short: &str, difficulty: Option<&str>| {
        let (document, researched) = knowledge
            .guide_source(short)
            .expect("readable")
            .expect("tracked document");
        let reference = BossReference {
            short: short.into(),
            difficulty: difficulty.map(str::to_owned),
        };
        render_guide(&document, &researched, &catalog, &reference).expect("guide")
    };

    // Structural only: the tracked prose is rewritten as research moves.
    let seren = guide("Seren", None);
    assert!(seren.starts_with("# Chosen Seren (Seren)\n"), "{seren}");
    for section in ["## Core\n- ", "## Danger\n- ", "## Tips\n- "] {
        assert!(seren.contains(&format!("\n\n{section}")), "{seren}");
    }
    assert!(seren.contains("\n\n## Difficulty notes\n### "), "{seren}");
    assert!(seren.contains("\n### Extreme\n"), "{seren}");
    assert!(
        !seren.contains("## Sources") && !seren.contains("https://"),
        "{seren}"
    );
    let seren_hard = guide("Seren", Some("h"));
    assert!(
        !seren_hard.contains("\n### Extreme\n") && !seren_hard.contains("\n### Normal\n"),
        "{seren_hard}"
    );

    let hard = guide("MaleficStar", Some("h"));
    assert!(hard.contains("### Hard\n- Entry level: 280\n"), "{hard}");
    for fact in [
        "- PDR: 380%\n",
        "- Party max: 3\n",
        "- Authentic Force: 550\n",
        "- HP: total 14.74q",
    ] {
        assert!(hard.contains(fact), "{fact}: {hard}");
    }
    assert!(!hard.contains("### Normal"), "{hard}");
    assert!(
        knowledge
            .guide_source("NotABoss")
            .expect("no read")
            .is_none()
    );
}

/// D-AUTO-FORWARD fixture (invented): Sat 3 Oct 2026 16:24 Kuala Lumpur, boss
/// weeks reset Thu 00:00. Member 11 finished runs earlier this week and has
/// planned runs on Mon 5 and Tue 6 Oct (this boss week, next calendar week)
/// and Fri 9 Oct (next boss week); member 44's only run is done.
async fn forward_world() -> World {
    let vectors = load("read_tools.json");
    let mut input = vectors["cases"][0]["input"].clone();
    input["clock"] = json!("2026-10-03T16:24:00+08:00");
    let run = |id: &str,
               at: &str,
               week: &str,
               bosses: &[&str],
               who: &[&str],
               channel: &str,
               status: &str| {
        json!({
            "id": id, "at": at, "week_start": week, "bosses": bosses, "participants": who,
            "channel_id": channel, "status": status, "fixed_run_id": null,
        })
    };
    let (previous, current, next) = (
        "2026-09-24T00:00:00+08:00",
        "2026-10-01T00:00:00+08:00",
        "2026-10-08T00:00:00+08:00",
    );
    input["world"]["fixed"] = json!([]);
    input["world"]["rsvps"] = json!([]);
    input["world"]["runs"] = json!([
        run(
            "d1d1d1d1-0000-4000-8000-000000000001",
            "2026-09-29T21:00:00+08:00",
            previous,
            &["HMaleficStar"],
            &["11"],
            "900",
            "done"
        ),
        run(
            "d3d3d3d3-0000-4000-8000-000000000003",
            "2026-09-30T21:00:00+08:00",
            previous,
            &["HCarling"],
            &["44"],
            "702",
            "done"
        ),
        run(
            "d2d2d2d2-0000-4000-8000-000000000002",
            "2026-10-02T21:00:00+08:00",
            current,
            &["XKalos"],
            &["11", "22"],
            "901",
            "planned"
        ),
        run(
            "0a0a0a0a-0000-4000-8000-000000000004",
            "2026-10-03T20:00:00+08:00",
            current,
            &["HCarling"],
            &["33"],
            "702",
            "planned"
        ),
        run(
            "c1c1c1c1-0000-4000-8000-000000000005",
            "2026-10-04T20:00:00+08:00",
            current,
            &["HBaldrix"],
            &["11"],
            "900",
            "cancelled"
        ),
        run(
            "e1e1e1e1-0000-4000-8000-000000000006",
            "2026-10-05T21:00:00+08:00",
            current,
            &["HMaleficStar", "HFA"],
            &["11", "22"],
            "900",
            "planned"
        ),
        run(
            "e2e2e2e2-0000-4000-8000-000000000007",
            "2026-10-06T22:00:00+08:00",
            current,
            &["HBaldrix"],
            &["11"],
            "700",
            "planned"
        ),
        run(
            "e3e3e3e3-0000-4000-8000-000000000008",
            "2026-10-09T21:00:00+08:00",
            next,
            &["XKalos"],
            &["11", "33"],
            "901",
            "planned"
        ),
    ]);
    World::new(&input).await
}

fn schedule_step(author: &str, channel: &str, arguments: Value, upcoming_only: bool) -> Value {
    json!({
        "op": "run", "tool": "get_schedule", "author_id": author, "channel_id": channel,
        "arguments": arguments, "upcoming_only": upcoming_only,
    })
}

#[tokio::test]
async fn auto_without_a_day_lists_upcoming_runs_from_now_across_weeks() {
    let mut world = forward_world().await;
    let mut session = PassthroughSession;
    let mut run = async |author: &str, channel: &str, arguments: Value, upcoming: bool| {
        let outcome = world
            .run_tool(
                &schedule_step(author, channel, arguments, upcoming),
                &mut session,
            )
            .await;
        assert!(outcome.ok, "{}", outcome.output);
        outcome.output
    };
    let mine = json!({"participant": "<@11>", "week": "auto"});

    // The live bug: Mon 5 Oct comes first, across both boss weeks.
    let forward = run("11", "700", mine.clone(), true).await;
    assert_eq!(
        forward,
        "**Your 3 upcoming runs · All channels**\n\n\
         `[e1e1e1e1]` **Hard MaleficStar + Hard FA**\n*Mon 05 Oct · 21:00* · `planned` · `0/2 yes` · <#900>\n\n\
         `[e2e2e2e2]` **Hard Baldrix**\n*Tue 06 Oct · 22:00* · `planned` · `0/1 yes` · <#700>\n\n\
         `[e3e3e3e3]` **Extreme Kalos**\n*Fri 09 Oct · 21:00* · `planned` · `0/2 yes` · <#901>"
    );
    // Forward from now whatever the question said.
    assert_eq!(run("11", "700", mine.clone(), false).await, forward);
    // A calendar week keeps its meaning and still excludes Mon 5 Oct.
    let calendar = json!({"participant": "<@11>", "week": "this"});
    assert_eq!(
        run("11", "700", calendar, true).await,
        "**No upcoming runs for you in this week.** Your matching scheduled runs are already done."
    );
    // `auto` with a day keeps its single-day meaning.
    let monday = json!({"participant": "<@11>", "week": "auto", "day": "monday"});
    assert!(
        run("11", "700", monday, true)
            .await
            .starts_with("**Your 1 run left Mon 05 Oct · All channels**")
    );

    // Everything this member had is done.
    let done = json!({"participant": "me", "week": "auto"});
    assert_eq!(
        run("44", "700", done, true).await,
        "**No upcoming runs for you.** Your matching scheduled runs are already done."
    );

    // Channel scope, with the away note.
    let here = json!({"participant": "<@11>", "week": "auto", "scope": "channel"});
    assert_eq!(
        run("11", "700", here.clone(), true).await,
        "**Your 1 upcoming run · This channel**\n\n\
         `[e2e2e2e2]` **Hard Baldrix**\n*Tue 06 Oct · 22:00* · `planned` · `0/1 yes`"
    );
    assert_eq!(
        run("11", "703", here, true).await,
        "**No upcoming runs for you in this channel.** You have 3 upcoming runs in other channels."
    );
    let group_here = json!({"week": "auto", "scope": "channel"});
    assert_eq!(
        run("22", "703", group_here, false).await,
        "**No upcoming runs in this channel.** The group has 4 upcoming runs in other channels."
    );
    let group = json!({"week": "auto"});
    assert!(
        run("22", "700", group.clone(), false)
            .await
            .starts_with("**4 upcoming runs · All channels**\n\n`[0a0a0a0a]`")
    );

    // Everything stored is over: done this week, then nothing at all.
    world
        .clock
        .set(instant(&json!("2026-10-10T12:00:00+08:00")));
    let mut run = async |arguments: Value| {
        let step = schedule_step("22", "700", arguments, false);
        world.run_tool(&step, &mut session).await.output
    };
    assert_eq!(
        run(group.clone()).await,
        "**No runs left · All channels**\n\nEverything scheduled is already done."
    );
    assert_eq!(
        run(json!({"participant": "me", "week": "auto"})).await,
        "**No upcoming runs for you.** Your matching scheduled runs are already done."
    );
    world
        .clock
        .set(instant(&json!("2026-10-20T12:00:00+08:00")));
    let mut run = async |arguments: Value| {
        let step = schedule_step("22", "700", arguments, false);
        world.run_tool(&step, &mut session).await.output
    };
    assert_eq!(run(group).await, "**No upcoming runs · All channels.**");
    assert_eq!(
        run(json!({"participant": "me", "week": "auto"})).await,
        "**No upcoming runs for you.**"
    );
}
