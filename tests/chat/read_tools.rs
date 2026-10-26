//! Read tools and the dispatcher boundary, through `dispatch::run` in the
//! full-set (v4) mode with a passthrough identity session.

use std::path::Path;

use kanade::chat::sanitize::schedule_defaults;
use kanade::chat::tools::ToolOutcome;
use kanade::chat::tools::read::render_guide;
use kanade::infrastructure::files::{load_catalog, load_knowledge_dir};
use kanade::infrastructure::llm::identity::PassthroughSession;
use kanade_domain::catalog::BossReference;
use serde_json::{Value, json};

use crate::common::instant;
use crate::support::world_input;
use crate::world::World;

/// Member 22's `get_schedule` for this week in 700, asked as `source`:
/// `participant` as the model filled it, `group` forcing a group listing.
async fn schedule_for(
    world: &mut World,
    participant: Option<&str>,
    source: &str,
    group: bool,
) -> ToolOutcome {
    let mut arguments = json!({"week": "this"});
    if let Some(participant) = participant {
        arguments["participant"] = json!(participant);
    }
    let step = json!({
        "op": "run", "tool": "get_schedule", "author_id": "22", "channel_id": "700",
        "arguments": arguments,
        "self_schedule_requested": schedule_defaults(source, Some("999"), None).self_schedule_requested,
        "force_group_schedule": group,
    });
    world.run_tool(&step, &mut PassthroughSession).await
}

#[tokio::test]
async fn an_explicit_self_schedule_recovers_only_unrecognized_model_mentions() {
    let input = world_input();
    let mut world = World::new(&input).await;
    let w = &mut world;
    let me = schedule_for(w, Some("me"), "what's for me today?", false).await;
    assert!(me.ok);
    let invented = schedule_for(w, Some("<@123>"), "<@999> what's for me today?", false).await;
    assert!(invented.ok);
    assert_eq!(invented.output, me.output);
    for bot_name in ["@Kanade", "kanade", "@kAnAdE"] {
        let copied_bot = schedule_for(w, Some(bot_name), "<@999> whats for me today?", false).await;
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
        let omitted = schedule_for(w, None, self_only, false).await;
        assert_eq!(omitted.output, me.output, "{self_only}");
        let unrecognized = schedule_for(w, Some("<@123>"), self_only, false).await;
        assert_eq!(unrecognized.output, me.output, "{self_only}");
    }

    let unknown = schedule_for(w, Some("<@123>"), "what's for Kanon today?", false).await;
    assert!(!unknown.ok);
    assert_eq!(
        unknown.output,
        "That does not identify one person on the roster. Ask who they mean."
    );
    for unsafe_name in ["@Kanade", "@Kanade and Kanon", "@NotTheBot"] {
        let not_self = schedule_for(w, Some(unsafe_name), "what's for Kanon today?", false).await;
        assert!(!not_self.ok, "{unsafe_name}");
    }
    let mixed_participant =
        schedule_for(w, Some("@Kanade and Kanon"), "what's for me today?", false).await;
    assert!(!mixed_participant.ok);
    let other = schedule_for(w, Some("kanon"), "what's for me today?", false).await;
    let other_without_self = schedule_for(w, Some("kanon"), "what's for Kanon today?", false).await;
    assert!(other.ok);
    assert_eq!(other.output, other_without_self.output);
    let group = schedule_for(w, None, "what's for me today?", true).await;
    let all = schedule_for(w, None, "what's on today?", false).await;
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
        let missing = schedule_for(w, None, mixed, false).await;
        assert_eq!(missing.output, all.output, "{mixed}");
        let unrecognized = schedule_for(w, Some("<@123>"), mixed, false).await;
        assert!(!unrecognized.ok, "{mixed}");
        assert_eq!(unrecognized.output, unknown.output, "{mixed}");
    }
    let mut collision_input = input.clone();
    collision_input["bot_user"]["name"] = json!("Kanon");
    let mut collision_world = World::new(&collision_input).await;
    let recognized_member = schedule_for(
        &mut collision_world,
        Some("@Kanon"),
        "what's for me today?",
        false,
    )
    .await;
    assert!(recognized_member.ok);
    assert_eq!(recognized_member.output, other.output);
}

/// The live guide of `short` (at `difficulty`) over the tracked catalog and
/// knowledge, or `None` when the boss has no tracked document.
fn guide(short: &str, difficulty: Option<&str>) -> Option<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog = load_catalog(&root.join("boss/bosses.yaml")).expect("shipped catalog");
    let knowledge = load_knowledge_dir(&root.join("boss/knowledge")).expect("knowledge");
    let (document, researched) = knowledge.guide_source(short).expect("readable")?;
    let reference = BossReference {
        short: short.into(),
        difficulty: difficulty.map(str::to_owned),
    };
    Some(render_guide(&document, &researched, &catalog, &reference).expect("guide"))
}

/// Every tracked knowledge document renders for the bot, whole and per
/// difficulty letter, so a content edit can never leave a boss unreadable.
#[test]
fn every_tracked_guide_renders_for_every_difficulty() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog = load_catalog(&root.join("boss/bosses.yaml")).expect("shipped catalog");
    let knowledge = load_knowledge_dir(&root.join("boss/knowledge")).expect("knowledge");
    assert!(!knowledge.keys.is_empty());
    for key in &knowledge.keys {
        let (document, researched) = knowledge
            .guide_source(key)
            .expect("readable")
            .expect("tracked document");
        for difficulty in [None, Some("e"), Some("n"), Some("h"), Some("c"), Some("x")] {
            let reference = BossReference {
                short: key.clone(),
                difficulty: difficulty.map(str::to_owned),
            };
            let text = render_guide(&document, &researched, &catalog, &reference);
            assert!(text.is_some(), "{key} {difficulty:?} is unreadable");
        }
    }
}

/// The live guide over tracked schema v2 knowledge: v4's section shape,
/// letter-keyed `difficulty_notes` under their difficulty, never sources.
#[test]
fn strategy_guides_render_tracked_knowledge_in_the_v4_shape() {
    // Structural only: the tracked prose is rewritten as research moves.
    let seren = guide("Seren", None).expect("tracked");
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
    let seren_hard = guide("Seren", Some("h")).expect("tracked");
    assert!(
        !seren_hard.contains("\n### Extreme\n") && !seren_hard.contains("\n### Normal\n"),
        "{seren_hard}"
    );

    let hard = guide("MaleficStar", Some("h")).expect("tracked");
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
    assert!(guide("NotABoss", None).is_none());
}

/// D-AUTO-FORWARD fixture (invented): Sat 3 Oct 2026 16:24 Kuala Lumpur, boss
/// weeks reset Thu 00:00. Member 11 finished runs earlier this week and has
/// planned runs on Mon 5 and Tue 6 Oct (this boss week, next calendar week)
/// and Fri 9 Oct (next boss week); member 44's only run is done.
async fn forward_world() -> World {
    forward_world_with(json!([])).await
}

async fn forward_world_with(rsvps: Value) -> World {
    let mut input = world_input();
    input["clock"] = json!("2026-10-03T16:24:00+08:00");
    let weeks = ["2026-09-24", "2026-10-01", "2026-10-08"];
    // Id stem, start, boss week, bosses, party, channel, status; the n-th
    // row's id is `<stem×4>-0000-4000-8000-00000000000n`.
    let runs = [
        (
            "d1",
            "2026-09-29T21:00",
            0,
            "HMaleficStar",
            "11",
            "900",
            "done",
        ),
        ("d3", "2026-09-30T21:00", 0, "HCarling", "44", "702", "done"),
        (
            "d2",
            "2026-10-02T21:00",
            1,
            "XKalos",
            "11 22",
            "901",
            "planned",
        ),
        (
            "0a",
            "2026-10-03T20:00",
            1,
            "HCarling",
            "33",
            "702",
            "planned",
        ),
        (
            "c1",
            "2026-10-04T20:00",
            1,
            "HBaldrix",
            "11",
            "900",
            "cancelled",
        ),
        (
            "e1",
            "2026-10-05T21:00",
            1,
            "HMaleficStar HFA",
            "11 22",
            "900",
            "planned",
        ),
        (
            "e2",
            "2026-10-06T22:00",
            1,
            "HBaldrix",
            "11",
            "700",
            "planned",
        ),
        (
            "e3",
            "2026-10-09T21:00",
            2,
            "XKalos",
            "11 33",
            "901",
            "planned",
        ),
    ];
    let runs: Vec<Value> = runs
        .iter()
        .enumerate()
        .map(|(n, (stem, at, week, bosses, who, channel, status))| {
            json!({
                "id": format!("{}-0000-4000-8000-{:012}", stem.repeat(4), n + 1),
                "at": format!("{at}:00+08:00"),
                "week_start": format!("{}T00:00:00+08:00", weeks[*week]),
                "bosses": bosses.split(' ').collect::<Vec<_>>(),
                "participants": who.split(' ').collect::<Vec<_>>(),
                "channel_id": channel, "status": status, "fixed_run_id": null,
            })
        })
        .collect();
    input["world"]["fixed"] = json!([]);
    input["world"]["rsvps"] = rsvps;
    input["world"]["runs"] = json!(runs);
    World::new(&input).await
}

fn schedule_step(author: &str, channel: &str, arguments: Value, question: &str) -> Value {
    let defaults = schedule_defaults(question, Some("5000"), None);
    json!({
        "op": "run", "tool": "get_schedule", "author_id": author, "channel_id": channel,
        "arguments": arguments, "upcoming_only": defaults.upcoming_only,
        "next_only": defaults.next_only,
    })
}

#[test]
fn only_a_singular_next_run_question_asks_for_one_run() {
    let next_only = |question: &str| schedule_defaults(question, Some("5000"), None).next_only;
    for singular in [
        "<@5000> when is my next run?",
        "my next boss run?",
        "When's the NEXT RUN",
        "what is our next run in here",
        "what's my next run",
        "when's our next run in here?",
        "<@5000> what’s my next run for me in this channel",
    ] {
        assert!(next_only(singular), "{singular}");
    }
    // Anything that may narrow the answer beyond what `get_schedule` filters
    // gets the full list back.
    for listing in [
        "when's the next run for hard lucid?",
        "my next run after Tuesday",
        "the next run and the one after",
        "next run times this week",
        "my next run this week",
        "when is my next run on friday",
        "when is <@123>'s next run",
        "when is kanon's next run",
        "<@5000> when are my next runs?",
        "what's for me next week?",
        "my runs left",
        "upcoming runs",
        "next boss runs",
        "runs next week",
    ] {
        assert!(!next_only(listing), "{listing}");
    }
}

#[tokio::test]
async fn auto_without_a_day_reads_what_is_left_of_this_boss_week() {
    let mut world = forward_world().await;
    let mut session = PassthroughSession;
    let mut run = async |author: &str, channel: &str, arguments: Value, question: &str| {
        let outcome = world
            .run_tool(
                &schedule_step(author, channel, arguments, question),
                &mut session,
            )
            .await;
        assert!(outcome.ok, "{}", outcome.output);
        outcome.output
    };
    let mine = json!({"participant": "<@11>", "week": "auto"});

    // The live shape: a singular "next run" is just Mon 5 Oct, with the
    // model-only context line (D-PERSONAL-CONTEXT).
    let next = run("11", "700", mine.clone(), "<@5000> when is my next run?").await;
    assert_eq!(
        next,
        "**Your next run · All channels**\n\n\
         `[e1e1e1e1]` **Hard MaleficStar + Hard FA**\n*Mon 05 Oct · 21:00* · `planned` · `0/2 yes` · <#900>\n\
         Context (hidden from members): in 2 days · you haven't answered · no answer yet from kanon"
    );
    // A plural ask lists this boss week's rest, never Fri 9 Oct (next boss week).
    let plural = run("11", "700", mine.clone(), "<@5000> when are my next runs?").await;
    assert_eq!(
        plural,
        "**Your 2 upcoming runs this boss week · All channels**\n\n\
         `[e1e1e1e1]` **Hard MaleficStar + Hard FA**\n*Mon 05 Oct · 21:00* · `planned` · `0/2 yes` · <#900>\n\
         Context (hidden from members): in 2 days · you haven't answered · no answer yet from kanon\n\n\
         `[e2e2e2e2]` **Hard Baldrix**\n*Tue 06 Oct · 22:00* · `planned` · `0/1 yes` · <#700>\n\
         Context (hidden from members): in 3 days · you haven't answered"
    );
    // Forward from now whatever the question said; earlier done runs never show.
    assert_eq!(run("11", "700", mine.clone(), "my schedule").await, plural);
    // Next boss week only when asked for explicitly.
    let next_boss = json!({"participant": "<@11>", "week": "next_boss"});
    assert!(
        run("11", "700", next_boss.clone(), "my runs next boss week")
            .await
            .starts_with("**Your 1 run next boss week · All channels**\n\n`[e3e3e3e3]`")
    );
    assert!(
        run("11", "700", next_boss, "when is my next run")
            .await
            .starts_with("**Your next run next boss week · All channels**\n\n`[e3e3e3e3]`")
    );
    // A calendar week keeps its meaning and still excludes Mon 5 Oct.
    let calendar = json!({"participant": "<@11>", "week": "this"});
    assert_eq!(
        run("11", "700", calendar, "my runs left this week").await,
        "**No upcoming runs for you in this week.** Your matching scheduled runs are already done."
    );
    // `auto` with a day keeps its single-day meaning.
    let monday = json!({"participant": "<@11>", "week": "auto", "day": "monday"});
    assert!(
        run("11", "700", monday, "my runs left on monday")
            .await
            .starts_with("**Your 1 run left Mon 05 Oct · All channels**")
    );

    // Everything this member had is done.
    let done = json!({"participant": "me", "week": "auto"});
    assert_eq!(
        run("44", "700", done.clone(), "when is my next run").await,
        "**No upcoming runs for you this boss week.** Your matching scheduled runs are already done."
    );

    // Channel scope, with the away note.
    let here = json!({"participant": "<@11>", "week": "auto", "scope": "channel"});
    assert_eq!(
        run("11", "700", here.clone(), "my upcoming runs in here").await,
        "**Your 1 upcoming run this boss week · This channel**\n\n\
         `[e2e2e2e2]` **Hard Baldrix**\n*Tue 06 Oct · 22:00* · `planned` · `0/1 yes`\n\
         Context (hidden from members): in 3 days · you haven't answered"
    );
    assert_eq!(
        run("11", "703", here, "my upcoming runs in here").await,
        "**No upcoming runs for you in this channel this boss week.** You have 2 upcoming runs in other channels."
    );
    let group_here = json!({"week": "auto", "scope": "channel"});
    assert_eq!(
        run("22", "703", group_here, "upcoming runs in here").await,
        "**No upcoming runs in this channel this boss week.** The group has 3 upcoming runs in other channels."
    );
    let group = json!({"week": "auto"});
    assert!(
        run("22", "700", group.clone(), "upcoming runs")
            .await
            .starts_with("**3 upcoming runs this boss week · All channels**\n\n`[0a0a0a0a]`")
    );
    assert!(
        run("22", "700", group.clone(), "when is the next run")
            .await
            .starts_with("**Next run · All channels**\n\n`[0a0a0a0a]`")
    );
    // The tool has no boss filter: a boss-qualified ask keeps the whole list.
    let boss_ask = run(
        "22",
        "700",
        group.clone(),
        "when's the next run for hard baldrix?",
    )
    .await;
    assert!(
        boss_ask.starts_with("**3 upcoming runs this boss week · All channels**")
            && boss_ask.contains("`[e2e2e2e2]` **Hard Baldrix**"),
        "{boss_ask}"
    );

    // Everything in reach is over: done this boss week, then nothing at all.
    for (clock, group_says, done_says) in [
        (
            "2026-10-10T12:00:00+08:00",
            "**No runs left this boss week · All channels**\n\nEverything scheduled this boss week is already done.",
            "**No upcoming runs for you this boss week.** Your matching scheduled runs are already done.",
        ),
        (
            "2026-10-20T12:00:00+08:00",
            "**No upcoming runs this boss week · All channels.**",
            "**No upcoming runs for you this boss week.**",
        ),
    ] {
        world.clock.set(instant(&json!(clock)));
        for (arguments, says) in [(&group, group_says), (&done, done_says)] {
            let step = schedule_step("22", "700", arguments.clone(), "when is my next run");
            let output = world.run_tool(&step, &mut session).await.output;
            assert_eq!(output, says, "{clock}");
        }
    }
}

/// D-PERSONAL-CONTEXT (user decision 2026-10-03): the context vocabulary is
/// relative to the turn clock in the guild zone; the asker's own answer
/// shows only on their own listing, and group listings carry no context.
#[tokio::test]
async fn personal_listings_carry_a_model_only_context_line() {
    let answers = json!([
        {"run_id": "e1e1e1e1-0000-4000-8000-000000000006", "state": "yes", "user_id": "11"},
    ]);
    let mut world = forward_world_with(answers).await;
    let mut session = PassthroughSession;
    let label = "Context (hidden from members): ";
    let cases = [
        // Mon 5 Oct 19:30: Hard MaleficStar + Hard FA at 21:00 tonight.
        (
            "2026-10-05T19:30:00+08:00",
            "11",
            json!({"participant": "me", "week": "auto"}),
            vec![
                "tonight · in 2 hours · you said yes · no answer yet from kanon",
                "tomorrow · you haven't answered",
            ],
        ),
        (
            "2026-10-05T20:40:00+08:00",
            "11",
            json!({"participant": "me", "week": "auto"}),
            vec![
                "tonight · in 20 minutes · you said yes · no answer yet from kanon",
                "tomorrow · you haven't answered",
            ],
        ),
        (
            "2026-10-05T09:00:00+08:00",
            "11",
            json!({"participant": "me", "week": "auto"}),
            vec![
                // Twelve hours out is past the hour phrase's reach.
                "tonight · you said yes · no answer yet from kanon",
                "tomorrow · you haven't answered",
            ],
        ),
        // Another member's listing: no own answer, the asker not singled out.
        (
            "2026-10-05T19:30:00+08:00",
            "22",
            json!({"participant": "<@11>", "week": "auto"}),
            vec![
                "tonight · in 2 hours · no answer yet from kanon",
                "tomorrow · no answer yet from Alvin tan",
            ],
        ),
        // A group listing has none.
        (
            "2026-10-05T19:30:00+08:00",
            "11",
            json!({"week": "auto"}),
            vec![],
        ),
    ];
    for (clock, author, arguments, expected) in cases {
        world.clock.set(instant(&json!(clock)));
        let step = schedule_step(author, "700", arguments, "my runs");
        let output = world.run_tool(&step, &mut session).await.output;
        let contexts: Vec<&str> = output
            .lines()
            .filter_map(|line| line.strip_prefix(label))
            .collect();
        assert_eq!(contexts, expected, "{clock} {output}");
    }
}
