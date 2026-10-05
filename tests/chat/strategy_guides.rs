//! `get_boss_strategy` over a schema v2 knowledge directory of invented
//! documents: strategies, force names, and event bosses outside the catalog.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use kanade::chat::tools::read::{
    EventBoss, GuideError, StrategyGuides, get_boss_strategy, list_bosses, render_guide,
};
use kanade::domain::catalog::{BossReference, BossTable};
use kanade::domain::schedule::ScheduleSnapshot;
use kanade::infrastructure::files::{KnowledgeDir, load_catalog, load_knowledge_dir};
use serde_json::{Map, Value, json};

use crate::support::load;
use crate::world::World;

const META: &str = "schema_version: 2\nresearched_as_of: '2031-04-05'\n";

const SOURCE: &str = "sources:
- url: https://example.invalid/guide
  title: Invented guide
  author: Fixture
  kind: guide
  fetched: '2031-04-01'
";

const EVENT: &str = "boss: Zephyrine
event:
  name: Invented Winds Season
  availability: 'Invented World only, 1 Jan 2031 until 1 Mar 2031; solo only.'
  aliases: [Zephy, 제피린]
summary: Zephyrine is an invented solo wind dancer.
core:
- Gusts push you toward the edge.
danger:
- Cyclone wipes the floor.
tips:
- Stand centre.
strategies:
- name: Safe kite
  when: First clears.
  risk: low
  damage: low
  payoff: A steady clear.
  steps:
  - Kite the gusts.
  - Burst on the calm.
- name: Burn rush
  when: Strong characters.
  risk: high
  damage: high
  payoff: Skips the second cyclone.
  steps:
  - Burst at the opening.
difficulties:
- name: Normal
  entry_level: 250
  force: {kind: arcane, value: 1100}
- name: Hard
  entry_level: 275
  pdr_percent: 300
  force: {kind: sacred, value: 330}
";

const CATALOG_DOC: &str = "boss: MaleficStar
summary: An invented star summary.
core:
- Invented core bullet.
danger:
- Invented danger bullet.
tips:
- Invented tip.
difficulties:
- name: Hard
  entry_level: 280
  force: {kind: sacred, value: 550}
";

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A fresh invented knowledge directory, removed on drop.
struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "kanade-chat-guides-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        fs::copy(
            root.join("boss/knowledge/schema.json"),
            dir.join("schema.json"),
        )
        .unwrap();
        fs::write(dir.join("_meta.yaml"), META).unwrap();
        fs::write(dir.join("zephyrine.yaml"), format!("{EVENT}{SOURCE}")).unwrap();
        fs::write(
            dir.join("maleficstar.yaml"),
            format!("{CATALOG_DOC}{SOURCE}"),
        )
        .unwrap();
        Self(dir)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn catalog() -> BossTable {
    load_catalog(&Path::new(env!("CARGO_MANIFEST_DIR")).join("boss/bosses.yaml"))
        .expect("shipped catalog")
}

/// Serve's `LiveStrategyGuides` over the fixture directory.
struct Guides<'a> {
    knowledge: &'a KnowledgeDir,
    catalog: &'a BossTable,
}

impl StrategyGuides for Guides<'_> {
    fn render(&self, reference: &BossReference) -> Result<String, GuideError> {
        let (document, researched) = self
            .knowledge
            .guide_source(&reference.short)
            .map_err(|_| GuideError::Unreadable)?
            .ok_or(GuideError::Missing)?;
        render_guide(&document, &researched, self.catalog, reference).ok_or(GuideError::Unreadable)
    }

    fn events(&self) -> Vec<EventBoss> {
        self.knowledge
            .events
            .iter()
            .filter(|event| self.catalog.boss(&event.key).is_none())
            .map(|event| EventBoss {
                key: event.key.clone(),
                name: event.name.clone(),
                availability: event.availability.clone(),
                aliases: event.aliases.clone(),
            })
            .collect()
    }
}

/// Calls `get_boss_strategy` with `args` over the fixture.
async fn strategy(args: Value) -> Result<String, String> {
    strategy_with(&[], args).await
}

/// As [`strategy`], with `extra` `(file, text)` documents added to the fixture.
async fn strategy_with(extra: &[(&str, &str)], args: Value) -> Result<String, String> {
    let fixture = Fixture::new();
    for (file, text) in extra {
        fs::write(fixture.0.join(file), format!("{text}{SOURCE}")).unwrap();
    }
    let knowledge = load_knowledge_dir(&fixture.0).expect("fixture knowledge");
    let catalog = catalog();
    let guides = Guides {
        knowledge: &knowledge,
        catalog: &catalog,
    };
    let vectors = load("read_tools.json");
    let world = World::new(&vectors["cases"][0]["input"]).await;
    let snapshot = ScheduleSnapshot::default();
    let tools = kanade::chat::tools::read::ToolWorld {
        catalog: &catalog,
        guides: Some(&guides),
        ..world.tool_world(&snapshot)
    };
    let args: Map<String, Value> = args.as_object().unwrap().clone();
    get_boss_strategy(&tools, &args).map_err(|error| error.0)
}

#[test]
fn knowledge_records_event_documents_with_name_availability_and_aliases() {
    let fixture = Fixture::new();
    let knowledge = load_knowledge_dir(&fixture.0).expect("fixture knowledge");
    assert_eq!(knowledge.keys, ["MaleficStar", "Zephyrine"]);
    assert_eq!(knowledge.events.len(), 1);
    assert_eq!(knowledge.events[0].key, "Zephyrine");
    assert_eq!(knowledge.events[0].name, "Invented Winds Season");
    assert_eq!(
        knowledge.events[0].availability,
        "Invented World only, 1 Jan 2031 until 1 Mar 2031; solo only."
    );
    assert_eq!(knowledge.events[0].aliases, ["Zephy", "제피린"]);
}

#[tokio::test]
async fn d_seasonal_list_appends_tracked_guides_after_the_unchanged_catalog_block() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog = load_catalog(&root.join("boss/bosses.yaml")).expect("shipped catalog");
    let knowledge = load_knowledge_dir(&root.join("boss/knowledge")).expect("knowledge");
    let guides = Guides {
        knowledge: &knowledge,
        catalog: &catalog,
    };
    let vectors = load("read_tools.json");
    let world = World::new(&vectors["cases"][0]["input"]).await;
    let snapshot = ScheduleSnapshot::default();
    let catalog_world = kanade::chat::tools::read::ToolWorld {
        catalog: &catalog,
        guides: None,
        ..world.tool_world(&snapshot)
    };
    let catalog_only = list_bosses(&catalog_world);
    let tool_world = kanade::chat::tools::read::ToolWorld {
        guides: Some(&guides),
        ..catalog_world
    };

    assert_eq!(
        list_bosses(&tool_world),
        format!(
            "{catalog_only}\n\n**Seasonal bosses (guide only, not scheduled)**\n\
             **Kai** (Challengers World Season 3): MapleSEA: ran in Challengers World Season 3 only, from 3 Jun 2026 (after the v251 patch) until it ended at the 30 Sep 2026 maintenance; solo only. Not currently available.\n\
             **Meilin** (Challengers World Season 4): KMS Challengers World Season 4 ran 18 Jun – 17 Sep 2026. Expected in MapleSEA around Nov 2026 (not yet announced); MapleSEA names may differ. Also called Maerin, 메이린."
        )
    );
}

#[tokio::test]
async fn list_bosses_omits_the_seasonal_section_without_event_guides() {
    let vectors = load("read_tools.json");
    let world = World::new(&vectors["cases"][0]["input"]).await;
    let snapshot = ScheduleSnapshot::default();
    let listing = list_bosses(&world.tool_world(&snapshot));

    assert!(!listing.contains("**Seasonal bosses (guide only, not scheduled)**"));
}

#[tokio::test]
async fn an_event_guide_is_found_by_key_and_shows_availability_and_strategies() {
    let guide = strategy(json!({"boss": "zephyrine"})).await.unwrap();
    assert!(
        guide.starts_with(
            "# Zephyrine\nAvailability: Invented World only, 1 Jan 2031 until 1 Mar 2031; solo only.\n_Researched as of 2031-04-05._\n"
        ),
        "{guide}"
    );
    assert!(
        guide.contains(
            "## Tips\n- Stand centre.\n\n## Strategies\n### Safe kite\n- When: First clears.\n\
             - Risk: low; damage needed: low\n- Payoff: A steady clear.\n- Steps:\n  \
             1. Kite the gusts.\n  2. Burst on the calm.\n### Burn rush\n"
        ),
        "{guide}"
    );
    assert!(
        guide.contains("- Risk: high; damage needed: high\n"),
        "{guide}"
    );
    assert!(
        guide.contains("### Normal\n- Entry level: 250\n- Arcane Force: 1100\n"),
        "{guide}"
    );
    assert!(
        guide.contains("- PDR: 300%\n- Authentic Force: 330"),
        "{guide}"
    );
    assert!(!guide.contains("Force: sacred"), "{guide}");
    assert!(!guide.contains("## Sources") && !guide.contains("https://"));
}

#[tokio::test]
async fn an_event_guide_is_found_by_alias_case_insensitively() {
    for name in ["ZEPHY", "zephy", "제피린", " Zephy "] {
        let guide = strategy(json!({"boss": name})).await.unwrap();
        assert!(
            guide.starts_with("# Zephyrine\nAvailability: "),
            "{name}: {guide}"
        );
    }
}

#[tokio::test]
async fn an_event_difficulty_word_filters_like_a_catalog_boss() {
    for args in [
        json!({"boss": "Hard Zephyrine"}),
        json!({"boss": "zephy hard"}),
        json!({"boss": "h 제피린"}),
        json!({"boss": "HZephy"}),
        json!({"boss": "Zephyrine", "difficulty": "Hard"}),
        json!({"boss": "Hard Zephy", "difficulty": "h"}),
    ] {
        let guide = strategy(args.clone()).await.unwrap();
        assert!(
            guide.contains("## Difficulty notes\n### Hard\n"),
            "{args}: {guide}"
        );
        assert!(!guide.contains("### Normal"), "{args}: {guide}");
    }
    let all = strategy(json!({"boss": "Zephy"})).await.unwrap();
    assert!(
        all.contains("### Normal") && all.contains("### Hard"),
        "{all}"
    );
    assert_eq!(
        strategy(json!({"boss": "Hard Zephy", "difficulty": "Normal"})).await,
        Err("conflicting difficulties: Hard and Normal".to_owned())
    );
    assert_eq!(
        strategy(json!({"boss": "Hard Zephy Normal"})).await,
        Err("conflicting difficulties: Hard and Normal".to_owned())
    );
}

#[tokio::test]
async fn an_unknown_name_keeps_the_catalog_error() {
    assert_eq!(
        strategy(json!({"boss": "NotABoss"})).await,
        Err("no boss found in `NotABoss`".to_owned())
    );
    // Difficulty words alone never name an event.
    assert_eq!(
        strategy(json!({"boss": "hard"})).await,
        Err("no boss found in `hard`".to_owned())
    );
}

#[tokio::test]
async fn catalog_bosses_keep_their_catalog_heading() {
    let guide = strategy(json!({"boss": "hard star"})).await.unwrap();
    assert!(
        guide.starts_with("# Radiant Malefic Star (MaleficStar)\n_Researched as of 2031-04-05._\n"),
        "{guide}"
    );
    assert!(!guide.contains("Availability:"), "{guide}");
    assert!(!guide.contains("## Strategies"), "{guide}");
    assert!(
        guide.contains("### Hard\n- Entry level: 280\n- Authentic Force: 550"),
        "{guide}"
    );
}

/// An invented document with every new shape (titled items with and without
/// `detail`, phases beside cross-phase `core`, mechanics, hp `count`, spec
/// figures and a mission).
const NEW_SHAPES: &str = "boss: Kalos
lead: An invented page lead the bot never reads.
summary: An invented summary the bot reads.
core:
- Invented cross-phase rule.
- title: Shared gauge
  text: Invented short gauge text.
  detail: Invented long gauge wording only the bot reads.
phases:
- name: Phase 1
  items:
  - Invented opener.
  - title: Orb sweep
    text: Invented short sweep text.
- name: Phase 2
  group: Invented loop
  cycle: true
  tag: Invented tag
  tone: safe
  items:
  - title: Floor split
    text: Invented short split text.
    detail: Invented long split wording.
danger:
- title: Wipe beam
  text: Invented beam text.
tips:
- Invented tip.
difficulty_notes:
  x:
    title: Extreme limit
    text: Invented short extreme note.
    detail: Invented long extreme note.
  n: Invented plain normal note.
notes:
- title: Naming
  text: Invented naming note.
mechanics:
- kind: ledger
  title: Invented gauge ledger
  rows:
  - {label: Invented fall, value: '-200', direction: down}
  - {label: Invented clear, value: '-150', direction: up}
  - {label: Idle, value: '0'}
  note: Invented ledger note.
- kind: zones
  title: Invented arena
  zones:
  - {name: Left, sub: Beam side, tone: risk}
  - {name: Right, tone: safe}
  - {name: Centre, sub: Neutral, tone: blue}
- kind: scale
  title: Invented HP bands
  bands:
  - {label: Early, span: 20, tone: green}
  - {label: Late, span: 80}
difficulties:
- name: Extreme
  entry_level: 265
  hp:
  - {phase: '1', value: '100.5t'}
  - {phase: '2', value: '20t', count: 3, target: Invented guards}
  recommended_spec:
    kind: Combat power
    text: Invented spec text.
    value: '≈ 86k'
    basis: Invented basis
  notes:
  - title: Extreme item
    text: Invented short.
    detail: Invented long extreme detail.
- name: Destiny
  mission:
    series: destiny-weapon
    order: 3
    title: Invented mission title
    modifier: {text: Invented modifier, direction: up}
    needs: Invented needs
    rules: [Invented rule one, Invented rule two]
";

const NEW_SHAPES_GUIDE: &str = "# Gatekeeper Kalos (Kalos)
_Researched as of 2031-04-05._

An invented summary the bot reads.

## Core
- Invented cross-phase rule.
- Shared gauge: Invented long gauge wording only the bot reads.

## Phases
### Phase 1
- Invented opener.
- Orb sweep: Invented short sweep text.
### Phase 2 (Invented loop, repeating): Invented tag
- Floor split: Invented long split wording.

## Danger
- Wipe beam: Invented beam text.

## Tips
- Invented tip.

## Mechanics
### Invented gauge ledger
- Invented fall: -200 (against you)
- Invented clear: -150 (in your favour)
- Idle: 0
- Note: Invented ledger note.
### Invented arena
- Left: Beam side (risky)
- Right (safe)
- Centre: Neutral
### Invented HP bands
- Early: 20/100
- Late: 80/100

## Difficulty notes
### Extreme
- Entry level: 265
- HP: 1 100.5t, 2 20t ×3 (Invented guards)
- Recommended (Combat power): Invented spec text. (≈ 86k; Invented basis)
- Extreme item: Invented long extreme detail.
Extreme limit: Invented long extreme note.
### Destiny
- Mission: Invented mission title (Destiny Weapon mission 3)
- Mission modifier: Invented modifier (in your favour)
- Mission needs: Invented needs
- Mission rules: Invented rule one; Invented rule two
### Normal
Invented plain normal note.

## Notes
- Naming: Invented naming note.";

#[tokio::test]
async fn a_new_shape_guide_renders_details_phases_missions_and_mechanics() {
    let guide = strategy_with(&[("kalos.yaml", NEW_SHAPES)], json!({"boss": "kalos"}))
        .await
        .unwrap();
    assert_eq!(guide, NEW_SHAPES_GUIDE);
    assert!(!guide.contains("page lead"), "lead is UI-only");
}

#[tokio::test]
async fn phases_without_core_skip_the_core_heading() {
    let doc = NEW_SHAPES.replacen(
        "core:\n- Invented cross-phase rule.\n- title: Shared gauge\n  text: Invented short gauge text.\n  detail: Invented long gauge wording only the bot reads.\n",
        "",
        1,
    );
    assert_ne!(doc, NEW_SHAPES);
    let guide = strategy_with(&[("kalos.yaml", &doc)], json!({"boss": "kalos"}))
        .await
        .unwrap();
    assert!(!guide.contains("## Core"), "{guide}");
    assert!(
        guide.contains("An invented summary the bot reads.\n\n## Phases\n### Phase 1\n"),
        "{guide}"
    );
}
