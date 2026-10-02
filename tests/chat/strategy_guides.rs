//! `get_boss_strategy` over a schema v2 knowledge directory of invented
//! documents: strategies, force names, and event bosses outside the catalog.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use kanade::chat::tools::read::{
    EventBoss, GuideError, StrategyGuides, get_boss_strategy, render_guide,
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
                aliases: event.aliases.clone(),
            })
            .collect()
    }
}

/// Calls `get_boss_strategy` with `args` over the fixture.
async fn strategy(args: Value) -> Result<String, String> {
    let fixture = Fixture::new();
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
fn knowledge_records_event_documents_with_their_aliases() {
    let fixture = Fixture::new();
    let knowledge = load_knowledge_dir(&fixture.0).expect("fixture knowledge");
    assert_eq!(knowledge.keys, ["MaleficStar", "Zephyrine"]);
    assert_eq!(knowledge.events.len(), 1);
    assert_eq!(knowledge.events[0].key, "Zephyrine");
    assert_eq!(knowledge.events[0].aliases, ["Zephy", "제피린"]);
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
